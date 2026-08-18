#!/usr/bin/env bash
set -euo pipefail

VERSION="${TO_DIGI_RS_VERSION:-0.9.0}"
DEFAULT_IMAGE="ghcr.io/johed-velca/to-digi-rs:0.9.0"
PACKAGE_IMAGE="${TO_DIGI_RS_PACKAGE_IMAGE:-${TO_DIGI_RS_RELEASE_IMAGE:-ghcr.io/johed-velca/to-digi-rs:$VERSION}}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="$ROOT_DIR/target/release-bundles"
BUNDLE_NAME="to-digi-rs-deploy"
BUNDLE_DIR="$BUILD_DIR/$BUNDLE_NAME"
ARCHIVE="$BUILD_DIR/to-digi-rs-deploy-v$VERSION.tar.gz"

render_asset() {
    local source="$1"
    local target="$2"
    local mode="$3"
    sed "s|$DEFAULT_IMAGE|$PACKAGE_IMAGE|g" "$source" >"$target"
    chmod "$mode" "$target"
}

rm -rf "$BUNDLE_DIR"
mkdir -p "$BUNDLE_DIR/output" "$BUNDLE_DIR/profiles"

render_asset "$ROOT_DIR/deploy/compose.yaml" "$BUNDLE_DIR/compose.yaml" 0644
render_asset "$ROOT_DIR/deploy/to-digi" "$BUNDLE_DIR/to-digi" 0755
install -m 0755 "$ROOT_DIR/deploy/import.sh" "$BUNDLE_DIR/import.sh"
install -m 0755 "$ROOT_DIR/deploy/run.sh" "$BUNDLE_DIR/run.sh"
install -m 0644 "$ROOT_DIR/deploy/config.example.toml" "$BUNDLE_DIR/config.example.toml"
render_asset "$ROOT_DIR/deploy/README.md" "$BUNDLE_DIR/README.md" 0644
install -m 0644 "$ROOT_DIR/profiles/example.toml" "$BUNDLE_DIR/profiles/example.toml"
install -m 0644 "$ROOT_DIR/profiles/bigway.toml" "$BUNDLE_DIR/profiles/bigway.toml"
install -m 0644 "$ROOT_DIR/profiles/starsky.toml" "$BUNDLE_DIR/profiles/starsky.toml"
touch "$BUNDLE_DIR/output/.gitkeep"

rm -f "$ARCHIVE"
tar -czf "$ARCHIVE" -C "$BUILD_DIR" "$BUNDLE_NAME"

printf '%s\n' "$ARCHIVE"
