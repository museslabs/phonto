// Sets a video as the macOS desktop + lock-screen wallpaper by injecting it
// into Apple's aerial catalog. The system's `WallpaperAerialsExtension`
// (Apple-signed, holds the private `com.apple.private.wallpaper.extension`
// entitlement) becomes our playback host and gives us native lock-screen
// rendering for free.
//
// Two pieces are critical and absent from prior iterations of this tool:
//
//   1. Transcode to HEVC `.mov` via VideoToolbox before injection. Raw user
//      MP4s play on the first lock cycle then go black after the first lid
//      sleep/wake — the aerial extension's AVPlayer state collapses across
//      the cycle when the asset is in an unexpected container/codec.
//      HEVC-in-mov plays reliably across many lock/unlock cycles. (Same
//      stance Wallper's published architecture takes for the same reason.)
//
//   2. Restart `WallpaperAerialsExtension` specifically. Killing
//      `WallpaperAgent` (what we did before) doesn't force the extension's
//      in-memory copy of `entries.json` to reload, so the new asset never
//      appears in the picker. (Confirmed via reverse-engineering Wallspace
//      — its binary literally contains the string "Restarted
//      WallpaperAerialsExtension".)

use std::ffi::c_void;
use std::fs;
use std::io::{self, Write};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use block2::RcBlock;
use clap::Parser;
use objc2::class;
use objc2::encode::{Encode, Encoding};
use objc2::exception::catch;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSError, NSSize, NSString, NSURL};
use serde_json::{Value, json};
use uuid::Uuid;

// `class!(AVURLAsset)` and friends only resolve at runtime if AVFoundation is
// actually loaded — and that only happens if the linker keeps the framework's
// link directive alive. Pulling in objc2-av-foundation as a dep brings the
// directive in *its* crate, but if we don't reference any symbol from that
// crate the linker dead-strips it. Referencing these extern statics keeps
// the link alive AND gives us the canonical NSString constants we need for
// AVAssetReader/Writer output settings (where passing the wrong key string
// throws an Objective-C exception that propagates up as `fatal runtime error:
// Rust cannot catch foreign exceptions`).
#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    static AVMediaTypeVideo: *const NSString;
    static AVVideoCodecKey: *const NSString;
    static AVVideoCodecHEVC: *const NSString;
    static AVVideoWidthKey: *const NSString;
    static AVVideoHeightKey: *const NSString;
    static AVVideoCompressionPropertiesKey: *const NSString;
    static AVVideoProfileLevelKey: *const NSString;
    static AVFileTypeQuickTimeMovie: *const NSString;
    // `AVVideoProfileLevelHEVCMain10_AutoLevel` is documented but doesn't
    // appear in the macOS 26 AVFoundation TBD, so we use its literal
    // string value `"HEVC_Main10_AutoLevel"` (AVFoundation key/value
    // comparison is by string contents, not pointer identity).
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: *const NSString;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    fn NSSetUncaughtExceptionHandler(handler: extern "C" fn(*mut AnyObject));
}

// Prints the exception's name and reason before Rust's foreign-exception
// guard aborts the process. `objc2::exception::catch` doesn't intercept
// AVFoundation's exceptions on macOS 26 (they use the C++ unwinding ABI
// and trip Rust's __rust_foreign_exception fence before objc2 sees them),
// so this best-effort handler is how we surface the actual reason.
extern "C" fn objc_exception_diagnostic(exc: *mut AnyObject) {
    if exc.is_null() {
        eprintln!("[!] uncaught NSException: <null>");
        return;
    }
    let name_ptr: *mut NSString = unsafe { msg_send![exc, name] };
    let reason_ptr: *mut NSString = unsafe { msg_send![exc, reason] };
    let render = |p: *mut NSString| -> String {
        if p.is_null() {
            return "<nil>".into();
        }
        match unsafe { Retained::retain(p) } {
            Some(s) => s.to_string(),
            None => "<nil>".into(),
        }
    };
    eprintln!(
        "[!] uncaught NSException: {} — {}",
        render(name_ptr),
        render(reason_ptr)
    );
}

