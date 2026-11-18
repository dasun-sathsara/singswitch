#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-aarch64-apple-darwin}"
PROFILE="${2:-release}"

BIN_DIR="$ROOT_DIR/target/${TARGET}/${PROFILE}"
APP_BUNDLE="$ROOT_DIR/dist/sbctl.app"

rm -rf "$APP_BUNDLE"
mkdir -p \
    "$APP_BUNDLE/Contents/MacOS" \
    "$APP_BUNDLE/Contents/Resources"

cp "$BIN_DIR/sbctl-app" "$APP_BUNDLE/Contents/MacOS/sbctl-app"
cp "$BIN_DIR/sbctl" "$APP_BUNDLE/Contents/MacOS/sbctl"
chmod 755 "$APP_BUNDLE/Contents/MacOS/sbctl-app" "$APP_BUNDLE/Contents/MacOS/sbctl"

cp "$ROOT_DIR/resources/macos/Info.plist" "$APP_BUNDLE/Contents/Info.plist"
cp "$ROOT_DIR/assets/icons/app.icns" "$APP_BUNDLE/Contents/Resources/AppIcon.icns"

echo "APPL????" > "$APP_BUNDLE/Contents/PkgInfo"

if command -v codesign >/dev/null 2>&1; then
    codesign --force --deep --sign - "$APP_BUNDLE" || true
fi

echo "Built bundle: $APP_BUNDLE"
