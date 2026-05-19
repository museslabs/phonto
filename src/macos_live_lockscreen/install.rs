// Install a video as the macOS desktop and lock-screen wallpaper by
// injecting it into Apple's aerial catalog. WallpaperAerialsExtension
// (Apple-signed, holds the private com.apple.private.wallpaper.extension
// entitlement) plays the asset on the lock screen.
//
// Pipeline (`phonto install-live-lockscreen <video>`):
//
//   1. Transcode to HEVC Main10 with two temporal sub-layers in the VPS.
//      Required for multi-cycle lock-screen playback. Implementation in
//      the sibling `transcode` module.
//   2. qlmanage thumbnail at 640px wide.
//   3. Upsert into entries.json under a stable UUIDv5 keyed off the
//      canonical source path.
//   4. killall WallpaperAerialsExtension. Killing WallpaperAgent works
//      equivalently. Either alone is sufficient. Without it the daemon
//      keeps playing the previously cached AVAsset under the same UUID.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::transcode;

// UUIDv5 namespace. Arbitrary value, just needs to be stable so the same
// video path maps to the same asset UUID across runs.
const NAMESPACE: Uuid = Uuid::from_bytes([
    0x70, 0x68, 0x6f, 0x6e, 0x74, 0x6f, 0x77, 0x70, 0x70, 0x72, 0x6f, 0x6a, 0x65, 0x63, 0x74, 0x21,
]);

// Phonto category in entries.json. The picker looks up `localizedNameKey`
// in localized .strings tables. Missing keys fall through to the raw
// string, which gives us the "Phonto" label.
const PHONTO_CATEGORY_ID: &str = "8C75F1C2-7E7E-4B5C-9C5C-50484F4E544F";

// Phonto subcategory. Apple's entries.json schema requires every asset to
// reference both a category and a subcategory UUID, and every category to
// have a non-empty subcategories array. Drop either and the aerials
// extension rejects the entire catalog. System Settings → Wallpaper then
// shows no Aerials section at all, not just no Phonto section.
const PHONTO_SUBCATEGORY_ID: &str = "8C75F1C2-7E7E-4B5C-9C5C-535542434154";

const PHONTO_LABEL: &str = "Phonto";

// Typed view over entries.json. We model the fields we read or write; any
// other field on a deserialized entry round-trips verbatim through `extra`
// (preserved insertion order thanks to serde_json's `preserve_order`).

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    assets: Vec<Asset>,
    #[serde(default)]
    categories: Vec<Category>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Asset {
    id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    categories: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    subcategories: Vec<String>,
    #[serde(
        default,
        rename = "previewImage",
        skip_serializing_if = "String::is_empty"
    )]
    preview_image: String,
    #[serde(
        default,
        rename = "accessibilityLabel",
        skip_serializing_if = "Option::is_none"
    )]
    accessibility_label: Option<String>,
    #[serde(
        default,
        rename = "localizedNameKey",
        skip_serializing_if = "Option::is_none"
    )]
    localized_name_key: Option<String>,
    #[serde(
        default,
        rename = "includeInShuffle",
        skip_serializing_if = "Option::is_none"
    )]
    include_in_shuffle: Option<bool>,
    #[serde(
        default,
        rename = "pointsOfInterest",
        skip_serializing_if = "Option::is_none"
    )]
    points_of_interest: Option<Map<String, Value>>,
    #[serde(
        default,
        rename = "preferredOrder",
        skip_serializing_if = "Option::is_none"
    )]
    preferred_order: Option<i64>,
    #[serde(default, rename = "shotID", skip_serializing_if = "Option::is_none")]
    shot_id: Option<String>,
    #[serde(
        default,
        rename = "showInTopLevel",
        skip_serializing_if = "Option::is_none"
    )]
    show_in_top_level: Option<bool>,
    #[serde(
        default,
        rename = "url-4K-SDR-240FPS",
        skip_serializing_if = "Option::is_none"
    )]
    url_4k_sdr_240fps: Option<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Category {
    id: String,
    #[serde(default, rename = "representativeAssetID")]
    representative_asset_id: String,
    #[serde(default, rename = "previewImage")]
    preview_image: String,
    #[serde(default)]
    subcategories: Vec<Subcategory>,
    #[serde(
        default,
        rename = "localizedDescriptionKey",
        skip_serializing_if = "Option::is_none"
    )]
    localized_description_key: Option<String>,
    #[serde(
        default,
        rename = "localizedNameKey",
        skip_serializing_if = "Option::is_none"
    )]
    localized_name_key: Option<String>,
    #[serde(
        default,
        rename = "preferredOrder",
        skip_serializing_if = "Option::is_none"
    )]
    preferred_order: Option<i64>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Subcategory {
    id: String,
    #[serde(default, rename = "representativeAssetID")]
    representative_asset_id: String,
    #[serde(default, rename = "previewImage")]
    preview_image: String,
    #[serde(
        default,
        rename = "localizedDescriptionKey",
        skip_serializing_if = "Option::is_none"
    )]
    localized_description_key: Option<String>,
    #[serde(
        default,
        rename = "localizedNameKey",
        skip_serializing_if = "Option::is_none"
    )]
    localized_name_key: Option<String>,
    #[serde(
        default,
        rename = "preferredOrder",
        skip_serializing_if = "Option::is_none"
    )]
    preferred_order: Option<i64>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

