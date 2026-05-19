//! HEVC Main10 transcoder for live lock-screen video.
//!
//! Drives `VTCompressionSession` directly because `AVAssetWriter`'s
//! `outputSettings` dictionary does not expose the temporal sub-layer
//! property keys. The encoded `CMSampleBuffer`s are then appended to an
//! `AVAssetWriter` configured for pass-through.

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
    CFNumber, CFNumberType, CFRetained, CFString, CFType, kCFBooleanFalse,
};
use objc2_core_media::{CMSampleBuffer, kCMTimeInvalid, kCMVideoCodecType_HEVC};
use objc2_core_video::{
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use objc2_video_toolbox::{
    VTCompressionSession, VTEncodeInfoFlags, VTSessionSetProperty,
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_BaseLayerFrameRate, kVTCompressionPropertyKey_ExpectedFrameRate,
    kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, kVTCompressionPropertyKey_ProfileLevel,
    kVTCompressionPropertyKey_RealTime, kVTProfileLevel_HEVC_Main10_AutoLevel,
};

const AVERAGE_BITRATE_BPS: i32 = 15_000_000;
const EXPECTED_FPS: i32 = 60;
const MAX_KEYFRAME_INTERVAL: i32 = 120;
const MAX_KEYFRAME_INTERVAL_SECS: f64 = 2.0;
const TEMPORAL_LAYER_COUNT: i32 = 2;
const BASE_LAYER_FPS: f64 = (EXPECTED_FPS as f64) / 2.0;

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
        // VT drops its reference once we return; retain for the writer pass.
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

fn open_input(
    input: &Path,
) -> Result<(
    Retained<AVAssetReader>,
    Retained<AVAssetReaderTrackOutput>,
    i32,
    i32,
)> {
    let input_ns = NSString::from_str(&input.to_string_lossy());
    let url_in = NSURL::fileURLWithPath(&input_ns);
    let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url_in, None) };

    let media_type =
        unsafe { AVMediaTypeVideo }.context("AVMediaTypeVideo constant unavailable")?;
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

    Ok((reader, reader_output, width, height))
}

fn create_session(
    width: i32,
    height: i32,
    sink_ptr: *mut c_void,
) -> Result<CFRetained<VTCompressionSession>> {
    let mut raw: *mut VTCompressionSession = ptr::null_mut();
    let st = unsafe {
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
            NonNull::new(&mut raw as *mut _).unwrap(),
        )
    };
    if st != 0 || raw.is_null() {
        bail!("VTCompressionSessionCreate failed: OSStatus {st}");
    }
    let session = unsafe { CFRetained::from_raw(NonNull::new(raw).unwrap()) };
    configure_session(&session)?;
    Ok(session)
}

fn configure_session(session: &VTCompressionSession) -> Result<()> {
    let s: &CFType = session;
    unsafe {
        let profile_level: &CFType = kVTProfileLevel_HEVC_Main10_AutoLevel;
        vt_set(
            s,
            kVTCompressionPropertyKey_ProfileLevel,
            profile_level,
            "ProfileLevel",
        )?;

        let avg = cfnum_i32(AVERAGE_BITRATE_BPS);
        vt_set(
            s,
            kVTCompressionPropertyKey_AverageBitRate,
            &avg,
            "AverageBitRate",
        )?;

        let fps = cfnum_i32(EXPECTED_FPS);
        vt_set(
            s,
            kVTCompressionPropertyKey_ExpectedFrameRate,
            &fps,
            "ExpectedFrameRate",
        )?;

        let kfi = cfnum_i32(MAX_KEYFRAME_INTERVAL);
        vt_set(
            s,
            kVTCompressionPropertyKey_MaxKeyFrameInterval,
            &kfi,
            "MaxKeyFrameInterval",
        )?;

        let kfid = cfnum_f64(MAX_KEYFRAME_INTERVAL_SECS);
        vt_set(
            s,
            kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
            &kfid,
            "MaxKeyFrameIntervalDuration",
        )?;

        let false_val: &CFType = kCFBooleanFalse.context("kCFBooleanFalse")?;
        vt_set(s, kVTCompressionPropertyKey_RealTime, false_val, "RealTime")?;
        vt_set(
            s,
            kVTCompressionPropertyKey_AllowFrameReordering,
            false_val,
            "AllowFrameReordering",
        )?;

        // Two temporal sub-layers in the VPS (`vps_max_sub_layers_minus1 = 1`).
        // The lock-screen player needs this shape to re-arm across lock cycles.
        let ntl_key = CFString::from_str("NumberOfTemporalLayers");
        let ntl_val = cfnum_i32(TEMPORAL_LAYER_COUNT);
        let st = VTSessionSetProperty(s, &ntl_key, Some(&ntl_val));
        if st != 0 {
            bail!("NumberOfTemporalLayers failed: OSStatus {st}");
        }
        let blfr = cfnum_f64(BASE_LAYER_FPS);
        vt_set(
            s,
            kVTCompressionPropertyKey_BaseLayerFrameRate,
            &blfr,
            "BaseLayerFrameRate",
        )?;
    }
    Ok(())
}

