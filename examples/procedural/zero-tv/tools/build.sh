#!/usr/bin/env bash
# Builds the images of the Zero TV pack from zerotv.mp4:
#   back.png   the back of the shelf, without the tapes
#   font.png   the font of the labels, from the glyphs in font.py
#
# Usage: tools/build.sh [VIDEO]. It needs ffmpeg and uv. uv gets Pillow and
# numpy. The video streams one frame at a time, so memory stays small.
set -euo pipefail
pack=$(cd "$(dirname "$0")/.." && pwd)
video=${1:-$pack/zerotv.mp4}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
run() { uv run -q --with pillow --with numpy python -I "$@"; }
run "$pack/tools/average.py" "$video" "$work"
run "$pack/tools/rebuild.py" "$work" "$pack" >/dev/null
run "$pack/tools/font.py" "$pack/font.png"
