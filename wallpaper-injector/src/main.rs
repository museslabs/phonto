// Injects a user-provided video into WallpaperAgent's aerial catalog so the
// macOS Wallpaper picker offers it as a selectable wallpaper (and, by
// extension, lock-screen background).
//
// Format reverse-engineered from `~/Library/Application
// Support/com.apple.wallpaper/aerials/manifest/entries.json` on macOS 26.
// Layout (per-asset): `videos/<UUID>.mov`, optional `thumbnails/<UUID>.png`,
// and an entry in `manifest/entries.json` cross-referencing both.
//
// Brittle by nature — Apple can revise the schema across macOS releases and
// WallpaperAgent occasionally refreshes the manifest from its tar, which
// erases our edits. Re-run this binary to restore.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde_json::{Value, json};
use uuid::Uuid;

// Existing IDs harvested from entries.json. Slotting our entry into the
// Landscapes/Tahoe subcategory makes it appear next to Apple's built-ins,
// which is the easiest place for the picker UI to render it.
const LANDSCAPES_CATEGORY: &str = "A33A55D9-EDEA-4596-A850-6C10B54FBBB5";
const TAHOE_SUBCATEGORY: &str = "0DC99DD8-3386-4D1E-8878-C43E97EB710A";

// UUIDv5 namespace — arbitrary, just needs to be stable across runs so the
// same video produces the same asset UUID (idempotent re-injection).
const NAMESPACE: Uuid = Uuid::from_bytes([
    0x70, 0x68, 0x6f, 0x6e, 0x74, 0x6f, 0x77, 0x70, 0x70, 0x72, 0x6f, 0x6a, 0x65, 0x63, 0x74, 0x21,
]);

#[derive(Parser)]
#[command(about = "Inject a video into the macOS Wallpaper picker")]
struct Args {
    /// Path to the video file (MP4/MOV).
    video: PathBuf,

    /// Display name for the picker (defaults to the file stem).
    #[arg(long)]
    name: Option<String>,

    /// Remove the previously-injected entry for this video instead of adding it.
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
    let aerials = PathBuf::from(home).join("Library/Application Support/com.apple.wallpaper/aerials");
    let videos_dir = aerials.join("videos");
    let thumbnails_dir = aerials.join("thumbnails");
    let manifest = aerials.join("manifest/entries.json");

    if !manifest.exists() {
        bail!(
            "manifest not found at {} — WallpaperAgent may not be initialised yet",
            manifest.display()
        );
    }

    // UUIDv5 of the canonical path → deterministic. Re-running with the same
    // video updates the entry in place instead of duplicating.
    let asset_id = Uuid::new_v5(&NAMESPACE, video.to_string_lossy().as_bytes())
        .hyphenated()
        .to_string()
        .to_uppercase();

    if args.remove {
        remove_entry(&manifest, &asset_id)?;
        let _ = fs::remove_file(videos_dir.join(format!("{asset_id}.mov")));
        let _ = fs::remove_file(thumbnails_dir.join(format!("{asset_id}.png")));
        kick_wallpaper_agent();
        println!("removed entry {asset_id}");
        return Ok(());
    }

    fs::create_dir_all(&videos_dir)?;
    fs::create_dir_all(&thumbnails_dir)?;

    let target_video = videos_dir.join(format!("{asset_id}.mov"));
    if !target_video.exists() {
        fs::copy(&video, &target_video)
            .with_context(|| format!("copying video to {}", target_video.display()))?;
        println!("copied video → {}", target_video.display());
    } else {
        println!("video already in place at {}", target_video.display());
    }

    let target_thumb = thumbnails_dir.join(format!("{asset_id}.png"));
    if !target_thumb.exists() {
        if let Err(e) = extract_thumbnail(&video, &target_thumb) {
            eprintln!("warning: thumbnail extraction failed ({e}) — picker may show a placeholder");
        } else {
            println!("thumbnail → {}", target_thumb.display());
        }
    }

    let display_name = args.name.unwrap_or_else(|| {
        video
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Phonto Custom".to_string())
    });

    upsert_entry(&manifest, &asset_id, &display_name, &target_video, &target_thumb)?;
    kick_wallpaper_agent();

    println!();
    println!("Injected '{display_name}' (id {asset_id}).");
    println!("Open System Settings → Wallpaper → Landscapes to select it.");
    println!("If it doesn't appear, log out + back in to force a manifest reload.");
    Ok(())
}

fn extract_thumbnail(video: &Path, dest: &Path) -> Result<()> {
    // `qlmanage -t` ships with macOS and writes <video-basename>.png into the
    // output directory. We then rename to <UUID>.png to match the manifest id.
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
    // Stash a one-time backup so the user can revert if anything goes sideways.
    let backup = manifest.with_extension("json.phonto-backup");
    if !backup.exists() {
        fs::copy(manifest, &backup)?;
        println!("backed up original manifest → {}", backup.display());
    }

    let text = fs::read_to_string(manifest)?;
    let mut data: Value = serde_json::from_str(&text).context("parsing entries.json")?;
    let assets = data
        .get_mut("assets")
        .and_then(Value::as_array_mut)
        .context("entries.json has no `assets` array")?;

    // Local file URLs so WallpaperAgent uses the on-disk copy and doesn't try
    // to fetch from sylvan.apple.com.
    let video_url = format!("file://{}", video_path.display());
    let preview_url = format!("file://{}", thumb_path.display());

    let entry = json!({
        "id": asset_id,
        "accessibilityLabel": display_name,
        "categories": [LANDSCAPES_CATEGORY],
        "subcategories": [TAHOE_SUBCATEGORY],
        "includeInShuffle": false,
        "localizedNameKey": display_name,
        "pointsOfInterest": {},
        "preferredOrder": 0,
        "previewImage": preview_url,
        "shotID": format!("PHONTO_{}", &asset_id[..8]),
        "showInTopLevel": true,
        "url-4K-SDR-240FPS": video_url,
    });

    if let Some(existing) = assets
        .iter_mut()
        .find(|a| a.get("id").and_then(Value::as_str) == Some(asset_id))
    {
        *existing = entry;
        println!("updated existing entry");
    } else {
        assets.insert(0, entry);
        println!("inserted new entry");
    }

    fs::write(manifest, serde_json::to_string_pretty(&data)?)?;
    Ok(())
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

fn kick_wallpaper_agent() {
    // SIGTERM the daemon; launchd respawns it and it re-reads the manifest.
    let _ = Command::new("/usr/bin/killall").arg("WallpaperAgent").status();
    println!("kicked WallpaperAgent");
}
