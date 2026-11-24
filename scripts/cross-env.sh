#!/usr/bin/env bash
# Environment configuration for Linux-hosted macOS ARM64 cross-compilation.
set -euo pipefail

if [[ -z "${MAC_SDK:-}" || ! -d "$MAC_SDK" ]]; then
    echo "error: set MAC_SDK to an extracted MacOSX14.5.sdk (or compatible) directory" >&2
    exit 1
fi

export TARGET_TRIPLE="aarch64-apple-darwin"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"
export SDKROOT="$MAC_SDK"

if ! command -v ld64.lld >/dev/null 2>&1 || ! command -v llvm-ar >/dev/null 2>&1; then
    SINGSWITCH_CROSS_SHIM_DIR="$(mktemp -d)"
    export SINGSWITCH_CROSS_SHIM_DIR
    if command -v ld64.lld-18 >/dev/null 2>&1; then
        ln -sf "$(command -v ld64.lld-18)" "${SINGSWITCH_CROSS_SHIM_DIR}/ld64.lld"
    elif command -v lld-18 >/dev/null 2>&1; then
        ln -sf "$(command -v lld-18)" "${SINGSWITCH_CROSS_SHIM_DIR}/ld64.lld"
    fi
    if command -v llvm-ar-18 >/dev/null 2>&1; then
        ln -sf "$(command -v llvm-ar-18)" "${SINGSWITCH_CROSS_SHIM_DIR}/llvm-ar"
    fi
    export PATH="${SINGSWITCH_CROSS_SHIM_DIR}:${PATH}"
fi

CLANG_BIN="${CLANG_BIN:-$(command -v clang)}"
AR_BIN="${AR_BIN:-$(command -v llvm-ar || command -v llvm-ar-18)}"
# An empty path here is exported as CC_aarch64_apple_darwin and then fails much
# later inside a build script, where the cause is not obvious.
if [[ -z "$CLANG_BIN" || -z "$AR_BIN" ]]; then
    echo "error: need clang and llvm-ar (or llvm-ar-18) on PATH for the macOS cross build" >&2
    exit 1
fi

export CC_aarch64_apple_darwin="$CLANG_BIN"
export CXX_aarch64_apple_darwin="${CLANG_BIN}++"
export AR_aarch64_apple_darwin="$AR_BIN"

export CFLAGS_aarch64_apple_darwin="-target arm64-apple-macos${MACOSX_DEPLOYMENT_TARGET} -isysroot ${MAC_SDK}"
export CXXFLAGS_aarch64_apple_darwin="${CFLAGS_aarch64_apple_darwin}"

export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="$CLANG_BIN"
export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS="\
-C link-arg=-target -C link-arg=arm64-apple-macos${MACOSX_DEPLOYMENT_TARGET} \
-C link-arg=-isysroot -C link-arg=${MAC_SDK} \
-C link-arg=-fuse-ld=lld \
-C link-arg=-Wl,-adhoc_codesign"
