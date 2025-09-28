#!/usr/bin/env bash
set -euo pipefail

# Fast verify for commit-time: format + unit tests (quick feedback)
# Use sccache when available to speed up rebuilds.
if command -v sccache >/dev/null 2>&1; then
  export RUSTC_WRAPPER="$(command -v sccache)"
fi

# Prevent test runs from mutating the repository log file.
export RUSTY_MEM_LOG_FILE="/dev/null"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

./scripts/verify.sh fmt test
