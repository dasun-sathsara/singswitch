#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${1:-$ROOT_DIR/target/aarch64-apple-darwin}"

APPLE_SRC="$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -maxdepth 3 -type d -name 'gpui-pre-apple-*' | sort -V | tail -n 1 || true)"
if [[ -z "$APPLE_SRC" || ! -d "$APPLE_SRC" ]]; then
    exit 0
fi

GPUI_VENDOR="${APPLE_SRC}/vendor/gpui"
SHADERS_SRC="${APPLE_SRC}/src/shaders.metal"
if [[ ! -d "$GPUI_VENDOR" || ! -f "$SHADERS_SRC" ]]; then
    echo "warning: unexpected gpui-pre-apple layout at $APPLE_SRC" >&2
    exit 0
fi

OUT_DIRS=()
while IFS= read -r dir; do
    OUT_DIRS+=("$dir")
done < <(find "$TARGET_DIR" -maxdepth 6 -type d \( -path '*/build/gpui-pre-apple-*/out' -o -path '*/build/gpui-pre-apple/*/out' \) 2>/dev/null || true)

if [[ ${#OUT_DIRS[@]} -eq 0 ]]; then
    exit 1
fi

# Cross builds run gpui-pre-apple's build script on the Linux host, where its
# macOS-only shader step is skipped, so the header has to be generated here.
# Shipping without it yields an app that opens no window and reports
# "building Metal library" at runtime.
HEADER_GEN="$ROOT_DIR/scripts/shader-header"

for out_dir in "${OUT_DIRS[@]}"; do
    stitched="${out_dir}/stitched_shaders.metal"
    header="${out_dir}/scene.h"

    if [[ ! -s "$header" || "$GPUI_VENDOR/src/scene.rs" -nt "$header" ]]; then
        cargo run --quiet --release --manifest-path "$HEADER_GEN/Cargo.toml" -- \
            "$GPUI_VENDOR" "$APPLE_SRC" "$header"
    fi

    if [[ ! -s "$header" ]]; then
        echo "error: shader header $header is empty" >&2
        exit 1
    fi

    {
        cat "$header"
        grep -v '^#include "scene.h"' "$SHADERS_SRC"
    } > "$stitched"

    if ! grep -q 'Bounds_ScaledPixels' "$stitched"; then
        echo "error: $stitched is missing the shader structs from scene.h" >&2
        exit 1
    fi
done
