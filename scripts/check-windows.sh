#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

SHIM_DIR=""
cleanup() {
    if [[ -n "$SHIM_DIR" && -d "$SHIM_DIR" ]]; then
        rm -rf "$SHIM_DIR"
    fi
}
trap cleanup EXIT

if ! command -v llvm-rc >/dev/null 2>&1 || ! command -v llvm-cvtres >/dev/null 2>&1; then
    SHIM_DIR="$(mktemp -d)"
    for tool in llvm-rc llvm-cvtres; do
        if command -v "${tool}-18" >/dev/null 2>&1; then
            ln -s "$(command -v "${tool}-18")" "${SHIM_DIR}/${tool}"
        elif command -v "$tool" >/dev/null 2>&1; then
            ln -s "$(command -v "$tool")" "${SHIM_DIR}/${tool}"
        else
            echo "error: missing ${tool} (or ${tool}-18) on PATH for Windows resource compilation" >&2
            exit 1
        fi
    done
    export PATH="${SHIM_DIR}:${PATH}"
fi

echo "==> Running cargo check (x86_64-pc-windows-msvc)..."
cargo check --workspace --target x86_64-pc-windows-msvc

echo "==> Running cargo clippy (x86_64-pc-windows-msvc, -D warnings)..."
cargo clippy --workspace --target x86_64-pc-windows-msvc -- -D warnings
