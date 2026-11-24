#!/usr/bin/env bash
# Pre-commit hook: refuses commits that don't pass the repository gate.
# Install (or repair after a fresh clone) by running scripts/gate.sh once,
# then: ln -s ../../scripts/pre-commit .git/hooks/pre-commit
set -euo pipefail
# The hook is symlinked from .git/hooks/pre-commit, so resolve the real
# script location rather than assuming the CWD is the repo root.
cd "$(dirname "$(readlink -f "$0")")/.."

echo "==> pre-commit gate"
./scripts/gate.sh