// CMTime is a C struct passed by value. Define minimally — we only need the
// zero value for AVAssetWriter::startSessionAtSourceTime.
#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

// SAFETY: layout matches `<CoreMedia/CMTime.h>` exactly; field encodings are
// the primitive ones; not Drop. msg_send! needs Encode to marshal this by
// value into a startSessionAtSourceTime: call.
unsafe impl Encode for CMTime {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING],
    );
}

const CMTIME_ZERO: CMTime = CMTime {
    value: 0,
    timescale: 1,
    flags: 0,
    epoch: 0,
};

// kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange = FourCC '420v'. The
// 8-bit-per-component 4:2:0 video-range pixel format AVAssetReader will hand
// us decoded frames in.
//
// We deliberately do NOT ask for the 10-bit variant ('x420'): typical user
// inputs are 8-bit H.264 MP4 and AVAssetReader throws when asked to widen
// 8-bit source to 10-bit pixel buffers. Instead we feed the writer 8-bit
// pixels; its Main10 encoder (VideoToolbox) up-converts to a 10-bit HEVC
// bitstream during encode. The aerials player inspects the resulting file's
// `bitsPerComponent` (a property of the bitstream, not the source pixels)
// and sees 10, which is what unblocks the second-lock-black bug.
const PIXEL_FORMAT_420_8BIT_VIDEO_RANGE: u32 = 0x3432_3076; // '420v'

// UUIDv5 namespace — arbitrary, just needs to be stable across runs so the
// same video path always produces the same asset UUID (idempotent
// re-injection).
const NAMESPACE: Uuid = Uuid::from_bytes([
    0x70, 0x68, 0x6f, 0x6e, 0x74, 0x6f, 0x77, 0x70, 0x70, 0x72, 0x6f, 0x6a, 0x65, 0x63, 0x74, 0x21,
]);

// Stable UUID for our own category in `entries.json`. The picker looks up
// `localizedNameKey` ("Phonto") in localized .strings tables; missing keys
// fall through to the raw string, which is how the section ends up labelled
// "Phonto" in System Settings.
const PHONTO_CATEGORY_ID: &str = "8C75F1C2-7E7E-4B5C-9C5C-50484F4E544F";

// Phonto sub-category. Apple's Codable schema for `entries.json` requires
// each asset to reference both a category AND a subcategory by UUID, and
// each category to have a non-empty `subcategories` array. Omitting either
// makes the aerials extension's strict decoder reject the entire catalog,
// at which point System Settings → Wallpaper shows no Aerials section at
// all (not just no Phonto section). Found the hard way.
const PHONTO_SUBCATEGORY_ID: &str = "8C75F1C2-7E7E-4B5C-9C5C-535542434154";

// AVAssetWriterStatus enum values (NSInteger).
const WRITER_STATUS_COMPLETED: isize = 2;
const WRITER_STATUS_FAILED: isize = 3;
const WRITER_STATUS_CANCELLED: isize = 4;

#[derive(Parser)]
#[command(about = "Set a video as the macOS desktop + lock-screen wallpaper")]
struct Args {
    /// Path to the video file (MP4/MOV).
    video: PathBuf,

    /// Display name for the picker (defaults to the file stem).
    #[arg(long)]
    name: Option<String>,

    /// Remove the previously-injected entry for this video and quit.
    #[arg(long)]
    remove: bool,
}

