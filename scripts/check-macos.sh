#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

HOST_OS="$(uname -s)"

if [[ "$HOST_OS" != "Darwin" ]]; then
    if ! "$ROOT_DIR/scripts/stitch-macos-shaders.sh" "$ROOT_DIR/target/aarch64-apple-darwin"; then
        echo "==> Priming aarch64-apple-darwin check directories for gpui-pre-apple shader stitch..."
        cargo check --workspace --target aarch64-apple-darwin >/dev/null 2>&1 || true
        "$ROOT_DIR/scripts/stitch-macos-shaders.sh" "$ROOT_DIR/target/aarch64-apple-darwin" || true
    fi
fi

echo "==> Running cargo check (aarch64-apple-darwin)..."
cargo check --workspace --target aarch64-apple-darwin

if [[ "$HOST_OS" != "Darwin" ]]; then
    echo "==> Ensuring clippy out directory has stitched Metal shaders..."
    cargo clippy --workspace --target aarch64-apple-darwin -- -D warnings >/dev/null 2>&1 || true
    "$ROOT_DIR/scripts/stitch-macos-shaders.sh" "$ROOT_DIR/target/aarch64-apple-darwin" || true
fi

echo "==> Running cargo clippy (aarch64-apple-darwin, -D warnings)..."
cargo clippy --workspace --target aarch64-apple-darwin -- -D warnings
