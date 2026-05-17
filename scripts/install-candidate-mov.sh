#!/bin/bash
#
# Drop a candidate .mov under our injected aerials entry and kick the
# wallpaper daemons so the next lock cycle picks it up. Pairs with
# `transcode-hevc-temporal.swift` for the test loop:
#
#   swift scripts/transcode-hevc-temporal.swift input.mp4 /tmp/test.mov
#   ./scripts/install-candidate-mov.sh /tmp/test.mov
#   # ...then lock the screen (Apple menu > Lock Screen) and verify
#   # multi-cycle playback.
#
# The destination UUID is the entry we injected via wallpaper-injector;
# entries.json already points at this path so we don't need to touch it.

set -euo pipefail

DST="$HOME/Library/Application Support/com.apple.wallpaper/aerials/videos/17A6A998-4049-5A53-A08D-FD553BE57044.mov"

if [[ $# -ne 1 ]]; then
    echo "usage: $0 <candidate.mov>" >&2
    exit 2
fi
SRC="$1"
if [[ ! -f "$SRC" ]]; then
    echo "not a file: $SRC" >&2
    exit 1
fi

cp "$SRC" "$DST"
# Strip any inherited xattrs (quarantine, LastETag) that have caused
# regressions before — see HANDOFF_PLAN.md "What we've ruled out" #3.
xattr -c "$DST"
killall WallpaperAerialsExtension Wallpaper WallpaperAgent 2>/dev/null || true

echo "installed: $DST"
echo "lock the screen and verify 3+ cycles play. Log check:"
echo "  ./scripts/capture-lock-cycle-logs.sh /tmp/candidate.log 35"
echo "  grep 'enqueue PTS' /tmp/candidate.log | wc -l   # expect >> 30"