fn main() -> Result<()> {
    unsafe { NSSetUncaughtExceptionHandler(objc_exception_diagnostic) };

    let args = Args::parse();
    let video = args
        .video
        .canonicalize()
        .with_context(|| format!("video not found: {}", args.video.display()))?;

    let home = std::env::var("HOME").context("HOME not set")?;
    let aerials =
        PathBuf::from(&home).join("Library/Application Support/com.apple.wallpaper/aerials");
    let videos_dir = aerials.join("videos");
    let thumbnails_dir = aerials.join("thumbnails");
    let manifest = aerials.join("manifest/entries.json");
    if !manifest.exists() {
        bail!(
            "manifest not found at {} — WallpaperAgent has never initialised it on this Mac",
            manifest.display()
        );
    }

    let asset_id = Uuid::new_v5(&NAMESPACE, video.to_string_lossy().as_bytes())
        .hyphenated()
        .to_string()
        .to_uppercase();

    if args.remove {
        remove_entry(&manifest, &asset_id)?;
        let _ = fs::remove_file(videos_dir.join(format!("{asset_id}.mov")));
        let _ = fs::remove_file(thumbnails_dir.join(format!("{asset_id}.png")));
        restart_aerials();
        println!("removed {asset_id}");
        return Ok(());
    }

    fs::create_dir_all(&videos_dir)?;
    fs::create_dir_all(&thumbnails_dir)?;

    let display_name = args.name.unwrap_or_else(|| {
        video
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Phonto Custom".to_string())
    });

    let target_video = videos_dir.join(format!("{asset_id}.mov"));
    transcode_to_hevc(&video, &target_video)?;

    let target_thumb = thumbnails_dir.join(format!("{asset_id}.png"));
    if let Err(e) = extract_thumbnail(&video, &target_thumb) {
        eprintln!("warning: thumbnail extraction failed ({e}) — picker will show a placeholder");
    } else {
        println!("thumbnail → {}", target_thumb.display());
    }

    upsert_entry(
        &manifest,
        &asset_id,
        &display_name,
        &target_video,
        &target_thumb,
    )?;

    restart_aerials();

    println!();
    println!("Injected '{display_name}' (id {asset_id}).");
    println!("Open System Settings → Wallpaper → scroll to the Phonto section → pick the video.");
    Ok(())
}

fn transcode_to_hevc(input: &Path, output: &Path) -> Result<()> {
    // AVAssetWriter refuses to overwrite — delete any prior output.
    let _ = fs::remove_file(output);

    let input_str = input.to_str().context("input path is not valid UTF-8")?;
    let output_str = output.to_str().context("output path is not valid UTF-8")?;

    print!("  transcoding → HEVC Main10 (10-bit) mov...");
    io::stdout().flush().ok();

    unsafe { run_transcode(input_str, output_str) }?;

    println!(
        "\r  transcoded → {}                                       ",
        output.display()
    );
    Ok(())
}

