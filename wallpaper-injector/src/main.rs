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

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use block2::RcBlock;
use clap::Parser;
use objc2::ClassType;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{AVAssetExportSession, AVURLAsset};
use objc2_foundation::{NSError, NSString, NSURL};
use serde_json::{Value, json};
use uuid::Uuid;

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

// HEVC 1920×1080 preset. If this doesn't fix the second-lock-black bug we
// fall back to AVAssetWriter with explicit 10-bit BT.2020 pixel format.
const TRANSCODE_PRESET: &str = "AVAssetExportPresetHEVC1920x1080";

// AVFileType UTI for QuickTime `.mov`. AVAssetExportSession refuses to emit
// without a file-type hint and Apple's aerial catalog is all-mov.
const QUICKTIME_MOVIE_UTI: &str = "com.apple.quicktime-movie";

// AVAssetExportSessionStatus enum values (NSInteger).
const STATUS_UNKNOWN: isize = 0;
const STATUS_WAITING: isize = 1;
const STATUS_EXPORTING: isize = 2;
const STATUS_COMPLETED: isize = 3;
const STATUS_FAILED: isize = 4;
const STATUS_CANCELLED: isize = 5;

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
    // AVAssetExportSession refuses to overwrite — delete any prior output.
    let _ = fs::remove_file(output);

    let input_str = input.to_str().context("input path is not valid UTF-8")?;
    let output_str = output.to_str().context("output path is not valid UTF-8")?;
    let input_url = unsafe { NSURL::fileURLWithPath(&NSString::from_str(input_str)) };
    let output_url = unsafe { NSURL::fileURLWithPath(&NSString::from_str(output_str)) };

    let nil_opts: *const AnyObject = std::ptr::null();
    let asset: Retained<AVURLAsset> = unsafe {
        msg_send![
            AVURLAsset::class(),
            URLAssetWithURL: &*input_url,
            options: nil_opts,
        ]
    };

    let preset = NSString::from_str(TRANSCODE_PRESET);
    let session: Retained<AVAssetExportSession> = unsafe {
        msg_send![
            AVAssetExportSession::class(),
            exportSessionWithAsset: &*asset,
            presetName: &*preset,
        ]
    };

    let file_type = NSString::from_str(QUICKTIME_MOVIE_UTI);
    unsafe {
        let _: () = msg_send![&*session, setOutputURL: &*output_url];
        let _: () = msg_send![&*session, setOutputFileType: &*file_type];
    }

    // Completion handler is required by the API but we ignore it and poll
    // `status` from this thread instead — the block runs on an AVFoundation
    // worker queue and we don't want to fight a callback indirection.
    let handler = RcBlock::new(|| {});
    unsafe {
        let _: () = msg_send![&*session, exportAsynchronouslyWithCompletionHandler: &*handler];
    }

    print!("  transcoding → HEVC mov...");
    io::stdout().flush().ok();
    loop {
        let status: isize = unsafe { msg_send![&*session, status] };
        let progress: f32 = unsafe { msg_send![&*session, progress] };
        match status {
            STATUS_COMPLETED => {
                println!("\r  transcoded → {} (100%)            ", output.display());
                return Ok(());
            }
            STATUS_FAILED => {
                let err: Option<Retained<NSError>> = unsafe { msg_send![&*session, error] };
                let msg = err
                    .map(|e| unsafe {
                        let s: Retained<NSString> = msg_send![&*e, localizedDescription];
                        s.to_string()
                    })
                    .unwrap_or_else(|| "<no error message>".into());
                bail!("AVAssetExportSession failed: {msg}");
            }
            STATUS_CANCELLED => bail!("AVAssetExportSession was cancelled"),
            STATUS_UNKNOWN | STATUS_WAITING | STATUS_EXPORTING => {
                print!(
                    "\r  transcoding → HEVC mov... {:>3}%   ",
                    (progress * 100.0) as u32
                );
                io::stdout().flush().ok();
                thread::sleep(Duration::from_millis(250));
            }
            other => bail!("unexpected AVAssetExportSession status {other}"),
        }
    }
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
