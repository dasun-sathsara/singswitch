#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-aarch64-apple-darwin}"
PROFILE="${2:-release}"

case "$TARGET" in
    aarch64-apple-darwin) SINGBOX_PLATFORM="darwin-arm64" ;;
    *)
        echo "error: no pinned sing-box release for target $TARGET in scripts/sing-box.lock" >&2
        exit 1
        ;;
esac

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

echo "==> Bundling the managed sing-box binary..."
"$ROOT_DIR/scripts/fetch-sing-box.sh" "$SINGBOX_PLATFORM" "$APP_BUNDLE/Contents/Resources"

echo "APPL????" > "$APP_BUNDLE/Contents/PkgInfo"

if command -v codesign >/dev/null 2>&1; then
    # A failed ad-hoc signature leaves an app that macOS refuses to launch, so
    # this must fail the build rather than print a success line over it.
    codesign --force --deep --sign - "$APP_BUNDLE"
    codesign --verify --deep --strict "$APP_BUNDLE"
fi

echo "Built bundle: $APP_BUNDLE"
