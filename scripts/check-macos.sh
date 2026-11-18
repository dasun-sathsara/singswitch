#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

HOST_OS="$(uname -s)"

stitch_gpui_apple_shaders() {
    local apple_src
    apple_src="$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -maxdepth 3 -type d -name 'gpui-pre-apple-*' | sort -V | tail -n 1 || true)"
    if [[ -z "$apple_src" || ! -d "$apple_src" ]]; then
        return 0
    fi

    local gpui_vendor="${apple_src}/vendor/gpui"
    local shaders_src="${apple_src}/src/shaders.metal"
    if [[ ! -d "$gpui_vendor" || ! -f "$shaders_src" ]]; then
        echo "warning: unexpected gpui-pre-apple layout at $apple_src" >&2
        return 0
    fi

    local out_dirs=()
    while IFS= read -r dir; do
        out_dirs+=("$dir")
    done < <(find "$ROOT_DIR/target/aarch64-apple-darwin" -maxdepth 6 -type d \( -path '*/build/gpui-pre-apple-*/out' -o -path '*/build/gpui-pre-apple/*/out' \) 2>/dev/null || true)

    if [[ ${#out_dirs[@]} -eq 0 ]]; then
        return 1
    fi

    for out_dir in "${out_dirs[@]}"; do
        local stitched="${out_dir}/stitched_shaders.metal"
        local header="${out_dir}/scene.h"

        if [[ ! -f "$header" || "$gpui_vendor/src/scene.rs" -nt "$header" ]]; then
            if command -v cbindgen >/dev/null 2>&1; then
                cbindgen --config "${gpui_vendor}/cbindgen.toml" --crate gpui --output "$header" "$gpui_vendor" 2>/dev/null || \
                cbindgen --crate gpui --lang c --output "$header" "$gpui_vendor" 2>/dev/null || \
                : > "$header"
            else
                : > "$header"
            fi
        fi

        if [[ ! -f "$stitched" || "$shaders_src" -nt "$stitched" ]]; then
            {
                if [[ -s "$header" ]]; then
                    cat "$header"
                fi
                grep -v '^#include "scene.h"' "$shaders_src"
            } > "$stitched"
        fi
    done
    return 0
}

if [[ "$HOST_OS" != "Darwin" ]]; then
    if ! stitch_gpui_apple_shaders; then
        echo "==> Priming aarch64-apple-darwin check directories for gpui-pre-apple shader stitch..."
        cargo check --workspace --target aarch64-apple-darwin >/dev/null 2>&1 || true
        stitch_gpui_apple_shaders || true
    fi
fi

echo "==> Running cargo check (aarch64-apple-darwin)..."
cargo check --workspace --target aarch64-apple-darwin

if [[ "$HOST_OS" != "Darwin" ]]; then
    echo "==> Ensuring clippy out directory has stitched Metal shaders..."
    cargo clippy --workspace --target aarch64-apple-darwin -- -D warnings >/dev/null 2>&1 || true
    stitch_gpui_apple_shaders || true
fi

echo "==> Running cargo clippy (aarch64-apple-darwin, -D warnings)..."
cargo clippy --workspace --target aarch64-apple-darwin -- -D warnings
