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
use serde_json::{Value, json};
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

    if remove {
        remove_entry(&manifest, &asset_id)?;
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
        &manifest,
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
    let status = Command::new("/usr/bin/qlmanage")
        .args(["-t", "-s", "640", "-o"])
        .arg(tmp_dir.path())
        .arg(video)
        .status()
        .context("running qlmanage")?;
    if !status.success() {
        bail!("qlmanage exited with {status}");
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
        log::info!("backed up original manifest → {}", backup.display());
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
        log::debug!("updated existing entry in entries.json");
    } else {
        assets.insert(0, entry);
        log::debug!("inserted new entry in entries.json");
    }

    fs::write(manifest, serde_json::to_string_pretty(&data)?)?;
    Ok(())
}

fn upsert_phonto_category(data: &mut Value, rep_asset_id: &str, preview_url: &str) -> Result<()> {
    // The picker validates the category. An empty representativeAssetID or
    // previewImage poisons the catalog load and the entire section list
    // (including Apple's Landscapes) silently disappears from System
    // Settings. Always populate these from the latest install.
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
        // Always overwrite subcategories. A stale empty array from older
        // builds poisons the catalog decode.
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
            // Apple's parser requires a non-empty subcategories array. One
            // self-referential entry suffices, since assets reference this
            // UUID directly.
            "subcategories": [{
                "id": PHONTO_SUBCATEGORY_ID,
                "localizedDescriptionKey": "Phonto",
                "localizedNameKey": "Phonto",
                "preferredOrder": 0,
                "previewImage": preview_url,
                "representativeAssetID": rep_asset_id,
            }],
        }));
        log::debug!("registered Phonto category in entries.json");
    }
    Ok(())
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

fn remove_entry(manifest: &Path, asset_id: &str) -> Result<()> {
    let text = fs::read_to_string(manifest)?;
    let mut data: Value = serde_json::from_str(&text)?;
    if let Some(assets) = data.get_mut("assets").and_then(Value::as_array_mut) {
        assets.retain(|a| a.get("id").and_then(Value::as_str) != Some(asset_id));
    }
    repair_phonto_category(&mut data);
    fs::write(manifest, serde_json::to_string_pretty(&data)?)?;
    Ok(())
}

// Keep the Phonto category in sync with the surviving assets. The category's
// representativeAssetID and previewImage are validated by the picker. If they
// point at a UUID we just deleted, the entire Aerials section disappears from
// System Settings. If no Phonto assets remain, drop the category entirely so
// the manifest looks like a fresh-install Mac.
fn repair_phonto_category(data: &mut Value) {
    let surviving = first_phonto_asset(data);
    let Some(categories) = data.get_mut("categories").and_then(Value::as_array_mut) else {
        return;
    };
    match surviving {
        Some((rep_id, preview)) => {
            if let Some(cat) = categories
                .iter_mut()
                .find(|c| c.get("id").and_then(Value::as_str) == Some(PHONTO_CATEGORY_ID))
            {
                cat["representativeAssetID"] = Value::String(rep_id.clone());
                cat["previewImage"] = Value::String(preview.clone());
                cat["subcategories"] = json!([{
                    "id": PHONTO_SUBCATEGORY_ID,
                    "localizedDescriptionKey": "Phonto",
                    "localizedNameKey": "Phonto",
                    "preferredOrder": 0,
                    "previewImage": preview,
                    "representativeAssetID": rep_id,
                }]);
            }
        }
        None => {
            categories.retain(|c| c.get("id").and_then(Value::as_str) != Some(PHONTO_CATEGORY_ID));
        }
    }
}

// Any remaining asset that references the Phonto category, returned as
// (id, previewImage). Used to repair representativeAssetID/previewImage on
// the category after a removal.
fn first_phonto_asset(data: &Value) -> Option<(String, String)> {
    let assets = data.get("assets")?.as_array()?;
    assets.iter().find_map(|asset| {
        let cats = asset.get("categories")?.as_array()?;
        let references_phonto = cats.iter().any(|c| c.as_str() == Some(PHONTO_CATEGORY_ID));
        if !references_phonto {
            return None;
        }
        let id = asset.get("id")?.as_str()?.to_string();
        let preview = asset.get("previewImage")?.as_str()?.to_string();
        Some((id, preview))
    })
}

// Force WallpaperAerialsExtension to re-read entries.json on its next
// launch. Without this, an updated .mov at an existing UUID still plays
// from the daemon's cached AVAsset. Killing WallpaperAgent works the same.
// Either is sufficient.
fn kick_aerials() {
    let _ = Command::new("/usr/bin/killall")
        .arg("WallpaperAerialsExtension")
        .status();
}
