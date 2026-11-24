#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

HOST_OS="$(uname -s)"
if [[ "$HOST_OS" != "Darwin" ]]; then
    # shellcheck source=cross-env.sh
    source "$ROOT_DIR/scripts/cross-env.sh"
    trap '[[ -n "${SINGSWITCH_CROSS_SHIM_DIR:-}" && -d "$SINGSWITCH_CROSS_SHIM_DIR" ]] && rm -rf "$SINGSWITCH_CROSS_SHIM_DIR"' EXIT

    echo "==> Priming shader outputs for gpui-pre-apple..."
    cargo build --release --workspace --target aarch64-apple-darwin >/dev/null 2>&1 || true
    "$ROOT_DIR/scripts/stitch-macos-shaders.sh" "$ROOT_DIR/target/aarch64-apple-darwin/release"
fi

APP_FEATURES=()
FONT_DIR=""
if [[ -f "$ROOT_DIR/assets/fonts/FeixenStandard-Regular.ttf" && -f "$ROOT_DIR/assets/fonts/FeixenStandard-Bold.ttf" ]]; then
    echo "==> Bundling Studio Feixen fonts for interface..."
    APP_FEATURES+=(--features feixen)
elif [[ -d "$ROOT_DIR/../../config/personal/custom-fonts/release/standard/ttf" ]]; then
    FONT_DIR="$ROOT_DIR/../../config/personal/custom-fonts/release/standard/ttf"
elif [[ -d "/home/boxd/CloudHome/config/personal/custom-fonts/release/standard/ttf" ]]; then
    FONT_DIR="/home/boxd/CloudHome/config/personal/custom-fonts/release/standard/ttf"
elif [[ -d "$HOME/Home/config/personal/custom-fonts/release/standard/ttf" ]]; then
    FONT_DIR="$HOME/Home/config/personal/custom-fonts/release/standard/ttf"
fi

if [[ -n "$FONT_DIR" && -f "$FONT_DIR/FeixenStandard-Regular.ttf" && -f "$FONT_DIR/FeixenStandard-Bold.ttf" ]]; then
    cp "$FONT_DIR/FeixenStandard-Regular.ttf" "$ROOT_DIR/assets/fonts/"
    cp "$FONT_DIR/FeixenStandard-Bold.ttf" "$ROOT_DIR/assets/fonts/"
    echo "==> Staged and bundling Studio Feixen fonts..."
    APP_FEATURES+=(--features feixen)
fi

echo "==> Building release binaries for aarch64-apple-darwin..."
cargo build --release -p singswitch-cli --target aarch64-apple-darwin
# Guarded expansion: macOS ships bash 3.2, where `set -u` plus `"${arr[@]}"` on
# an empty array aborts with "unbound variable", which is every build without
# the Feixen cut.
cargo build --release -p singswitch-app ${APP_FEATURES[@]+"${APP_FEATURES[@]}"} --target aarch64-apple-darwin

echo "==> Assembling macOS .app bundle..."
"$ROOT_DIR/scripts/bundle-macos.sh" aarch64-apple-darwin release
