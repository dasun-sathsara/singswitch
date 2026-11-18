#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

HOST_OS="$(uname -s)"
if [[ "$HOST_OS" != "Darwin" ]]; then
    # shellcheck source=cross-env.sh
    source "$ROOT_DIR/scripts/cross-env.sh"

    echo "==> Priming shader outputs for gpui-pre-apple..."
    cargo build --release --workspace --target aarch64-apple-darwin >/dev/null 2>&1 || true

    APPLE_SRC="$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -maxdepth 3 -type d -name 'gpui-pre-apple-*' | sort -V | tail -n 1 || true)"
    if [[ -n "$APPLE_SRC" ]]; then
        while IFS= read -r out_dir; do
            if command -v cbindgen >/dev/null 2>&1; then
                cbindgen --crate gpui --lang c --output "${out_dir}/scene.h" "${APPLE_SRC}/vendor/gpui" 2>/dev/null || : > "${out_dir}/scene.h"
            else
                : > "${out_dir}/scene.h"
            fi
            {
                if [[ -s "${out_dir}/scene.h" ]]; then
                    cat "${out_dir}/scene.h"
                fi
                grep -v '^#include "scene.h"' "${APPLE_SRC}/src/shaders.metal"
            } > "${out_dir}/stitched_shaders.metal"
        done < <(find "$ROOT_DIR/target/aarch64-apple-darwin/release/build" -maxdepth 5 -type d \( -path '*/gpui-pre-apple-*/out' -o -path '*/gpui-pre-apple/*/out' \) 2>/dev/null)
    fi
fi

echo "==> Building release binaries for aarch64-apple-darwin..."
cargo build --release -p sbctl-cli -p sbctl-app --target aarch64-apple-darwin

echo "==> Assembling macOS .app bundle..."
"$ROOT_DIR/scripts/bundle-macos.sh" aarch64-apple-darwin release
