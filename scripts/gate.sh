#!/usr/bin/env bash
# Gate script: the single source of truth for "is this repo acceptable".
# Runs in CI and locally. Exit 0 = green, anything else = red.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() { echo "CHECK-FAILED: $*" >&2; exit 1; }

echo "==> fmt check"
cargo fmt --all -- --check || fail "cargo fmt --check"

echo "==> clippy (all targets, -D warnings)"
cargo clippy --workspace --all-targets -- -D warnings || fail "clippy"

echo "==> icon audit"
./scripts/check-icons.py || fail "icon audit"

echo "==> build"
cargo build --workspace || fail "build"

# Guard: the workspace lint thresholds must not be weakened. cargo-clippy
# reads these; removing them silently downgrades the CI gate. We check the
# workspace Cargo.toml still defines our baseline lints.
if ! grep -q 'unused_must_use = "deny"' Cargo.toml 2>/dev/null; then
    fail "workspace lint baseline (unused_must_use=deny) missing from Cargo.toml — see AGENTS.md exception process"
fi

echo "==> ALL CHECKS PASSED"