pub fn run(video: PathBuf, name: Option<String>, remove: bool) -> Result<()> {
    let video = if remove {
        // Removal only needs the path to derive the UUID. Fall back to a
        // best-effort absolute path if the source file is gone so cleanup
        // still works.
        video
            .canonicalize()
            .or_else(|_| std::path::absolute(&video))
            .with_context(|| format!("could not resolve path: {}", video.display()))?
    } else {
        video
            .canonicalize()
            .with_context(|| format!("video not found: {}", video.display()))?
    };

    let home = std::env::var("HOME").context("HOME not set")?;
    let aerials =
        PathBuf::from(&home).join("Library/Application Support/com.apple.wallpaper/aerials");
    let videos_dir = aerials.join("videos");
    let thumbnails_dir = aerials.join("thumbnails");
    let manifest_path = aerials.join("manifest/entries.json");
    if !manifest_path.exists() {
        bail!(
            "manifest not found at {} — WallpaperAgent has never initialised it on this Mac",
            manifest_path.display()
        );
    }

    let asset_id = Uuid::new_v5(&NAMESPACE, video.to_string_lossy().as_bytes())
        .hyphenated()
        .to_string()
        .to_uppercase();

    if remove {
        remove_entry(&manifest_path, &asset_id)?;
        let _ = fs::remove_file(videos_dir.join(format!("{asset_id}.mov")));
        let _ = fs::remove_file(thumbnails_dir.join(format!("{asset_id}.png")));
        kick_aerials();
        println!("removed {asset_id}");
        return Ok(());
    }

    fs::create_dir_all(&videos_dir)?;
    fs::create_dir_all(&thumbnails_dir)?;

    let display_name = name.unwrap_or_else(|| {
        video
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Phonto Custom".to_string())
    });

    let target_video = videos_dir.join(format!("{asset_id}.mov"));
    log::info!("transcoding → HEVC Main10 (2 temporal sub-layers)...");
    transcode::transcode(&video, &target_video)?;

    // Bail on thumbnail failure rather than write an entry with a dangling
    // previewImage. The catalog validator's behavior on missing-but-non-empty
    // preview URLs is undocumented and the comments above describe how easily
    // the whole Aerials section disappears when category metadata is malformed.
    let target_thumb = thumbnails_dir.join(format!("{asset_id}.png"));
    extract_thumbnail(&video, &target_thumb).context("extracting thumbnail (qlmanage)")?;
    log::info!("thumbnail → {}", target_thumb.display());

    upsert_entry(
        &manifest_path,
        &asset_id,
        &display_name,
        &target_video,
        &target_thumb,
    )?;

    kick_aerials();

    println!();
    println!("Installed '{display_name}' (id {asset_id}).");
    println!(
        "Open System Settings → Wallpaper, pick the 'Phonto' section, click \
         this entry, then lock the screen (Apple menu → Lock Screen) to verify \
         multi-cycle playback."
    );
    Ok(())
}

