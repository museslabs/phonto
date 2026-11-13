//! The video channels of a procedural wallpaper. A thread decodes each video
//! with an `AVAssetReader` and gives its frames to a [`VideoQueue`]. The
//! playback clock of the wallpaper then sets the speed. A frame stays in its
//! IOSurface. Metal reads it through a `CVMetalTextureCache`, with no copy on
//! the CPU.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use objc2::Message;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_av_foundation::{
    AVAsset, AVAssetReader, AVAssetReaderStatus, AVAssetReaderTrackOutput, AVAssetTrack,
    AVMediaTypeVideo, AVURLAsset, NSValueAVFoundationExtensions,
};
use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_media::{CMTime, CMTimeRange, kCMTimeZero};
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer,
    CVPixelBufferGetHeight, CVPixelBufferGetWidth, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelFormatType_32BGRA, kCVReturnSuccess,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString, NSURL, NSValue};
use objc2_metal::{MTLCommandBuffer, MTLCommandBufferStatus, MTLDevice, MTLPixelFormat};

use super::procedural::GpuTexture;
use crate::procedural::{Decoded, VideoQueue};

type CommandBuffer = ProtocolObject<dyn MTLCommandBuffer>;

/// The longest wait for a video that is behind, as a part of the time since
/// the last frame of the wallpaper. It is never more than `MAX_CATCH_UP_WAIT`
/// seconds. These are the values of the Wayland backend.
const CATCH_UP_SHARE: f64 = 0.25;
const MAX_CATCH_UP_WAIT: f64 = 0.025;

/// The most frames that wait for their copy. While the GPU is that far
/// behind, new frames do not show. The decoder would run out of buffers
/// otherwise.
const MAX_COPYING: usize = 3;

/// A video that a channel of a procedural wallpaper shows. It plays at the
/// speed of the playback clock of the wallpaper. It stops while time stops.
pub struct VideoChannel {
    queue: Arc<VideoQueue<Frame>>,
    cache: CFRetained<CVMetalTextureCache>,
    /// Frames that the GPU still copies, with the command buffer that copies
    /// them, oldest first.
    copying: VecDeque<(MetalFrame, Retained<CommandBuffer>)>,
    /// True while more copies wait than `MAX_COPYING`.
    stalled: bool,
}

/// A decoded frame, as the decoder thread gives it.
struct Frame(CFRetained<CVPixelBuffer>);

// SAFETY: A pixel buffer is a reference-counted CF object, which any thread
// may retain and release. The decoder doesn't touch it after it gives it away.
unsafe impl Send for Frame {}

/// A decoded frame that Metal can read. The texture reads the memory of the
/// pixel buffer. Both stay until the GPU has read them.
pub struct MetalFrame {
    texture: GpuTexture,
    _metal: CFRetained<CVMetalTexture>,
    _frame: Frame,
}

impl MetalFrame {
    pub fn texture(&self) -> &GpuTexture {
        &self.texture
    }
}

impl VideoChannel {
    /// Opens the video at `path`, and starts to decode it on its own thread.
    /// Fails if AVFoundation cannot read it.
    pub fn start(path: &Path, device: &ProtocolObject<dyn MTLDevice>) -> anyhow::Result<Self> {
        let mut cache = std::ptr::null_mut();
        // SAFETY: `cache` is a valid place for the new cache.
        let status = unsafe {
            CVMetalTextureCache::create(None, None, device, None, NonNull::from(&mut cache))
        };
        let cache = NonNull::new(cache)
            .filter(|_| status == kCVReturnSuccess)
            .with_context(|| format!("create a Metal texture cache (CVReturn {status})"))?;
        // SAFETY: The cache was created with a +1 retain count.
        let cache = unsafe { CFRetained::from_raw(cache) };

        let queue = Arc::new(VideoQueue::new());
        let (opened, open_result) = mpsc::sync_channel(1);
        let decoder_queue = Arc::clone(&queue);
        let decoder_path = path.to_path_buf();
        std::thread::Builder::new()
            .name("video-channel".to_string())
            .spawn(move || decode(decoder_path, decoder_queue, opened))
            .context("start the thread of a video channel")?;
        open_result
            .recv()
            .context("the video thread stopped")?
            .with_context(|| format!("video {}", path.display()))?;
        Ok(Self {
            queue,
            cache,
            copying: VecDeque::new(),
            stalled: false,
        })
    }