fn encode_all(
    reader: &AVAssetReader,
    reader_output: &AVAssetReaderTrackOutput,
    session: &VTCompressionSession,
) -> Result<usize> {
    if !unsafe { reader.startReading() } {
        let err = unsafe { reader.error() };
        bail!("AVAssetReader startReading failed: {err:?}");
    }

    let mut count: usize = 0;
    while unsafe { reader.status() } == AVAssetReaderStatus::Reading {
        let Some(sample) = (unsafe { reader_output.copyNextSampleBuffer() }) else {
            break;
        };
        let Some(pb) = (unsafe { sample.image_buffer() }) else {
            continue;
        };
        let pts = unsafe { sample.presentation_time_stamp() };
        let dur = unsafe { sample.duration() };
        let mut flags = VTEncodeInfoFlags::empty();
        let st = unsafe { session.encode_frame(&pb, pts, dur, None, ptr::null_mut(), &mut flags) };
        if st != 0 {
            bail!("VTCompressionSessionEncodeFrame failed at frame {count}: OSStatus {st}");
        }
        count += 1;
        if count.is_multiple_of(60) {
            log::debug!("encoding... {count} frames");
        }
    }

    if unsafe { reader.status() } == AVAssetReaderStatus::Failed {
        let err = unsafe { reader.error() };
        bail!("AVAssetReader failed: {err:?}");
    }

    let st = unsafe { session.complete_frames(kCMTimeInvalid) };
    if st != 0 {
        bail!("VTCompressionSessionCompleteFrames failed: OSStatus {st}");
    }
    Ok(count)
}

fn write_samples(output: &Path, samples: &[CFRetained<CMSampleBuffer>]) -> Result<usize> {
    let media_type =
        unsafe { AVMediaTypeVideo }.context("AVMediaTypeVideo constant unavailable")?;
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

    finish_writing(writer)?;
    Ok(idx)
}

// finishWriting blocks until the moov atom is flushed. On the main thread
// AVFoundation logs a warning, and the async variant requires `block2`. Run
// the sync call on a worker. AVAssetWriter is not declared `Send` by
// objc2-av-foundation
fn finish_writing(writer: Retained<AVAssetWriter>) -> Result<()> {
    struct SendWriter(Retained<AVAssetWriter>);
    // SAFETY: ownership transfers to the worker; no concurrent access.
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

    // `sw.finish()` consumes the whole wrapper, so the closure captures
    // `sw: SendWriter` (Send) rather than the inner `Retained` field (not Send).
    let sw = SendWriter(writer);
    let (finished, status, err_str) = std::thread::spawn(move || sw.finish())
        .join()
        .map_err(|e| anyhow!("finishWriting worker panicked: {e:?}"))?;
    if !finished {
        bail!("AVAssetWriter finishWriting failed: {err_str:?}");
    }
    if status == AVAssetWriterStatus::Failed {
        bail!("AVAssetWriter finished with Failed status: {err_str:?}");
    }
    Ok(())
}

pub fn transcode(input: &Path, output: &Path) -> Result<()> {
    // AVAssetWriter refuses to overwrite an existing file.
    let _ = std::fs::remove_file(output);
    if let Some(parent) = output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let (reader, reader_output, width, height) = open_input(input)?;

    let sink: Box<SampleSink> = Box::new(SampleSink {
        samples: Mutex::new(Vec::new()),
        first_error: Mutex::new(0),
    });
    let sink_ptr = (&*sink as *const SampleSink) as *mut c_void;

    let session = create_session(width, height, sink_ptr)?;
    let frames_in = encode_all(&reader, &reader_output, &session)?;
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

    let written = write_samples(output, &samples)?;
    log::info!("wrote {written} samples → {}", output.display());

    drop(sink); // outlives all VT callbacks
    Ok(())
}

// Asks the reader for 8-bit NV12 frames; VT converts internally. The output
// profile is governed by `ProfileLevel`, not the input pixel format.
fn pixel_format_settings() -> Retained<NSDictionary<NSString, AnyObject>> {
    let key_cf: &'static CFString = unsafe { kCVPixelBufferPixelFormatTypeKey };
    let key_ns: &NSString = unsafe { &*(key_cf as *const CFString as *const NSString) };

    let val = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
    let val_obj: &AnyObject = AsRef::<AnyObject>::as_ref(&*val);
    NSDictionary::<NSString, AnyObject>::from_slices(&[key_ns], &[val_obj])
}
