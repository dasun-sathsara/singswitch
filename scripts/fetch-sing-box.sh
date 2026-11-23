#!/usr/bin/env bash
# Download the pinned sing-box release, verify its checksum, and unpack it into
# a staging directory. The bundle scripts use this so the app never depends on a
# sing-box copy managed by Homebrew, WinGet, or whatever happens to be on PATH.
#
# Usage: fetch-sing-box.sh <darwin-arm64|windows-amd64> <output-dir>
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=sing-box.lock
source "$ROOT_DIR/scripts/sing-box.lock"

PLATFORM="${1:-}"
OUT_DIR="${2:-}"

if [[ -z "$PLATFORM" || -z "$OUT_DIR" ]]; then
    echo "usage: fetch-sing-box.sh <darwin-arm64|windows-amd64> <output-dir>" >&2
    exit 2
fi

case "$PLATFORM" in
    darwin-arm64) EXT="tar.gz" ;;
    windows-amd64) EXT="zip" ;;
    *)
        echo "error: unsupported platform $PLATFORM" >&2
        exit 2
        ;;
esac

HASH_VAR="SINGBOX_SHA_${PLATFORM//-/_}"
EXPECTED="${!HASH_VAR:-}"
if [[ -z "$EXPECTED" ]]; then
    echo "error: no pinned checksum for $PLATFORM in scripts/sing-box.lock" >&2
    exit 1
fi

ASSET="sing-box-${SINGBOX_VERSION}-${PLATFORM}"
ARCHIVE="${ASSET}.${EXT}"
URL="https://github.com/SagerNet/sing-box/releases/download/v${SINGBOX_VERSION}/${ARCHIVE}"
CACHE_DIR="${SINGSWITCH_SINGBOX_CACHE:-${HOME}/.cache/singswitch-sing-box}/${SINGBOX_VERSION}"
CACHED="${CACHE_DIR}/${ARCHIVE}"

mkdir -p "$CACHE_DIR" "$OUT_DIR"

if [[ ! -f "$CACHED" ]]; then
    echo "==> Downloading ${ARCHIVE}..."
    curl -fsSL --retry 3 -o "${CACHED}.part" "$URL"
    mv "${CACHED}.part" "$CACHED"
fi

if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL="$(sha256sum "$CACHED" | cut -d' ' -f1)"
else
    ACTUAL="$(shasum -a 256 "$CACHED" | cut -d' ' -f1)"
fi
if [[ "$ACTUAL" != "$EXPECTED" ]]; then
    echo "error: checksum mismatch for ${ARCHIVE}" >&2
    echo "  expected ${EXPECTED}" >&2
    echo "  actual   ${ACTUAL}" >&2
    rm -f "$CACHED"
    exit 1
fi

rm -rf "${CACHE_DIR}/${ASSET:?}"
case "$EXT" in
    tar.gz) tar -xzf "$CACHED" -C "$CACHE_DIR" ;;
    zip) unzip -q -o "$CACHED" -d "$CACHE_DIR" ;;
esac

SRC_DIR="${CACHE_DIR}/${ASSET}"
rm -rf "${OUT_DIR:?}/sing-box"
mkdir -p "$OUT_DIR/sing-box"

if [[ "$PLATFORM" == "windows-amd64" ]]; then
    cp "$SRC_DIR/sing-box.exe" "$OUT_DIR/sing-box/"
    # The official build links against libcronet.dll, so a bare exe is useless.
    if [[ -f "$SRC_DIR/libcronet.dll" ]]; then
        cp "$SRC_DIR/libcronet.dll" "$OUT_DIR/sing-box/"
    fi
else
    cp "$SRC_DIR/sing-box" "$OUT_DIR/sing-box/"
fi
cp "$SRC_DIR/LICENSE" "$OUT_DIR/sing-box/"

chmod 755 "$OUT_DIR/sing-box/"sing-box* 2>/dev/null || true

echo "==> Staged sing-box ${SINGBOX_VERSION} for ${PLATFORM} in ${OUT_DIR}/sing-box"
