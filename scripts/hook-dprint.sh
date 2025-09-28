#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

if ! command -v dprint >/dev/null 2>&1; then
  echo "[hook-dprint] dprint not installed. Install with 'cargo install dprint'." >&2
  exit 1
fi

exec dprint check
