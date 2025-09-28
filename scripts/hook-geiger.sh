#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

log() {
  echo "[hook-geiger] $*"
}

if ! command -v cargo-geiger >/dev/null 2>&1; then
  log "cargo-geiger is not installed. Install with 'cargo install cargo-geiger'."
  log "You can temporarily skip this hook with PRE_COMMIT_ALLOW_NO_CONFIG=1 if needed."
  exit 1
fi

run_cargo() {
  local toolchain="${GEIGER_TOOLCHAIN_NAME-}"
  if [[ -n "$toolchain" ]]; then
    if command -v rustup >/dev/null 2>&1; then
      rustup run "$toolchain" cargo geiger "$@"
      return
    else
      log "GEIGER_TOOLCHAIN_NAME is set but rustup is unavailable; falling back to default toolchain."
    fi
  fi
  cargo geiger "$@"
}

log "Running cargo geiger --forbid-only --package rusty-mem"
run_cargo --forbid-only --package rusty-mem
