#!/usr/bin/env python3
"""
Inject (or update) a Phonto category + a single asset entry into the macOS
aerial-wallpaper catalog at:
  ~/Library/Application Support/com.apple.wallpaper/aerials/manifest/entries.json

Usage:
  scripts/inject-entries.py <asset-uuid> <display-name>

Where <asset-uuid> is the same UUID used for the .mov + thumbnail filenames
under aerials/videos/<UUID>.mov and aerials/thumbnails/<UUID>.png.

This is the same shape the (currently-broken) Rust wallpaper-injector writes;
having it as a separate script lets us verify the 10-bit transcode
hypothesis end-to-end without needing the Rust transcoder to compile.

Backs up the original entries.json to entries.json.phonto-backup once, the
first time the script runs against a fresh catalog.
"""

import json
import os
import shutil
import sys
from pathlib import Path

PHONTO_CATEGORY_ID = "8C75F1C2-7E7E-4B5C-9C5C-50484F4E544F"
PHONTO_SUBCATEGORY_ID = "8C75F1C2-7E7E-4B5C-9C5C-535542434154"


def file_url(path: Path) -> str:
    # macOS aerials want spaces percent-encoded; everything else in the
    # standard ~/Library tree is plain ASCII.
    return "file://" + str(path).replace(" ", "%20")


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <asset-uuid> <display-name>", file=sys.stderr)
        return 2

    asset_id = sys.argv[1].upper()
    display_name = sys.argv[2]

    aerials = Path.home() / "Library/Application Support/com.apple.wallpaper/aerials"
    manifest = aerials / "manifest/entries.json"
    video_path = aerials / "videos" / f"{asset_id}.mov"
    thumb_path = aerials / "thumbnails" / f"{asset_id}.png"

    if not manifest.exists():
        print(f"manifest not found: {manifest}", file=sys.stderr)
        return 1
    if not video_path.exists():
        print(f"video not present at {video_path} — run the transcoder first", file=sys.stderr)
        return 1

    backup = manifest.with_suffix(".json.phonto-backup")
    if not backup.exists():
        shutil.copy(manifest, backup)
        print(f"backed up original manifest -> {backup}")

    with open(manifest) as f:
        data = json.load(f)

    video_url = file_url(video_path)
    preview_url = file_url(thumb_path) if thumb_path.exists() else ""

    upsert_category(data, asset_id, preview_url)
    upsert_asset(data, asset_id, display_name, video_url, preview_url)

    with open(manifest, "w") as f:
        json.dump(data, f, indent=2)

    print(f"injected '{display_name}' (id {asset_id})")
    print("open System Settings -> Wallpaper, scroll to the Phonto section.")
    return 0


def upsert_category(data: dict, rep_asset_id: str, preview_url: str) -> None:
    # Apple's parser is strict: a Phonto category with empty
    # representativeAssetID / previewImage / subcategories poisons the whole
    # catalog and the picker silently drops *every* section. So we always
    # populate them with our newest asset.
    categories = data.setdefault("categories", [])
    subcategory = {
        "id": PHONTO_SUBCATEGORY_ID,
        "localizedDescriptionKey": "Phonto",
        "localizedNameKey": "Phonto",
        "preferredOrder": 0,
        "previewImage": preview_url,
        "representativeAssetID": rep_asset_id,
    }
    for existing in categories:
        if existing.get("id") == PHONTO_CATEGORY_ID:
            existing["representativeAssetID"] = rep_asset_id
            existing["previewImage"] = preview_url
            existing["subcategories"] = [subcategory]
            print("updated existing Phonto category")
            return
    categories.append({
        "id": PHONTO_CATEGORY_ID,
        "localizedDescriptionKey": "Phonto",
        "localizedNameKey": "Phonto",
        # -1 sorts above Apple's Landscapes (which is 0)
        "preferredOrder": -1,
        "previewImage": preview_url,
        "representativeAssetID": rep_asset_id,
        "subcategories": [subcategory],
    })
    print("registered new Phonto category")


def upsert_asset(data: dict, asset_id: str, display_name: str,
                 video_url: str, preview_url: str) -> None:
    entry = {
        "id": asset_id,
        "accessibilityLabel": display_name,
        "categories": [PHONTO_CATEGORY_ID],
        "subcategories": [PHONTO_SUBCATEGORY_ID],
        "includeInShuffle": False,
        "localizedNameKey": display_name,
        "pointsOfInterest": {},
        "preferredOrder": 0,
        "previewImage": preview_url,
        "shotID": f"PHONTO_{asset_id[:8]}",
        "showInTopLevel": True,
        "url-4K-SDR-240FPS": video_url,
    }
    assets = data.setdefault("assets", [])
    for i, a in enumerate(assets):
        if a.get("id") == asset_id:
            assets[i] = entry
            print("updated existing asset entry")
            return
    assets.insert(0, entry)
    print("inserted new asset entry")


if __name__ == "__main__":
    sys.exit(main())
