#!/usr/bin/env bash
# Build a Crane AppImage from an already-built release binary.
#
# Expects:
#   - target/release/crane            (from `cargo build --release`)
#   - crane.png                        (app icon)
#   - scripts/linux/crane.desktop      (desktop entry)
#
# Produces:
#   - Crane-${VERSION}-x86_64.AppImage at the repo root
#
# linuxdeploy is downloaded into a .build-cache/ directory so repeat
# runs don't re-fetch it.
set -euo pipefail

VERSION="${1:?usage: build-appimage.sh <version>}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CACHE="$ROOT/.build-cache"
APPDIR="$ROOT/target/appdir"

mkdir -p "$CACHE"

# linuxdeploy — single binary, bundles dependencies and wraps them into
# a squashfs-based AppImage. Pinned to latest continuous release so
# builds are reproducible against a known URL shape.
LD="$CACHE/linuxdeploy-x86_64.AppImage"
if [[ ! -x "$LD" ]]; then
  echo "downloading linuxdeploy..."
  curl -L -o "$LD" \
    "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
  chmod +x "$LD"
fi

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/icons/hicolor/512x512/apps"

cp "$ROOT/target/release/crane" "$APPDIR/usr/bin/crane"
cp "$ROOT/scripts/linux/crane.desktop" "$APPDIR/usr/share/applications/crane.desktop"
cp "$ROOT/crane.png" "$APPDIR/usr/share/icons/hicolor/512x512/apps/crane.png"
# linuxdeploy also wants a copy at the AppDir root for icon discovery.
cp "$ROOT/crane.png" "$APPDIR/crane.png"

cd "$ROOT"
OUTPUT="Crane-${VERSION}-x86_64.AppImage" \
  "$LD" --appdir "$APPDIR" --output appimage \
        --desktop-file "$APPDIR/usr/share/applications/crane.desktop" \
        --icon-file "$APPDIR/crane.png"

echo "built: $ROOT/Crane-${VERSION}-x86_64.AppImage"
