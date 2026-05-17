#!/usr/bin/env bash
# Assemble Phonto.saver from the phonto-screensaver cdylib.
#
# Usage:
#   ./screensaver/build-saver.sh [path-to-video.mp4]
#
# Output: ./screensaver/Phonto.saver  (drop into ~/Library/Screen Savers/)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUNDLE="$SCRIPT_DIR/Phonto.saver"
PROFILE="${PROFILE:-release}"
CARGO_FLAGS=()
if [ "$PROFILE" = "release" ]; then
    CARGO_FLAGS+=(--release)
fi

VIDEO_SRC="${1:-}"

echo "→ building phonto-screensaver ($PROFILE)"
cargo build --manifest-path "$WORKSPACE_ROOT/Cargo.toml" \
    -p phonto-screensaver "${CARGO_FLAGS[@]}"

DYLIB="$WORKSPACE_ROOT/target/$PROFILE/libPhontoScreenSaver.dylib"
if [ ! -f "$DYLIB" ]; then
    echo "expected $DYLIB to exist after build" >&2
    exit 1
fi

echo "→ assembling $BUNDLE"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
cp "$SCRIPT_DIR/Info.plist" "$BUNDLE/Contents/Info.plist"
cp "$DYLIB" "$BUNDLE/Contents/MacOS/PhontoScreenSaver"

if [ -n "$VIDEO_SRC" ]; then
    if [ ! -f "$VIDEO_SRC" ]; then
        echo "video not found: $VIDEO_SRC" >&2
        exit 1
    fi
    echo "→ embedding video: $VIDEO_SRC"
    cp "$VIDEO_SRC" "$BUNDLE/Contents/Resources/wallpaper.mp4"
else
    echo "  (no video provided — drop one at $BUNDLE/Contents/Resources/wallpaper.mp4 before installing)"
fi

# Ad-hoc sign so legacyScreenSaver will load it on a fresh machine without
# a Developer ID. For distribution, replace with: codesign --sign "Developer ID Application: ..."
echo "→ ad-hoc signing"
codesign --force --sign - --timestamp=none "$BUNDLE"

echo
echo "Built: $BUNDLE"
echo
echo "Install with:"
echo "  cp -R \"$BUNDLE\" ~/Library/Screen\\ Savers/"
echo "Then open System Settings → Screen Saver and pick Phonto."
