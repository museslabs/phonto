#!/usr/bin/env bash
# Look into Wallper's separate LockScreenCache and try to map out how it's
# wired into the actual lock-screen rendering pipeline.
#
# The hypothesis: Wallper does TWO things instead of one:
#   (a) Inject into ~/Library/Application Support/com.apple.wallpaper/aerials/
#       (the only thing we've been doing so far).
#   (b) Stage a separate .mov under
#       ~/Library/Application Support/Wallper/LockScreenCache/ and reference
#       it via some other state (Index.plist? a private API call? a LaunchAgent?).
#
# Usage:
#   scripts/probe-wallper-lockcache.sh

set -u

WPDIR="$HOME/Library/Application Support/Wallper"
LSCDIR="$WPDIR/LockScreenCache"
INDEX="$HOME/Library/Application Support/com.apple.wallpaper/Store/Index.plist"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "=== full Wallper app-support tree ==="
find "$WPDIR" -type f 2>/dev/null | sort
echo

echo "=== LockScreenCache contents ==="
ls -la "$LSCDIR" 2>/dev/null
echo

LSC_UUID=$(/bin/ls "$LSCDIR" 2>/dev/null | head -1 | sed 's/\.mov$//')
echo "LockScreenCache UUID: $LSC_UUID"
echo

if [ -n "$LSC_UUID" ]; then
    echo "=== inspect LockScreenCache mov ==="
    "$HERE/inspect-bitdepth.swift" "$LSCDIR/$LSC_UUID.mov" 2>/dev/null
    echo
    echo "=== xattrs on LockScreenCache mov ==="
    xattr -l "$LSCDIR/$LSC_UUID.mov"
    echo
fi

echo "=== local_media_metadata.json (truncated) ==="
if [ -f "$WPDIR/local_media_metadata.json" ]; then
    /usr/bin/python3 -c "
import json, sys
d = json.load(open('$WPDIR/local_media_metadata.json'))
print(json.dumps(d, indent=2)[:3000])
"
fi
echo

echo "=== local_media_titles.json (truncated) ==="
if [ -f "$WPDIR/local_media_titles.json" ]; then
    /usr/bin/python3 -c "
import json, sys
d = json.load(open('$WPDIR/local_media_titles.json'))
print(json.dumps(d, indent=2)[:1500])
"
fi
echo

if [ -n "$LSC_UUID" ]; then
    echo "=== where does $LSC_UUID get referenced? ==="
    echo "--- in Index.plist ---"
    plutil -p "$INDEX" 2>/dev/null | grep -i "$LSC_UUID" || echo "  (not found in Index.plist)"
    echo "--- in any plist under ~/Library/Preferences/ ---"
    grep -rl "$LSC_UUID" ~/Library/Preferences/ 2>/dev/null | head
    echo "--- in any plist under ~/Library/Containers/ ---"
    grep -rl "$LSC_UUID" ~/Library/Containers/ 2>/dev/null | head
    echo "--- in entries.json ---"
    grep "$LSC_UUID" "$HOME/Library/Application Support/com.apple.wallpaper/aerials/manifest/entries.json" || echo "  (not found in entries.json)"
fi
echo

echo "=== launch agents Wallper might have installed ==="
ls -la ~/Library/LaunchAgents/*[Ww]allper* ~/Library/LaunchAgents/sandimax* 2>/dev/null
echo

echo "=== anything in Wallper.app/Contents/Resources/ that hints at the mechanism ==="
RES="/Applications/Wallper.app/Contents/Resources"
ls "$RES" 2>/dev/null | head
echo
echo "=== running Wallper processes ==="
ps -A | grep -iE '[Ww]allper' | head
echo

echo "=== leftover xattr cleanup on OUR injected mov ==="
OURS="$HOME/Library/Application Support/com.apple.wallpaper/aerials/videos/17A6A998-4049-5A53-A08D-FD553BE57044.mov"
if [ -f "$OURS" ]; then
    xattr -d LastETag "$OURS" 2>/dev/null && echo "  removed LastETag"
    xattr -d com.apple.quarantine "$OURS" 2>/dev/null && echo "  removed com.apple.quarantine"
    echo "  remaining xattrs:"
    xattr -l "$OURS" 2>/dev/null
fi
