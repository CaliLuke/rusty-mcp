#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

if ! command -v taplo >/dev/null 2>&1; then
  echo "[hook-taplo] taplo CLI not found. Install with 'cargo install taplo-cli'." >&2
  exit 1
fi

if [[ $# -eq 0 ]]; then
  mapfile -t tracked_toml < <(git ls-files '*.toml')
  if [[ ${#tracked_toml[@]} -eq 0 ]]; then
    exit 0
  fi
  set -- "${tracked_toml[@]}"
fi

# taplo exits non-zero when formatting differs; that's desirable for --check.
exec taplo fmt --check "$@"
