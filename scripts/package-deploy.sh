#!/usr/bin/env bash
set -euo pipefail

VERSION="${TO_DIGI_RS_VERSION:-0.7.0}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="$ROOT_DIR/target/release-bundles"
BUNDLE_NAME="to-digi-rs-deploy"
BUNDLE_DIR="$BUILD_DIR/$BUNDLE_NAME"
ARCHIVE="$BUILD_DIR/to-digi-rs-deploy-v$VERSION.tar.gz"

rm -rf "$BUNDLE_DIR"
mkdir -p "$BUNDLE_DIR/output"

install -m 0644 "$ROOT_DIR/deploy/compose.yaml" "$BUNDLE_DIR/compose.yaml"
install -m 0755 "$ROOT_DIR/deploy/import.sh" "$BUNDLE_DIR/import.sh"
install -m 0755 "$ROOT_DIR/deploy/run.sh" "$BUNDLE_DIR/run.sh"
install -m 0644 "$ROOT_DIR/deploy/config.example.toml" "$BUNDLE_DIR/config.example.toml"
install -m 0644 "$ROOT_DIR/deploy/README.md" "$BUNDLE_DIR/README.md"
touch "$BUNDLE_DIR/output/.gitkeep"

rm -f "$ARCHIVE"
tar -czf "$ARCHIVE" -C "$BUILD_DIR" "$BUNDLE_NAME"

printf '%s\n' "$ARCHIVE"