fn extract_thumbnail(video: &Path, dest: &Path) -> Result<()> {
    let tmp_dir = tempfile::Builder::new().prefix("phonto-thumb").tempdir()?;
    let output = Command::new("/usr/bin/qlmanage")
        .args(["-t", "-s", "640", "-o"])
        .arg(tmp_dir.path())
        .arg(video)
        .output()
        .context("running qlmanage")?;
    log_subprocess_output("qlmanage", &output.stdout, &output.stderr);
    if !output.status.success() {
        bail!("qlmanage exited with {}", output.status);
    }
    let generated = tmp_dir.path().join(format!(
        "{}.png",
        video.file_name().unwrap().to_string_lossy()
    ));
    if !generated.exists() {
        bail!("qlmanage produced no thumbnail at {}", generated.display());
    }
    fs::copy(&generated, dest)?;
    Ok(())
}

fn load_manifest(path: &Path) -> Result<Manifest> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).context("parsing entries.json")
}

fn save_manifest(path: &Path, manifest: &Manifest) -> Result<()> {
    fs::write(path, serde_json::to_string_pretty(manifest)?)?;
    Ok(())
}

fn upsert_entry(
    manifest_path: &Path,
    asset_id: &str,
    display_name: &str,
    video_path: &Path,
    thumb_path: &Path,
) -> Result<()> {
    // One-time backup so the user can always revert with a single `cp`.
    let backup = manifest_path.with_extension("json.phonto-backup");
    if !backup.exists() {
        fs::copy(manifest_path, &backup)?;
        log::info!("backed up original manifest → {}", backup.display());
    }

    let mut manifest = load_manifest(manifest_path)?;
    let video_url = file_url(video_path);
    let preview_url = file_url(thumb_path);

    upsert_phonto_category(&mut manifest, asset_id, &preview_url);

    let new_asset = phonto_asset(asset_id, display_name, &video_url, &preview_url);
    if let Some(existing) = manifest.assets.iter_mut().find(|a| a.id == asset_id) {
        *existing = new_asset;
        log::debug!("updated existing entry in entries.json");
    } else {
        manifest.assets.insert(0, new_asset);
        log::debug!("inserted new entry in entries.json");
    }

    save_manifest(manifest_path, &manifest)
}

fn upsert_phonto_category(manifest: &mut Manifest, rep_asset_id: &str, preview_url: &str) {
    // The picker validates the category. An empty representativeAssetID or
    // previewImage poisons the catalog load and the entire section list
    // (including Apple's Landscapes) silently disappears from System
    // Settings. Always populate these from the latest install.
    if let Some(existing) = manifest
        .categories
        .iter_mut()
        .find(|c| c.id == PHONTO_CATEGORY_ID)
    {
        existing.representative_asset_id = rep_asset_id.to_string();
        existing.preview_image = preview_url.to_string();
        // Always overwrite subcategories. A stale empty array from older
        // builds poisons the catalog decode.
        existing.subcategories = vec![phonto_subcategory(rep_asset_id, preview_url)];
    } else {
        manifest
            .categories
            .push(phonto_category(rep_asset_id, preview_url));
        log::debug!("registered Phonto category in entries.json");
    }
}

fn remove_entry(manifest_path: &Path, asset_id: &str) -> Result<()> {
    let mut manifest = load_manifest(manifest_path)?;
    manifest.assets.retain(|a| a.id != asset_id);
    repair_phonto_category(&mut manifest);
    save_manifest(manifest_path, &manifest)
}

