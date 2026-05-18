// HEVC Main10 transcoder with two temporal sub-layers in the VPS. The
// aerials extension requires this exact bitstream shape for playback to work
// on subsequent lock cycles. AVAssetWriter doesn't expose the temporal
// sub-layer keys, so we drive VTCompressionSession directly and feed the
// NAL units through an AVAssetWriter in pass-through mode.
//
// Property dict copied from Wallper.app via lldb.

use std::ffi::c_void;
use std::path::Path;
use std::ptr::{self, NonNull};
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, bail};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAssetReader, AVAssetReaderStatus, AVAssetReaderTrackOutput, AVAssetWriter,
    AVAssetWriterInput, AVAssetWriterStatus, AVFileTypeQuickTimeMovie, AVMediaTypeVideo,
    AVURLAsset,
};
use objc2_core_foundation::{
    CFArray, CFArrayCallBacks, CFNumber, CFNumberType, CFRetained, CFString, CFType,
    kCFBooleanFalse, kCFBooleanTrue, kCFTypeArrayCallBacks,
};
use objc2_core_media::{CMSampleBuffer, kCMTimeInvalid, kCMVideoCodecType_HEVC};
use objc2_core_video::{
    CVImageBuffer, kCVImageBufferColorPrimariesKey, kCVImageBufferTransferFunctionKey,
    kCVImageBufferYCbCrMatrixKey, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use objc2_video_toolbox::{
    VTCompressionSession, VTEncodeInfoFlags, VTSessionSetProperty,
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_BaseLayerFrameRate, kVTCompressionPropertyKey_DataRateLimits,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, kVTCompressionPropertyKey_ProfileLevel,
    kVTCompressionPropertyKey_RealTime, kVTProfileLevel_HEVC_Main10_AutoLevel,
};

// Encoded samples from the compressor. The VT callback runs on its own
// thread. We drain after CompleteFrames returns.
struct SampleSink {
    samples: Mutex<Vec<CFRetained<CMSampleBuffer>>>,
    first_error: Mutex<i32>,
}

unsafe extern "C-unwind" fn output_callback(
    refcon: *mut c_void,
    _src_refcon: *mut c_void,
    status: i32,
    _flags: VTEncodeInfoFlags,
    sample: *mut CMSampleBuffer,
) {
    if refcon.is_null() {
        return;
    }
    let sink = unsafe { &*(refcon as *const SampleSink) };
    if status != 0 {
        let mut fe = sink.first_error.lock().unwrap();
        if *fe == 0 {
            *fe = status;
        }
        return;
    }
    if let Some(nn) = NonNull::new(sample) {
        // VT releases its reference after the callback returns. Retain so
        // the writer loop can append the sample later.
        let retained: CFRetained<CMSampleBuffer> = unsafe { CFRetained::retain(nn) };
        sink.samples.lock().unwrap().push(retained);
    }
}

fn cfnum_i32(v: i32) -> CFRetained<CFNumber> {
    unsafe {
        CFNumber::new(
            None,
            CFNumberType::SInt32Type,
            &v as *const i32 as *const c_void,
        )
        .expect("CFNumberCreate i32")
    }
}

fn cfnum_f64(v: f64) -> CFRetained<CFNumber> {
    unsafe {
        CFNumber::new(
            None,
            CFNumberType::Float64Type,
            &v as *const f64 as *const c_void,
        )
        .expect("CFNumberCreate f64")
    }
}

fn vt_set(session: &CFType, key: &CFString, value: &CFType, name: &str) -> Result<()> {
    let st = unsafe { VTSessionSetProperty(session, key, Some(value)) };
    if st != 0 {
        bail!("VTSessionSetProperty({name}) failed: OSStatus {st}");
    }
    Ok(())
}

pub fn transcode(input: &Path, output: &Path) -> Result<()> {
    // AVAssetWriter refuses to overwrite an existing file.
    let _ = std::fs::remove_file(output);
    if let Some(parent) = output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let input_ns = NSString::from_str(&input.to_string_lossy());
    let url_in = NSURL::fileURLWithPath(&input_ns);
    let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url_in, None) };

    let media_type =
        unsafe { AVMediaTypeVideo }.context("AVMediaTypeVideo constant unavailable")?;
    // Sync API. Async variant needs `block2`.
    #[allow(deprecated)]
    let tracks = unsafe { asset.tracksWithMediaType(media_type) };
    let track = tracks.firstObject().context("no video track in input")?;
    let size = unsafe { track.naturalSize() };
    let width = size.width as i32;
    let height = size.height as i32;
    log::debug!("source: {width}x{height}");

    let reader = unsafe { AVAssetReader::assetReaderWithAsset_error(&asset) }
        .map_err(|e| anyhow!("AVAssetReader init failed: {e:?}"))?;

    let settings = pixel_format_settings();
    let reader_output = unsafe {
        AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(
            &track,
            Some(&settings),
        )
    };
    unsafe { reader.addOutput(&reader_output) };

    let sink: Box<SampleSink> = Box::new(SampleSink {
        samples: Mutex::new(Vec::new()),
        first_error: Mutex::new(0),
    });
    let sink_ptr = (&*sink as *const SampleSink) as *mut c_void;

    let mut raw_session: *mut VTCompressionSession = ptr::null_mut();
    let create_st = unsafe {
        VTCompressionSession::create(
            None,
            width,
            height,
            kCMVideoCodecType_HEVC,
            None,
            None,
            None,
            Some(output_callback),
            sink_ptr,
            NonNull::new(&mut raw_session as *mut _).unwrap(),
        )
    };
    if create_st != 0 || raw_session.is_null() {
        bail!("VTCompressionSessionCreate failed: OSStatus {create_st}");
    }
    let session: CFRetained<VTCompressionSession> =
        unsafe { CFRetained::from_raw(NonNull::new(raw_session).unwrap()) };
    let s: &CFType = &session;

    unsafe {
        let profile_level: &CFType = kVTProfileLevel_HEVC_Main10_AutoLevel;
        vt_set(
            s,
            kVTCompressionPropertyKey_ProfileLevel,
            profile_level,
            "ProfileLevel",
        )?;

        let avg = cfnum_i32(9_500_000);
        vt_set(
            s,
            kVTCompressionPropertyKey_AverageBitRate,
            &avg,
            "AverageBitRate",
        )?;

        let limit_bytes = cfnum_i32(1_500_000);
        let limit_secs = cfnum_i32(1);
        let bytes_ref: &CFType = &limit_bytes;
        let secs_ref: &CFType = &limit_secs;
        let mut items: [*const c_void; 2] = [
            bytes_ref as *const CFType as *const c_void,
            secs_ref as *const CFType as *const c_void,
        ];
        let limits = CFArray::new(
            None,
            items.as_mut_ptr(),
            2,
            &kCFTypeArrayCallBacks as *const CFArrayCallBacks,
        )
        .context("CFArrayCreate(DataRateLimits)")?;
        vt_set(
            s,
            kVTCompressionPropertyKey_DataRateLimits,
            &limits,
            "DataRateLimits",
        )?;

        let fps = cfnum_i32(60);
        vt_set(
            s,
            kVTCompressionPropertyKey_ExpectedFrameRate,
            &fps,
            "ExpectedFrameRate",
        )?;

        let kfi = cfnum_i32(60);
        vt_set(
            s,
            kVTCompressionPropertyKey_MaxKeyFrameInterval,
            &kfi,
            "MaxKeyFrameInterval",
        )?;

        let kfid = cfnum_f64(1.0);
        vt_set(
            s,
            kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
            &kfid,
            "MaxKeyFrameIntervalDuration",
        )?;

        let rt: &CFType = kCFBooleanFalse.context("kCFBooleanFalse")?;
        vt_set(s, kVTCompressionPropertyKey_RealTime, rt, "RealTime")?;
        let afr: &CFType = kCFBooleanFalse.context("kCFBooleanFalse")?;
        vt_set(
            s,
            kVTCompressionPropertyKey_AllowFrameReordering,
            afr,
            "AllowFrameReordering",
        )?;

        // Probe the modern single-key form first. HEVCTemporalSubLayerAccess
        // isn't in the public SDK headers and is rejected on most hardware,
        // in which case fall back to the explicit NumberOfTemporalLayers and
        // BaseLayerFrameRate pair. Both paths produce the same VPS.
        let tsla_key = CFString::from_str("HEVCTemporalSubLayerAccess");
        let tsla_val: &CFType = kCFBooleanTrue.context("kCFBooleanTrue")?;
        let tsla_st = VTSessionSetProperty(s, &tsla_key, Some(tsla_val));
        if tsla_st == 0 {
            log::debug!("temporal sub-layers: HEVCTemporalSubLayerAccess");
        } else {
            log::debug!(
                "temporal sub-layers: NumberOfTemporalLayers + BaseLayerFrameRate \
                 (TSLA returned {tsla_st})"
            );
            let ntl_key = CFString::from_str("NumberOfTemporalLayers");
            let ntl_val = cfnum_i32(2);
            let ntl_st = VTSessionSetProperty(s, &ntl_key, Some(&ntl_val));
            if ntl_st != 0 {
                bail!("NumberOfTemporalLayers failed: OSStatus {ntl_st}");
            }
            let blfr = cfnum_f64(30.0);
            vt_set(
                s,
                kVTCompressionPropertyKey_BaseLayerFrameRate,
                &blfr,
                "BaseLayerFrameRate",
            )?;
        }
    }

    if !unsafe { reader.startReading() } {
        let err = unsafe { reader.error() };
        bail!("AVAssetReader startReading failed: {err:?}");
    }

    let mut frames_in: usize = 0;
    while unsafe { reader.status() } == AVAssetReaderStatus::Reading {
        let Some(sample) = (unsafe { reader_output.copyNextSampleBuffer() }) else {
            break;
        };
        let Some(pb) = (unsafe { sample.image_buffer() }) else {
            continue;
        };
        // Strip color attachments. The encoder skips the VUI color
        // description when these are absent.
        let cv: &CVImageBuffer = &pb;
        unsafe {
            cv.remove_attachment(kCVImageBufferColorPrimariesKey);
            cv.remove_attachment(kCVImageBufferTransferFunctionKey);
            cv.remove_attachment(kCVImageBufferYCbCrMatrixKey);
        }

        let pts = unsafe { sample.presentation_time_stamp() };
        let dur = unsafe { sample.duration() };
        let mut info_flags = VTEncodeInfoFlags::empty();
        let enc_st =
            unsafe { session.encode_frame(&pb, pts, dur, None, ptr::null_mut(), &mut info_flags) };
        if enc_st != 0 {
            bail!("VTCompressionSessionEncodeFrame failed at frame {frames_in}: OSStatus {enc_st}");
        }
        frames_in += 1;
        if frames_in.is_multiple_of(60) {
            log::debug!("encoding... {frames_in} frames");
        }
    }

    if unsafe { reader.status() } == AVAssetReaderStatus::Failed {
        let err = unsafe { reader.error() };
        bail!("AVAssetReader failed: {err:?}");
    }

    let complete_st = unsafe { session.complete_frames(kCMTimeInvalid) };
    if complete_st != 0 {
        bail!("VTCompressionSessionCompleteFrames failed: OSStatus {complete_st}");
    }
    unsafe { session.invalidate() };

    {
        let fe = *sink.first_error.lock().unwrap();
        if fe != 0 {
            bail!("encoder produced OSStatus {fe}");
        }
    }
    let samples: Vec<CFRetained<CMSampleBuffer>> =
        std::mem::take(&mut *sink.samples.lock().unwrap());
    if samples.is_empty() {
        bail!("encoder produced no samples");
    }
    log::info!("encoded {frames_in} frames");

    let first_format_desc = unsafe { samples[0].format_description() }
        .context("first encoded sample has no format description")?;

    let output_ns = NSString::from_str(&output.to_string_lossy());
    let url_out = NSURL::fileURLWithPath(&output_ns);
    let file_type =
        unsafe { AVFileTypeQuickTimeMovie }.context("AVFileTypeQuickTimeMovie unavailable")?;
    let writer = unsafe { AVAssetWriter::assetWriterWithURL_fileType_error(&url_out, file_type) }
        .map_err(|e| anyhow!("AVAssetWriter init failed: {e:?}"))?;

    let writer_input = unsafe {
        AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings_sourceFormatHint(
            media_type,
            None,
            Some(&first_format_desc),
        )
    };
    unsafe { writer_input.setExpectsMediaDataInRealTime(false) };
    unsafe { writer.addInput(&writer_input) };

    if !unsafe { writer.startWriting() } {
        let err = unsafe { writer.error() };
        bail!("AVAssetWriter startWriting failed: {err:?}");
    }
    let first_pts = unsafe { samples[0].presentation_time_stamp() };
    unsafe { writer.startSessionAtSourceTime(first_pts) };

    let mut idx = 0usize;
    while idx < samples.len() {
        if unsafe { writer_input.isReadyForMoreMediaData() } {
            if !unsafe { writer_input.appendSampleBuffer(&samples[idx]) } {
                let err = unsafe { writer.error() };
                bail!("AVAssetWriterInput.appendSampleBuffer at {idx} failed: {err:?}");
            }
            idx += 1;
        } else {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    unsafe { writer_input.markAsFinished() };

    // finishWriting blocks until the moov atom is flushed. On the main
    // thread AVFoundation logs a warning. The async variant needs a block,
    // which would pull in `block2`. Run sync on a worker.
    //
    // AVAssetWriter isn't declared Send by objc2-av-foundation. The newtype
    // asserts Send for the move, and the consuming finish() method makes
    // the closure capture the whole wrapper instead of the inner Retained.
    struct SendWriter(Retained<AVAssetWriter>);
    // SAFETY: ownership transfers to the worker. No concurrent access.
    unsafe impl Send for SendWriter {}
    impl SendWriter {
        fn finish(self) -> (bool, AVAssetWriterStatus, Option<String>) {
            #[allow(deprecated)]
            let ok = unsafe { self.0.finishWriting() };
            let err = unsafe { self.0.error() };
            let status = unsafe { self.0.status() };
            (ok, status, err.map(|e| format!("{e:?}")))
        }
    }

    let sw = SendWriter(writer);
    let join_handle = std::thread::spawn(move || sw.finish());
    let (finished, status, err_str) = join_handle
        .join()
        .map_err(|e| anyhow!("finishWriting worker panicked: {e:?}"))?;
    if !finished {
        bail!("AVAssetWriter finishWriting failed: {err_str:?}");
    }
    if status == AVAssetWriterStatus::Failed {
        bail!("AVAssetWriter finished with Failed status: {err_str:?}");
    }

    log::info!("wrote {idx} samples → {}", output.display());
    drop(sink); // outlives all VT callbacks
    Ok(())
}

// Reader settings asking for 8-bit NV12. VT converts internally. Output
// profile is set by ProfileLevel above, not by the input pixel format.
fn pixel_format_settings() -> Retained<NSDictionary<NSString, AnyObject>> {
    let key_cf: &'static CFString = unsafe { kCVPixelBufferPixelFormatTypeKey };
    let key_ns: &NSString = unsafe { &*(key_cf as *const CFString as *const NSString) };

    let val = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
    let val_obj: &AnyObject = AsRef::<AnyObject>::as_ref(&*val);
    NSDictionary::<NSString, AnyObject>::from_slices(&[key_ns], &[val_obj])
}
