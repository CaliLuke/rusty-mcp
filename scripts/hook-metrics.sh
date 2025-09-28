#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
METRICS_SCRIPT="$SCRIPT_DIR/metrics.sh"
cd "$SCRIPT_DIR/.."

if [[ ! -x "$METRICS_SCRIPT" ]]; then
  echo "[hook-metrics] scripts/metrics.sh missing or not executable" >&2
  exit 0
fi

METRICS_SOFT=1 "$METRICS_SCRIPT" || true
