#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

if ! command -v cargo-spellcheck >/dev/null 2>&1; then
  echo "[hook-spellcheck] cargo-spellcheck not installed. Install with 'cargo install cargo-spellcheck'." >&2
  exit 0
fi

exec cargo spellcheck --cfg docsrs
