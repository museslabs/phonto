#!/usr/bin/env bash
# Deeper comparison of Wallper's injected state vs ours. The obvious
# differences (mov format, entries.json shape) have been ruled out; this
# script looks at everything else Wallper might write that we don't:
# extended attributes, MOV container atoms, the WallpaperAgent Store, the
# extension's container, and anything modified under ~/Library since
# Wallper was active.
#
# Usage:
#   scripts/diff-wallper-vs-ours.sh

set -u

AERIALS="$HOME/Library/Application Support/com.apple.wallpaper/aerials"
JSON="$AERIALS/manifest/entries.json"
VIDEOS="$AERIALS/videos"

# Resolve Wallper's and our injected asset UUIDs from the manifest.
read -r WALLPER_ID OURS_ID < <(/usr/bin/python3 - "$JSON" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
w = next((a["id"] for a in d.get("assets", [])
          if a.get("shotID", "").startswith("CUSTOM_")), "")
o = next((a["id"] for a in d.get("assets", [])
          if a.get("shotID", "").startswith("PHONTO_")), "")
print(w or "MISSING", o or "MISSING")
PY
)

W="$VIDEOS/$WALLPER_ID.mov"
O="$VIDEOS/$OURS_ID.mov"
echo "Wallper mov: $W"
echo "Our mov:     $O"
echo

echo "=== file metadata diff ==="
echo "--- Wallper ---"
ls -la@e "$W" 2>/dev/null || ls -la "$W"
echo
echo "--- ours ---"
ls -la@e "$O" 2>/dev/null || ls -la "$O"
echo

echo "=== extended attributes (xattr -l) ==="
echo "--- Wallper ---"
xattr -l "$W" 2>&1
echo
echo "--- ours ---"
xattr -l "$O" 2>&1
echo

echo "=== mdls metadata diff ==="
diff <(mdls "$W" 2>/dev/null) <(mdls "$O" 2>/dev/null) || true
echo

echo "=== first 256 bytes of each (mov atom signatures) ==="
echo "--- Wallper ---"
xxd -l 256 "$W"
echo
echo "--- ours ---"
xxd -l 256 "$O"
echo

echo "=== top-level mov atoms (via macOS' 'atomicparsley' or simple atom-scan) ==="
# We use Python to do a minimal top-level atom walk — no external tool dep.
atom_walk() {
    local file="$1"
    /usr/bin/python3 - "$file" <<'PY'
import struct, sys
with open(sys.argv[1], "rb") as f:
    while True:
        hdr = f.read(8)
        if len(hdr) < 8: break
        size, name = struct.unpack(">I4s", hdr)
        name = name.decode("ascii", errors="replace")
        if size == 1:
            size = struct.unpack(">Q", f.read(8))[0]
            body = size - 16
        elif size == 0:
            print(f"  [{name}] (to EOF)"); break
        else:
            body = size - 8
        print(f"  [{name}] size={size}")
        f.seek(body, 1)
PY
}
echo "--- Wallper atoms ---"
atom_walk "$W"
echo "--- our atoms ---"
atom_walk "$O"
echo

echo "=== files under ~/Library written or modified in the last 30 minutes (excluding caches/logs) ==="
find ~/Library \
    -not -path '*/Caches/*' \
    -not -path '*/Logs/*' \
    -not -path '*/Mail/*' \
    -not -path '*/Application Scripts/*' \
    -mmin -30 -type f 2>/dev/null \
    | grep -iE 'wallpaper|aerial|wallper' \
    | head -40
echo

echo "=== Wallpaper-related sqlite/plist databases ==="
find ~/Library -maxdepth 6 -type f \
    \( -name '*.sqlite*' -o -name '*.db' \) \
    2>/dev/null | grep -iE 'wallpaper|aerial|wallper'
echo

echo "=== Containers for wallpaper-related apps ==="
for d in ~/Library/Containers/* ~/Library/Group\ Containers/*; do
    name=$(basename "$d" 2>/dev/null)
    case "$name" in
        *[Ww]allpaper*|*[Aa]erial*|*[Ww]allper*|*sandimax*)
            echo "--- $d ---"
            ls -la "$d" 2>/dev/null | head
        ;;
    esac
done
echo

echo "=== Index.plist Phonto-related slot configs ==="
INDEX="$HOME/Library/Application Support/com.apple.wallpaper/Store/Index.plist"
if [ -f "$INDEX" ]; then
    plutil -p "$INDEX" 2>/dev/null \
        | grep -B1 -A4 'choice.aerials\|Phonto\|Wallper\|17A6A998\|ED68F689' \
        | head -60
fi
