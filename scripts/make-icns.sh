#!/usr/bin/env bash
# Generate icons/crane.icns from crane.png at all macOS resolutions.
set -euo pipefail

cd "$(dirname "$0")/.."

SRC="crane.png"
OUT_DIR="icons"
SET_DIR="$OUT_DIR/crane.iconset"
OUT="$OUT_DIR/crane.icns"

if [[ ! -f "$SRC" ]]; then
  echo "error: $SRC not found" >&2
  exit 1
fi

mkdir -p "$SET_DIR"

sips -z 16   16   "$SRC" --out "$SET_DIR/icon_16x16.png"    >/dev/null
sips -z 32   32   "$SRC" --out "$SET_DIR/icon_16x16@2x.png" >/dev/null
sips -z 32   32   "$SRC" --out "$SET_DIR/icon_32x32.png"    >/dev/null
sips -z 64   64   "$SRC" --out "$SET_DIR/icon_32x32@2x.png" >/dev/null
sips -z 128  128  "$SRC" --out "$SET_DIR/icon_128x128.png"  >/dev/null
sips -z 256  256  "$SRC" --out "$SET_DIR/icon_128x128@2x.png" >/dev/null
sips -z 256  256  "$SRC" --out "$SET_DIR/icon_256x256.png"  >/dev/null
sips -z 512  512  "$SRC" --out "$SET_DIR/icon_256x256@2x.png" >/dev/null
sips -z 512  512  "$SRC" --out "$SET_DIR/icon_512x512.png"  >/dev/null
sips -z 1024 1024 "$SRC" --out "$SET_DIR/icon_512x512@2x.png" >/dev/null

iconutil -c icns "$SET_DIR" -o "$OUT"
echo "wrote $OUT"
