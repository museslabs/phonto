#!/usr/bin/env bash
# Patch our injected Phonto entries to match the shape Wallper uses, then
# kick the wallpaper daemons. Use after running the transcoder + injector;
# this is the second-lock-black fix experiment.
#
# Two fields differ between Wallper's working entry and ours:
#   - includeInShuffle: false → true
#   - pointsOfInterest:  {}   → { "0": "<shotID>_0" }
#
# Also inspects Wallper's transcoded .mov so we can compare codec/bit depth
# against ours and rule out the transcode itself as the cause.
#
# Usage:
#   scripts/test-wallper-shape.sh

set -euo pipefail

AERIALS="$HOME/Library/Application Support/com.apple.wallpaper/aerials"
JSON="$AERIALS/manifest/entries.json"
VIDEOS="$AERIALS/videos"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ ! -f "$JSON" ]; then
    echo "manifest not found: $JSON" >&2
    exit 1
fi

echo "=== mov-file comparison: Wallper vs ours ==="
# Read the assets out of entries.json once, find Wallper's UUID (CUSTOM_*
# shotID) and ours (PHONTO_* shotID), inspect both.
read -r WALLPER_ID OURS_ID < <(/usr/bin/python3 - "$JSON" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
w = next((a["id"] for a in d.get("assets", [])
          if a.get("shotID", "").startswith("CUSTOM_")), "")
o = next((a["id"] for a in d.get("assets", [])
          if a.get("shotID", "").startswith("PHONTO_")), "")
print(w, o)
PY
)

if [ -n "$WALLPER_ID" ] && [ -f "$VIDEOS/$WALLPER_ID.mov" ]; then
    echo "--- Wallper's ($WALLPER_ID.mov) ---"
    "$HERE/inspect-bitdepth.swift" "$VIDEOS/$WALLPER_ID.mov" || true
else
    echo "no Wallper-injected entry found in manifest"
fi

if [ -n "$OURS_ID" ] && [ -f "$VIDEOS/$OURS_ID.mov" ]; then
    echo "--- ours ($OURS_ID.mov) ---"
    "$HERE/inspect-bitdepth.swift" "$VIDEOS/$OURS_ID.mov" || true
else
    echo "no Phonto-injected entry found in manifest — run inject-entries.py first"
fi

echo
echo "=== patching our PHONTO_* assets to Wallper-style shape ==="
/usr/bin/python3 - "$JSON" <<'PY'
import json, sys
path = sys.argv[1]
with open(path) as f:
    d = json.load(f)

patched = 0
for a in d.get("assets", []):
    sid = a.get("shotID", "")
    if not sid.startswith("PHONTO_"):
        continue
    a["includeInShuffle"] = True
    a["pointsOfInterest"] = {"0": f"{sid}_0"}
    patched += 1
    print(f"  patched {sid}")

if patched == 0:
    print("  no PHONTO_* assets found to patch")
else:
    with open(path, "w") as f:
        json.dump(d, f, indent=2)
    print(f"  wrote {patched} change(s) to {path}")
PY

echo
echo "=== kicking aerials extension + picker UI + WallpaperAgent ==="
killall WallpaperAerialsExtension 2>/dev/null && echo "  killed WallpaperAerialsExtension"
killall Wallpaper                  2>/dev/null && echo "  killed Wallpaper (Settings extension)"
killall WallpaperAgent             2>/dev/null && echo "  killed WallpaperAgent"

echo
echo "Done. Re-open System Settings → Wallpaper, re-pick the Phonto video,"
echo "then run the lock-cycle test (lock → unlock → lock, several times)."
