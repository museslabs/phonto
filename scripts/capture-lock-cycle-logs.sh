#!/usr/bin/env bash
# Capture os_log output across the processes + subsystems that actually
# render aerial wallpapers on the lock screen, for a fixed window. Run this
# once with Wallper's working mov installed as your wallpaper, and once
# with our broken mov, then diff the two outputs — the divergence is the
# concrete signal we've been missing from static-file comparison.
#
# Captures:
#   - process: WallpaperAerialsExtension (the actual aerial player),
#     WallpaperAgent (the orchestrator), loginwindow (lock-screen host),
#     Wallpaper (Settings extension)
#   - subsystem: com.apple.wallpaper, com.apple.coremedia, com.apple.AVFCore,
#     com.apple.videotoolbox — covers WP plumbing AND the underlying
#     media/decoder layers where the actual playback failure would surface
#
# Usage:
#   scripts/capture-lock-cycle-logs.sh <output-file> [duration-sec]
#
# The script runs `log stream` for the given duration. While it's running,
# you should: lock the screen (Apple menu -> Lock Screen), wait ~3s, unlock,
# wait ~3s, lock again, unlock — repeat until duration elapses.

set -u

OUT="${1:-}"
DUR="${2:-30}"

if [ -z "$OUT" ]; then
    echo "usage: $0 <output-file> [duration-sec]" >&2
    exit 2
fi

echo "Capturing wallpaper / media / VT logs for $DUR seconds to $OUT"
echo "While this runs: lock the screen (Apple menu -> Lock Screen), unlock,"
echo "and repeat the cycle 3+ times before the timer expires."
echo

PRED='process == "WallpaperAerialsExtension"
    OR process == "WallpaperAgent"
    OR process == "loginwindow"
    OR process == "Wallpaper"
    OR subsystem == "com.apple.wallpaper"
    OR subsystem == "com.apple.coremedia"
    OR subsystem == "com.apple.AVFCore"
    OR subsystem == "com.apple.videotoolbox"
    OR subsystem == "com.apple.avplayer"'

# `log stream` doesn't accept a duration directly, so run it in the
# background and kill it after $DUR seconds.
log stream --predicate "$PRED" --info --debug --style compact > "$OUT" 2>&1 &
LOG_PID=$!

# Show a countdown so the user knows how long they have to do lock cycles.
for ((i = DUR; i > 0; i--)); do
    printf "\r  %ds left — do lock/unlock cycles now           " "$i"
    sleep 1
done
echo

kill "$LOG_PID" 2>/dev/null
wait "$LOG_PID" 2>/dev/null

LINES=$(wc -l < "$OUT" | tr -d ' ')
echo "captured $LINES log lines -> $OUT"