    /// Moves the video `delta` seconds on, and returns the newest frame that
    /// is due, if a new one is. If the decoder is behind, this waits for it at
    /// most `wait`.
    pub fn advance(&mut self, delta: f64, wait: Duration) -> anyhow::Result<Option<MetalFrame>> {
        let done = self
            .copying
            .iter()
            .take_while(|(_, cmd)| {
                matches!(
                    cmd.status(),
                    MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error
                )
            })
            .count();
        self.copying.drain(..done);
        let Some(frame) = self.queue.advance(delta, wait) else {
            return Ok(None);
        };
        let stalled = self.copying.len() >= MAX_COPYING;
        if stalled && !self.stalled {
            log::warn!("video channel: the GPU did not finish a copy in time");
        }
        self.stalled = stalled;
        // The GPU never read this frame. It can go back to the decoder.
        if stalled {
            return Ok(None);
        }
        let (width, height) = (
            CVPixelBufferGetWidth(&frame.0),
            CVPixelBufferGetHeight(&frame.0),
        );
        let mut metal = std::ptr::null_mut();
        // SAFETY: The decoder makes BGRA buffers, which are IOSurfaces. The
        // texture is their size. `metal` is a valid place for the texture.
        let status = unsafe {
            CVMetalTextureCache::create_texture_from_image(
                None,
                &self.cache,
                &frame.0,
                None,
                MTLPixelFormat::BGRA8Unorm,
                width,
                height,
                0,
                NonNull::from(&mut metal),
            )
        };
        let metal = NonNull::new(metal)
            .filter(|_| status == kCVReturnSuccess)
            .with_context(|| format!("map a video frame to Metal (CVReturn {status})"))?;
        // SAFETY: The texture was created with a +1 retain count.
        let metal = unsafe { CFRetained::from_raw(metal) };
        let texture = CVMetalTextureGetTexture(&metal).context("a video frame has no texture")?;
        // The cache recycles textures that nothing uses any more.
        self.cache.flush(0);
        Ok(Some(MetalFrame {
            texture,
            _metal: metal,
            _frame: frame,
        }))
    }

    /// Holds `frame` until `cmd`, which copies it, has completed.
    pub fn hold(&mut self, frame: MetalFrame, cmd: &CommandBuffer) {
        self.copying.push_back((frame, cmd.retain()));
    }
}

/// How long a wallpaper on screen waits for a video that is behind, after
/// `delta` seconds of playback.
pub fn catch_up_wait(delta: f64) -> Duration {
    Duration::from_secs_f64((delta * CATCH_UP_SHARE).min(MAX_CATCH_UP_WAIT))
}

impl Drop for VideoChannel {
    fn drop(&mut self) {
        // The decoder can wait to give a frame. Closing the queue ends the
        // wait, and the decoder stops.
        self.queue.close();
    }
}

