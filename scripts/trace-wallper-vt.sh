#!/bin/bash
# Launch the re-signed /tmp/Wallper.app under lldb with VideoToolbox tracing.
#
# Usage:
#   scripts/trace-wallper-vt.sh
#
# Then in the Wallper UI:
#   1. Pick a local video file.
#   2. Set it as your wallpaper.
#   3. Watch this terminal — VTCompressionSessionCreate / VTSessionSetProperty
#      calls will stream as they fire.
#   4. When done, type `quit` at the (lldb) prompt or press ^C then `quit`.
#
# All output is also tee'd to /tmp/wallper-vt-trace.log.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP=/tmp/Wallper.app
LOG=/tmp/wallper-vt-trace.log

if [ ! -d "$APP" ]; then
  echo "error: $APP not found — re-sign step missing" >&2
  exit 1
fi

# Make sure no other Wallper is running, otherwise lldb may attach to the
# wrong one or the new one will exit silently due to single-instance lock.
pkill -x Wallper 2>/dev/null || true
sleep 0.5

echo "Tracing $APP under lldb. Log -> $LOG"
echo "Trigger a transcode in Wallper's UI when it opens."
echo

exec lldb -s "$SCRIPT_DIR/trace-wallper-vt.lldb" 2>&1 | tee "$LOG"