/// AVAssetReader → AVAssetWriter pipeline that re-encodes the input as
/// HEVC 10-bit (Main10) into a QuickTime `.mov`. This is what the aerials
/// extension's playback path expects for a video that survives lid-sleep /
/// wake cycles. `AVAssetExportSession` with the HEVC preset only emits
/// 8-bit, which plays through the first cycle then renders black on every
/// subsequent lock screen invocation (empirically confirmed).
///
/// Strict obj-c plumbing via `msg_send!` throughout — most of the AVF
/// classes here aren't surfaced as safe wrappers in objc2-av-foundation 0.3.
unsafe fn run_transcode(input_str: &str, output_str: &str) -> Result<()> {
    eprintln!("\n[debug] step 1: build URLs ({input_str})");
    let input_url = unsafe { NSURL::fileURLWithPath(&NSString::from_str(input_str)) };
    let output_url = unsafe { NSURL::fileURLWithPath(&NSString::from_str(output_str)) };

    // ─── 1. Asset + video track ────────────────────────────────────────────
    eprintln!("[debug] step 2: peek extern statics");
    let av_media_video_ptr: *const NSString = AVMediaTypeVideo;
    eprintln!("[debug]   AVMediaTypeVideo ptr = {av_media_video_ptr:p}");
    let cv_pixfmt_key_ptr: *const NSString = kCVPixelBufferPixelFormatTypeKey;
    eprintln!("[debug]   kCVPixelBufferPixelFormatTypeKey ptr = {cv_pixfmt_key_ptr:p}");

    eprintln!("[debug] step 3: AVURLAsset URLAssetWithURL:options:");
    let opts: *const AnyObject = std::ptr::null();
    let asset_ptr: *mut AnyObject = unsafe {
        msg_send![
            class!(AVURLAsset),
            URLAssetWithURL: &*input_url,
            options: opts,
        ]
    };
    eprintln!("[debug]   asset_ptr = {asset_ptr:p}");
    let asset = unsafe { Retained::retain(asset_ptr) }
        .context("AVURLAsset URLAssetWithURL: returned nil")?;

    eprintln!("[debug] step 4: tracksWithMediaType:");
    let video_media_type: &NSString = unsafe { &*AVMediaTypeVideo };
    let tracks_ptr: *mut AnyObject =
        unsafe { msg_send![&*asset, tracksWithMediaType: video_media_type] };
    eprintln!("[debug]   tracks_ptr = {tracks_ptr:p}");
    let tracks = unsafe { Retained::retain(tracks_ptr) }
        .context("tracksWithMediaType: returned nil")?;
    eprintln!("[debug] step 5: firstObject");
    let track_ptr: *mut AnyObject = unsafe { msg_send![&*tracks, firstObject] };
    if track_ptr.is_null() {
        bail!("input has no video track");
    }
    let track =
        unsafe { Retained::retain(track_ptr) }.context("video track retain failed")?;
    eprintln!("[debug] step 6: naturalSize");
    let size: NSSize = unsafe { msg_send![&*track, naturalSize] };
    eprintln!("[debug]   natural size = {} x {}", size.width, size.height);
    let width = size.width as i64;
    let height = size.height as i64;

    // ─── 2. AVAssetReader + 10-bit pixel-format output ─────────────────────
    // alloc returns a +1 retain, untyped. init consumes it; we wrap the init
    // result with from_raw to transfer ownership without bumping the count.
    eprintln!("[debug] step 7: AVAssetReader alloc");
    let reader_alloc: *mut AnyObject =
        unsafe { msg_send![class!(AVAssetReader), alloc] };
    eprintln!("[debug]   reader_alloc = {reader_alloc:p}");
    eprintln!("[debug] step 8: AVAssetReader initWithAsset:error:");
    let mut reader_err: *mut NSError = std::ptr::null_mut();
    let reader_ptr: *mut AnyObject = unsafe {
        msg_send![
            reader_alloc,
            initWithAsset: &*asset,
            error: &mut reader_err,
        ]
    };
    eprintln!("[debug]   reader_ptr = {reader_ptr:p}, err = {reader_err:p}");
    if reader_ptr.is_null() {
        bail!("AVAssetReader init failed: {}", ns_error_msg(reader_err));
    }
    let reader = unsafe { Retained::from_raw(reader_ptr) }
        .context("AVAssetReader retain failed")?;

    let pix_fmt_key: &NSString = unsafe { &*kCVPixelBufferPixelFormatTypeKey };
    let pix_fmt_num_ptr: *mut AnyObject = unsafe {
        msg_send![
            class!(NSNumber),
            numberWithUnsignedInt: PIXEL_FORMAT_420_8BIT_VIDEO_RANGE,
        ]
    };
    let pix_fmt_num = unsafe { Retained::retain(pix_fmt_num_ptr) }
        .context("NSNumber numberWithUnsignedInt: returned nil")?;
    let reader_settings_ptr: *mut AnyObject =
        unsafe { msg_send![class!(NSMutableDictionary), dictionary] };
    let reader_settings = unsafe { Retained::retain(reader_settings_ptr) }
        .context("NSMutableDictionary dictionary returned nil")?;
    unsafe {
        let _: () =
            msg_send![&*reader_settings, setObject: &*pix_fmt_num, forKey: pix_fmt_key];
    }

    eprintln!("[debug] step 9: AVAssetReaderTrackOutput alloc + init");
    let track_output_alloc: *mut AnyObject =
        unsafe { msg_send![class!(AVAssetReaderTrackOutput), alloc] };
    let track_output_ptr: *mut AnyObject = unsafe {
        msg_send![
            track_output_alloc,
            initWithTrack: &*track,
            outputSettings: &*reader_settings,
        ]
    };
    eprintln!("[debug]   track_output_ptr = {track_output_ptr:p}");
    let track_output = unsafe { Retained::from_raw(track_output_ptr) }
        .context("AVAssetReaderTrackOutput init returned nil")?;
    eprintln!("[debug] step 10: canAddOutput / addOutput (in exception-catch)");
    // Wrap in catch so an Objective-C NSException becomes a Rust error with
    // the exception's reason string instead of an immediate process abort.
    let reader_for_catch = &*reader;
    let track_output_for_catch = &*track_output;
    let result = unsafe {
        catch(AssertUnwindSafe(|| {
            let can_add: bool = msg_send![reader_for_catch, canAddOutput: track_output_for_catch];
            if !can_add {
                return Err("canAddOutput returned NO".to_string());
            }
            let _: () = msg_send![reader_for_catch, addOutput: track_output_for_catch];
            Ok(())
        }))
    };
    match result {
        Ok(Ok(())) => {}
        Ok(Err(msg)) => bail!("AVAssetReader: {msg}"),
        Err(exc) => {
            let reason = describe_exception(exc);
            bail!("ObjC exception in canAddOutput/addOutput: {reason}");
        }
    }

    // ─── 3. AVAssetWriter + HEVC Main10 settings ───────────────────────────
    let file_type: &NSString = unsafe { &*AVFileTypeQuickTimeMovie };
    let writer_alloc: *mut AnyObject =
        unsafe { msg_send![class!(AVAssetWriter), alloc] };
    let mut writer_err: *mut NSError = std::ptr::null_mut();
    let writer_ptr: *mut AnyObject = unsafe {
        msg_send![
            writer_alloc,
            initWithURL: &*output_url,
            fileType: file_type,
            error: &mut writer_err,
        ]
    };
    if writer_ptr.is_null() {
        bail!("AVAssetWriter init failed: {}", ns_error_msg(writer_err));
    }
    let writer = unsafe { Retained::from_raw(writer_ptr) }
        .context("AVAssetWriter retain failed")?;

    // Compression sub-dict: AVVideoProfileLevelKey = HEVC Main10 auto level.
    let prof_key: &NSString = unsafe { &*AVVideoProfileLevelKey };
    let prof_val = NSString::from_str("HEVC_Main10_AutoLevel");
    let compression_props_ptr: *mut AnyObject =
        unsafe { msg_send![class!(NSMutableDictionary), dictionary] };
    let compression_props = unsafe { Retained::retain(compression_props_ptr) }
        .context("NSMutableDictionary returned nil (compression)")?;
    unsafe {
        let _: () =
            msg_send![&*compression_props, setObject: &*prof_val, forKey: prof_key];
    }

    // Top-level video settings dict.
    let codec_key: &NSString = unsafe { &*AVVideoCodecKey };
    let codec_val: &NSString = unsafe { &*AVVideoCodecHEVC };
    let width_key: &NSString = unsafe { &*AVVideoWidthKey };
    let height_key: &NSString = unsafe { &*AVVideoHeightKey };
    let comp_key: &NSString = unsafe { &*AVVideoCompressionPropertiesKey };
    let width_num_ptr: *mut AnyObject =
        unsafe { msg_send![class!(NSNumber), numberWithLongLong: width] };
    let width_num = unsafe { Retained::retain(width_num_ptr) }
        .context("NSNumber width returned nil")?;
    let height_num_ptr: *mut AnyObject =
        unsafe { msg_send![class!(NSNumber), numberWithLongLong: height] };
    let height_num = unsafe { Retained::retain(height_num_ptr) }
        .context("NSNumber height returned nil")?;
    let video_settings_ptr: *mut AnyObject =
        unsafe { msg_send![class!(NSMutableDictionary), dictionary] };
    let video_settings = unsafe { Retained::retain(video_settings_ptr) }
        .context("NSMutableDictionary returned nil (video settings)")?;
    unsafe {
        let _: () = msg_send![&*video_settings, setObject: codec_val, forKey: codec_key];
        let _: () = msg_send![&*video_settings, setObject: &*width_num, forKey: width_key];
        let _: () = msg_send![&*video_settings, setObject: &*height_num, forKey: height_key];
        let _: () =
            msg_send![&*video_settings, setObject: &*compression_props, forKey: comp_key];
    }

    let writer_input_alloc: *mut AnyObject =
        unsafe { msg_send![class!(AVAssetWriterInput), alloc] };
    let writer_input_ptr: *mut AnyObject = unsafe {
        msg_send![
            writer_input_alloc,
            initWithMediaType: video_media_type,
            outputSettings: &*video_settings,
        ]
    };
    let writer_input = unsafe { Retained::from_raw(writer_input_ptr) }
        .context("AVAssetWriterInput init returned nil")?;
    // expectsMediaDataInRealTime=NO lets the writer pace itself instead of
    // assuming a live camera feed.
    unsafe {
        let _: () = msg_send![&*writer_input, setExpectsMediaDataInRealTime: false];
    }
    unsafe {
        let _: () = msg_send![&*writer, addInput: &*writer_input];
    }

    // ─── 4. Start ──────────────────────────────────────────────────────────
    let started_reading: bool = unsafe { msg_send![&*reader, startReading] };
    if !started_reading {
        let err: *mut NSError = unsafe { msg_send![&*reader, error] };
        bail!("AVAssetReader startReading failed: {}", ns_error_msg(err));
    }
    let started_writing: bool = unsafe { msg_send![&*writer, startWriting] };
    if !started_writing {
        let err: *mut NSError = unsafe { msg_send![&*writer, error] };
        bail!("AVAssetWriter startWriting failed: {}", ns_error_msg(err));
    }
    unsafe {
        let _: () = msg_send![&*writer, startSessionAtSourceTime: CMTIME_ZERO];
    }

    // ─── 5. Synchronous sample pump ────────────────────────────────────────
    // Instead of requestMediaDataWhenReadyOnQueue (block-on-dispatch-queue),
    // poll isReadyForMoreMediaData on this thread. Simpler, and our inputs
    // are short enough that the overhead is fine.
    let mut samples_written = 0usize;
    loop {
        let ready: bool = unsafe { msg_send![&*writer_input, isReadyForMoreMediaData] };
        if !ready {
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        let sample: *mut c_void = unsafe { msg_send![&*track_output, copyNextSampleBuffer] };
        if sample.is_null() {
            // End of stream — or a reader error. Distinguish by status.
            let reader_status: isize = unsafe { msg_send![&*reader, status] };
            // 1=reading, 2=completed, 3=failed, 4=cancelled
            if reader_status == 3 {
                let err: *mut NSError = unsafe { msg_send![&*reader, error] };
                bail!("AVAssetReader failed mid-read: {}", ns_error_msg(err));
            }
            unsafe {
                let _: () = msg_send![&*writer_input, markAsFinished];
            }
            break;
        }
        let appended: bool =
            unsafe { msg_send![&*writer_input, appendSampleBuffer: sample] };
        unsafe { CFRelease(sample) };
        if !appended {
            let err: *mut NSError = unsafe { msg_send![&*writer, error] };
            bail!(
                "AVAssetWriterInput appendSampleBuffer failed after {samples_written} samples: {}",
                ns_error_msg(err),
            );
        }
        samples_written += 1;
        if samples_written.is_multiple_of(30) {
            print!("\r  transcoding → HEVC Main10 (10-bit) mov... {samples_written} frames");
            io::stdout().flush().ok();
        }
    }

    // ─── 6. Finalize asynchronously, wait synchronously ────────────────────
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let done2 = Arc::clone(&done);
    let handler = RcBlock::new(move || {
        let (lock, cv) = &*done2;
        let mut d = lock.lock().unwrap();
        *d = true;
        cv.notify_one();
    });
    unsafe {
        let _: () = msg_send![&*writer, finishWritingWithCompletionHandler: &*handler];
    }
    let (lock, cv) = &*done;
    let mut d = lock.lock().unwrap();
    while !*d {
        d = cv.wait(d).unwrap();
    }

    let final_status: isize = unsafe { msg_send![&*writer, status] };
    match final_status {
        WRITER_STATUS_COMPLETED => Ok(()),
        WRITER_STATUS_FAILED => {
            let err: *mut NSError = unsafe { msg_send![&*writer, error] };
            bail!("AVAssetWriter failed: {}", ns_error_msg(err))
        }
        WRITER_STATUS_CANCELLED => bail!("AVAssetWriter cancelled"),
        other => bail!("AVAssetWriter unexpected final status {other}"),
    }
}

/// Pull the human-readable reason out of an Objective-C NSException so we
/// can include it in a Rust error instead of aborting on a foreign-exception
/// boundary.
fn describe_exception(exc: Option<Retained<objc2::exception::Exception>>) -> String {
    let Some(exc) = exc else {
        return "<nil NSException>".to_string();
    };
    // `name` and `reason` are both `nullable NSString`s.
    let name_ptr: *mut NSString = unsafe { msg_send![&*exc, name] };
    let reason_ptr: *mut NSString = unsafe { msg_send![&*exc, reason] };
    let to_string = |p: *mut NSString| -> String {
        if p.is_null() {
            return "<nil>".into();
        }
        match unsafe { Retained::retain(p) } {
            Some(s) => s.to_string(),
            None => "<nil>".into(),
        }
    };
    format!("{} — {}", to_string(name_ptr), to_string(reason_ptr))
}

/// Safe to call as long as `err` is either null or a pointer to a valid
/// `NSError`. msg_send! to a non-null NSError's localizedDescription always
/// returns a non-null NSString.
fn ns_error_msg(err: *mut NSError) -> String {
    if err.is_null() {
        return "<no NSError populated>".into();
    }
    let s_ptr: *mut NSString = unsafe { msg_send![err, localizedDescription] };
    if s_ptr.is_null() {
        return "<NSError without localizedDescription>".into();
    }
    let s = match unsafe { Retained::retain(s_ptr) } {
        Some(s) => s,
        None => return "<localizedDescription retain failed>".into(),
    };
    s.to_string()
}

fn extract_thumbnail(video: &Path, dest: &Path) -> Result<()> {
    let tmp_dir = tempfile::Builder::new().prefix("phonto-thumb").tempdir()?;
    let status = Command::new("/usr/bin/qlmanage")
        .args(["-t", "-s", "640", "-o"])
        .arg(tmp_dir.path())
        .arg(video)
        .status()
        .context("running qlmanage")?;
    if !status.success() {
        bail!("qlmanage exited with {status}");
    }
    let generated = tmp_dir
        .path()
        .join(format!("{}.png", video.file_name().unwrap().to_string_lossy()));
    if !generated.exists() {
        bail!("qlmanage produced no thumbnail at {}", generated.display());
    }
    fs::copy(&generated, dest)?;
    Ok(())
}

fn upsert_entry(
    manifest: &Path,
    asset_id: &str,
    display_name: &str,
    video_path: &Path,
    thumb_path: &Path,
) -> Result<()> {
    // One-time backup so the user can always revert with a single `cp`.
    let backup = manifest.with_extension("json.phonto-backup");
    if !backup.exists() {
        fs::copy(manifest, &backup)?;
        println!("backed up original manifest → {}", backup.display());
    }

    let text = fs::read_to_string(manifest)?;
    let mut data: Value = serde_json::from_str(&text).context("parsing entries.json")?;

    let video_url = file_url(video_path);
    let preview_url = file_url(thumb_path);

    upsert_phonto_category(&mut data, asset_id, &preview_url)?;

    let entry = json!({
        "id": asset_id,
        "accessibilityLabel": display_name,
        "categories": [PHONTO_CATEGORY_ID],
        "subcategories": [PHONTO_SUBCATEGORY_ID],
        "includeInShuffle": false,
        "localizedNameKey": display_name,
        "pointsOfInterest": {},
        "preferredOrder": 0,
        "previewImage": preview_url,
        "shotID": format!("PHONTO_{}", &asset_id[..8]),
        "showInTopLevel": true,
        "url-4K-SDR-240FPS": video_url,
    });

    let assets = data
        .get_mut("assets")
        .and_then(Value::as_array_mut)
        .context("entries.json has no `assets` array")?;
    if let Some(existing) = assets
        .iter_mut()
        .find(|a| a.get("id").and_then(Value::as_str) == Some(asset_id))
    {
        *existing = entry;
        println!("updated existing entry in entries.json");
    } else {
        assets.insert(0, entry);
        println!("inserted new entry in entries.json");
    }

    fs::write(manifest, serde_json::to_string_pretty(&data)?)?;
    Ok(())
}

fn upsert_phonto_category(
    data: &mut Value,
    rep_asset_id: &str,
    preview_url: &str,
) -> Result<()> {
    // Apple's picker validates the category — empty `representativeAssetID`
    // or empty `previewImage` poisons the catalog load and the entire
    // section list (including Apple's Landscapes) silently disappears from
    // System Settings. So we always populate these with the most recently
    // injected asset's values.
    let categories = data
        .get_mut("categories")
        .and_then(Value::as_array_mut)
        .context("entries.json has no `categories` array")?;
    if let Some(existing) = categories
        .iter_mut()
        .find(|c| c.get("id").and_then(Value::as_str) == Some(PHONTO_CATEGORY_ID))
    {
        existing["representativeAssetID"] = Value::String(rep_asset_id.into());
        existing["previewImage"] = Value::String(preview_url.into());
        // Always overwrite subcategories so a stale empty array from older
        // builds gets repaired — that empty array is what was poisoning the
        // catalog deserialization before this fix.
        existing["subcategories"] = json!([{
            "id": PHONTO_SUBCATEGORY_ID,
            "localizedDescriptionKey": "Phonto",
            "localizedNameKey": "Phonto",
            "preferredOrder": 0,
            "previewImage": preview_url,
            "representativeAssetID": rep_asset_id,
        }]);
    } else {
        categories.push(json!({
            "id": PHONTO_CATEGORY_ID,
            "localizedDescriptionKey": "Phonto",
            "localizedNameKey": "Phonto",
            // -1 sorts before Apple's Landscapes (which has 0).
            "preferredOrder": -1,
            "previewImage": preview_url,
            "representativeAssetID": rep_asset_id,
            // Apple's parser appears to require a non-empty `subcategories`
            // array. A single self-referential subcategory is enough — assets
            // reference this UUID so the structural invariant holds.
            "subcategories": [{
                "id": PHONTO_SUBCATEGORY_ID,
                "localizedDescriptionKey": "Phonto",
                "localizedNameKey": "Phonto",
                "preferredOrder": 0,
                "previewImage": preview_url,
                "representativeAssetID": rep_asset_id,
            }],
        }));
        println!("registered Phonto category in entries.json");
    }
    Ok(())
}

// Build a file URL with spaces percent-encoded. macOS' wallpaper system
// rejects raw spaces in `file://` URLs (Apple's own Index.plist always
// stores `Application%20Support`). Other special chars don't appear in
// our targets (the aerial catalog directory is fixed) so this is enough.
fn file_url(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte == b' ' {
            out.push_str("%20");
        } else {
            out.push(byte as char);
        }
    }
    out
}

fn remove_entry(manifest: &Path, asset_id: &str) -> Result<()> {
    let text = fs::read_to_string(manifest)?;
    let mut data: Value = serde_json::from_str(&text)?;
    if let Some(assets) = data.get_mut("assets").and_then(Value::as_array_mut) {
        assets.retain(|a| a.get("id").and_then(Value::as_str) != Some(asset_id));
    }
    fs::write(manifest, serde_json::to_string_pretty(&data)?)?;
    Ok(())
}

fn restart_aerials() {
    // Three processes hold cached state for the wallpaper picker / playback:
    //   - WallpaperAerialsExtension: the playback host (reads entries.json
    //     on launch, serves it to WallpaperAgent).
    //   - Wallpaper: the Settings-pane extension that draws the picker UI
    //     in System Settings. If we only kick the first two, the picker UI
    //     still shows its cached pre-injection catalog until System Settings
    //     is fully restarted.
    //   - WallpaperAgent: caches the resolved choice descriptor.
    for proc in ["WallpaperAerialsExtension", "Wallpaper", "WallpaperAgent"] {
        let _ = Command::new("/usr/bin/killall").arg(proc).status();
    }
    println!("kicked WallpaperAerialsExtension + Wallpaper (Settings) + WallpaperAgent");
}
