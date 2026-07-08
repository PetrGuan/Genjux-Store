#!/usr/bin/env bash
#
# Genjux-Store macOS GUI end-to-end validation (#67).
#
# Compiles and runs scripts/e2e-validate.swift: a standalone (non-XCTest)
# executable that exercises the real macOS GUI networking/decoding code
# (CoreServiceClient.swift, Models.swift, ServiceLifecycle.swift) against
# real, curated GitHub repos through a real, lazily-started genjuxd --
# the same rigor as core/tests/e2e_real_repos.rs (#21), but validating the
# Swift/HTTP layer the GUI actually depends on, not just the Rust core.
#
# Deliberately NOT an XCTest target -- see the comment atop
# scripts/e2e-validate.swift for why.
#
# Usage:
#   cargo build --bin genjuxd   # from the repo root, first
#   macos/scripts/e2e-validate.sh
#
# Optional: set GENJUX_GITHUB_TOKEN to a token with no special scopes
# (public read access is enough) to avoid unauthenticated GitHub API rate
# limits (60/hr vs 5000/hr) -- e.g. `GENJUX_GITHUB_TOKEN=$(gh auth token)`
# if you have the GitHub CLI authenticated.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MACOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$MACOS_DIR/.." && pwd)"
GENJUXD_BIN="$REPO_ROOT/target/debug/genjuxd"
BIN_PATH="$(mktemp -d)/genjux-e2e-validate"

if [[ ! -x "$GENJUXD_BIN" ]]; then
  echo "error: $GENJUXD_BIN not found -- run 'cargo build --bin genjuxd' first" >&2
  exit 1
fi

echo "==> Compiling e2e-validate"
swiftc -O \
  "$MACOS_DIR/GenjuxStore/Core/ServiceInfo.swift" \
  "$MACOS_DIR/GenjuxStore/Core/ServiceLifecycle.swift" \
  "$MACOS_DIR/GenjuxStore/Core/Models.swift" \
  "$MACOS_DIR/GenjuxStore/Core/CoreServiceClient.swift" \
  "$SCRIPT_DIR/e2e-validate.swift" \
  -o "$BIN_PATH"

RUNTIME_DIR="$(mktemp -d)"
echo "==> Running against a fresh runtime dir ($RUNTIME_DIR)"
echo

set +e
GENJUX_GENJUXD_PATH="$GENJUXD_BIN" \
GENJUX_RUNTIME_DIR="$RUNTIME_DIR" \
GENJUX_GITHUB_TOKEN="${GENJUX_GITHUB_TOKEN:-}" \
  "$BIN_PATH"
STATUS=$?
set -e

rm -rf "$RUNTIME_DIR" "$(dirname "$BIN_PATH")"
exit $STATUS