// Keep the Phonto category in sync with the surviving assets. The category's
// representativeAssetID and previewImage are validated by the picker. If they
// point at a UUID we just deleted, the entire Aerials section disappears from
// System Settings. If no Phonto assets remain, drop the category entirely so
// the manifest looks like a fresh-install Mac.
fn repair_phonto_category(manifest: &mut Manifest) {
    match first_phonto_asset(manifest) {
        Some((rep_id, preview)) => {
            if let Some(cat) = manifest
                .categories
                .iter_mut()
                .find(|c| c.id == PHONTO_CATEGORY_ID)
            {
                cat.representative_asset_id = rep_id.clone();
                cat.preview_image = preview.clone();
                cat.subcategories = vec![phonto_subcategory(&rep_id, &preview)];
            }
        }
        None => {
            manifest.categories.retain(|c| c.id != PHONTO_CATEGORY_ID);
        }
    }
}

// Any remaining asset that references the Phonto category, returned as
// (id, previewImage). Used to repair representativeAssetID/previewImage on
// the category after a removal.
fn first_phonto_asset(manifest: &Manifest) -> Option<(String, String)> {
    manifest.assets.iter().find_map(|asset| {
        if !asset.categories.iter().any(|c| c == PHONTO_CATEGORY_ID) {
            return None;
        }
        Some((asset.id.clone(), asset.preview_image.clone()))
    })
}

fn phonto_asset(asset_id: &str, display_name: &str, video_url: &str, preview_url: &str) -> Asset {
    Asset {
        id: asset_id.to_string(),
        categories: vec![PHONTO_CATEGORY_ID.to_string()],
        subcategories: vec![PHONTO_SUBCATEGORY_ID.to_string()],
        preview_image: preview_url.to_string(),
        accessibility_label: Some(display_name.to_string()),
        localized_name_key: Some(display_name.to_string()),
        include_in_shuffle: Some(false),
        points_of_interest: Some(Map::new()),
        preferred_order: Some(0),
        shot_id: Some(format!("PHONTO_{}", &asset_id[..8])),
        show_in_top_level: Some(true),
        url_4k_sdr_240fps: Some(video_url.to_string()),
        extra: Map::new(),
    }
}

fn phonto_category(rep_asset_id: &str, preview_url: &str) -> Category {
    Category {
        id: PHONTO_CATEGORY_ID.to_string(),
        representative_asset_id: rep_asset_id.to_string(),
        preview_image: preview_url.to_string(),
        // Apple's parser requires a non-empty subcategories array. One
        // self-referential entry suffices, since assets reference this
        // UUID directly.
        subcategories: vec![phonto_subcategory(rep_asset_id, preview_url)],
        localized_description_key: Some(PHONTO_LABEL.to_string()),
        localized_name_key: Some(PHONTO_LABEL.to_string()),
        // -1 sorts before Apple's Landscapes (which has 0).
        preferred_order: Some(-1),
        extra: Map::new(),
    }
}

fn phonto_subcategory(rep_asset_id: &str, preview_url: &str) -> Subcategory {
    Subcategory {
        id: PHONTO_SUBCATEGORY_ID.to_string(),
        representative_asset_id: rep_asset_id.to_string(),
        preview_image: preview_url.to_string(),
        localized_description_key: Some(PHONTO_LABEL.to_string()),
        localized_name_key: Some(PHONTO_LABEL.to_string()),
        preferred_order: Some(0),
        extra: Map::new(),
    }
}

// Build a file URL with spaces percent-encoded. The wallpaper system
// rejects raw spaces in file:// URLs. Apple's own Index.plist uses %20.
// Other special characters don't appear in our target paths.
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

// Force WallpaperAerialsExtension to re-read entries.json on its next
// launch. Without this, an updated .mov at an existing UUID still plays
// from the daemon's cached AVAsset. Killing WallpaperAgent works the same.
// Either is sufficient.
fn kick_aerials() {
    if let Ok(output) = Command::new("/usr/bin/killall")
        .arg("WallpaperAerialsExtension")
        .output()
    {
        log_subprocess_output("killall", &output.stdout, &output.stderr);
    }
}

fn log_subprocess_output(tag: &str, stdout: &[u8], stderr: &[u8]) {
    for line in String::from_utf8_lossy(stdout).lines() {
        log::debug!("{tag}: {line}");
    }
    for line in String::from_utf8_lossy(stderr).lines() {
        log::debug!("{tag}: {line}");
    }
}