/// Decodes the video at `path` into `queue`, from its start again at its
/// end. `opened` gets the result of opening it.
fn decode(
    path: PathBuf,
    queue: Arc<VideoQueue<Frame>>,
    opened: mpsc::SyncSender<anyhow::Result<()>>,
) {
    let (reader, output, range) = match autoreleasepool(|_| open(&path)) {
        Ok(reader) => {
            let _ = opened.send(Ok(()));
            reader
        }
        Err(err) => {
            let _ = opened.send(Err(err));
            return;
        }
    };
    let mut frames = 0;
    loop {
        let more = autoreleasepool(|_| {
            // SAFETY: The output belongs to the reader, which started.
            let Some(sample) = (unsafe { output.copyNextSampleBuffer() }) else {
                // With random access, the reader stays in `Reading` at the
                // end of its range. Any other status means that it failed.
                if frames == 0 || unsafe { reader.status() } != AVAssetReaderStatus::Reading {
                    let reason = unsafe { reader.error() }
                        .map_or_else(|| "it has no frames".to_string(), |e| describe(&e));
                    log::warn!("video {} stopped: {reason}", path.display());
                    queue.end();
                    return false;
                }
                frames = 0;
                // The queue must know of the new loop before its first frame.
                queue.start_loop();
                let ranges = NSArray::from_retained_slice(&[unsafe {
                    NSValue::valueWithCMTimeRange(range)
                }]);
                // SAFETY: The output supports random access, and it read all
                // of its range without a failure.
                unsafe { output.resetForReadingTimeRanges(&ranges) };
                return true;
            };
            // A sample without an image only marks something in the stream.
            let Some(image) = (unsafe { sample.image_buffer() }) else {
                return true;
            };
            frames += 1;
            let decoded = Decoded {
                frame: Frame(image),
                time: seconds(unsafe { sample.presentation_time_stamp() }).unwrap_or(0.0),
                length: seconds(unsafe { sample.duration() }).filter(|length| *length > 0.0),
            };
            queue.push(decoded)
        });
        if !more {
            // SAFETY: Cancelling is allowed in any state.
            unsafe { reader.cancelReading() };
            return;
        }
    }
}

/// A reader of the frames of the video at `path`, as BGRA, that can read
/// them again, and the time range of the video.
fn open(
    path: &Path,
) -> anyhow::Result<(
    Retained<AVAssetReader>,
    Retained<AVAssetReaderTrackOutput>,
    CMTimeRange,
)> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) };
    let reader = unsafe { AVAssetReader::assetReaderWithAsset_error(&asset) }
        .map_err(|e| anyhow!("AVFoundation cannot read it: {}", describe(&e)))?;
    let track = video_track(&asset)?;
    let output = bgra_output(&track);
    // phonto does not change the frames. The reader needs no copy of them.
    unsafe { output.setAlwaysCopiesSampleData(false) };
    // A reader cannot start again, but an output with random access can read
    // its range again. That costs about 3 ms, and a new reader about 10 ms.
    unsafe { output.setSupportsRandomAccess(true) };
    // Without a range, random access reads one frame past the end of some
    // videos.
    let range = unsafe { CMTimeRange::new(kCMTimeZero, asset.duration()) };
    unsafe {
        reader.setTimeRange(range);
        reader.addOutput(&output);
    }
    if !unsafe { reader.startReading() } {
        let reason =
            unsafe { reader.error() }.map_or_else(|| "unknown".to_string(), |e| describe(&e));
        bail!("AVFoundation cannot read it: {reason}");
    }
    Ok((reader, output, range))
}

/// The first video track of `asset`.
pub fn video_track(asset: &AVAsset) -> anyhow::Result<Retained<AVAssetTrack>> {
    let video = unsafe { AVMediaTypeVideo }.context("AVMediaTypeVideo constant unavailable")?;
    unsafe { asset.tracks() }
        .iter()
        .find(|t| &*unsafe { t.mediaType() } == video)
        .context("no video track in asset")
}

/// An output that decodes `track` to BGRA pixel buffers. They are IOSurfaces,
/// which Metal can read.
pub fn bgra_output(track: &AVAssetTrack) -> Retained<AVAssetReaderTrackOutput> {
    let key_cf: &'static CFString = unsafe { kCVPixelBufferPixelFormatTypeKey };
    let key_ns: &NSString = unsafe { &*(key_cf as *const CFString as *const NSString) };
    let val = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
    let val_obj: &AnyObject = &val;
    let settings = NSDictionary::<NSString, AnyObject>::from_slices(&[key_ns], &[val_obj]);
    unsafe {
        AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(
            track,
            Some(&settings),
        )
    }
}

/// `time` in seconds, if it is a number.
fn seconds(time: CMTime) -> Option<f64> {
    Some(unsafe { time.seconds() }).filter(|s| s.is_finite())
}

/// What AVFoundation says about an error, with its reason if it gives one.
fn describe(error: &NSError) -> String {
    let description = error.localizedDescription();
    match error.localizedFailureReason() {
        Some(reason) => format!("{description} ({reason})"),
        None => description.to_string(),
    }
}
