#!/usr/bin/env bash
# Vendor libpdfium.dylib for the PDF viewer Pane.
#
# Downloads pre-built binaries from bblanchon/pdfium-binaries pinned
# to the chromium tag whose ABI matches pdfium-render's `pdfium_latest`
# feature. Bump PDFIUM_TAG together with the pdfium-render version in
# Cargo.toml — the API and ABI move in lockstep.
#
# Layout produced:
#   vendor/pdfium/arm64/libpdfium.dylib
#   vendor/pdfium/x86_64/libpdfium.dylib
#
# Idempotent: re-running with the same tag is a no-op.

set -euo pipefail

# Pinned to match pdfium-render 0.9.x → pdfium_7763 ABI.
# Bump alongside Cargo.toml's pdfium-render version.
PDFIUM_TAG="${PDFIUM_TAG:-chromium/7763}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR_DIR="$REPO_ROOT/vendor/pdfium"
STAMP="$VENDOR_DIR/.pinned-tag"

mkdir -p "$VENDOR_DIR/arm64" "$VENDOR_DIR/x86_64"

if [[ -f "$STAMP" ]] && [[ "$(cat "$STAMP")" == "$PDFIUM_TAG" ]] \
        && [[ -f "$VENDOR_DIR/arm64/libpdfium.dylib" ]] \
        && [[ -f "$VENDOR_DIR/x86_64/libpdfium.dylib" ]]; then
    echo "pdfium $PDFIUM_TAG already vendored"
    exit 0
fi

fetch_arch() {
    local arch_dir="$1"
    local asset="$2"
    local url="https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_TAG}/${asset}"
    local tmp
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' RETURN

    echo "fetching $asset ..."
    curl --fail --location --silent --show-error --output "$tmp/pdfium.tgz" "$url"
    tar -xzf "$tmp/pdfium.tgz" -C "$tmp"
    cp "$tmp/lib/libpdfium.dylib" "$VENDOR_DIR/$arch_dir/libpdfium.dylib"
    chmod 0644 "$VENDOR_DIR/$arch_dir/libpdfium.dylib"
}

fetch_arch arm64 pdfium-mac-arm64.tgz
fetch_arch x86_64 pdfium-mac-x64.tgz

echo "$PDFIUM_TAG" > "$STAMP"
echo "vendored: $VENDOR_DIR (tag $PDFIUM_TAG)"
