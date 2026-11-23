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

echo "==> Building release binaries for aarch64-apple-darwin..."
cargo build --release -p singswitch-cli -p singswitch-app --target aarch64-apple-darwin

echo "==> Assembling macOS .app bundle..."
"$ROOT_DIR/scripts/bundle-macos.sh" aarch64-apple-darwin release
