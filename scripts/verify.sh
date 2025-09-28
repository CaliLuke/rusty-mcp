#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/verify.sh [STEP...]

Runs the requested verification steps. When no steps are provided, the default
suite (fmt, clippy, test) is executed.

Available steps:
  fmt      - Run cargo fmt --all --check
  clippy   - Run cargo clippy with warnings promoted to errors
  test     - Run cargo test
  doc      - Run cargo doc --no-deps with RUSTDOCFLAGS="-D warnings"

Examples:
  scripts/verify.sh          # run default suite (fmt, clippy, test)
  scripts/verify.sh doc      # only run documentation checks
  scripts/verify.sh fmt test # run a subset
USAGE
}

if [[ ${1-} == "-h" || ${1-} == "--help" ]]; then
  usage
  exit 0
fi

DEFAULT_STEPS=(fmt clippy test)
AVAILABLE_STEPS=(fmt clippy test doc)

declare -a REQUESTED_STEPS=()
if [[ $# -eq 0 ]]; then
  REQUESTED_STEPS=("${DEFAULT_STEPS[@]}")
else
  for step in "$@"; do
    found=false
    for candidate in "${AVAILABLE_STEPS[@]}"; do
      if [[ $step == "$candidate" ]]; then
        REQUESTED_STEPS+=("$step")
        found=true
        break
      fi
    done
    if [[ $found == false ]]; then
      echo "[verify] unknown step: $step" >&2
      echo >&2
      usage >&2
      exit 1
    fi
  done
fi

failures=()

log() {
  echo "[verify] $*"
}

run_step() {
  local name="$1"
  shift
  log "$name"
  if "$@"; then
    log "$name ✔"
  else
    log "$name ✖"
    failures+=("$name")
  fi
}

step_fmt() {
  cargo fmt --all --check
}

step_clippy() {
  cargo clippy --all-targets --all-features -- -D warnings
}

step_test() {
  cargo test
}

step_doc() {
  local rustdocflags_was_set=1
  if [[ -z ${RUSTDOCFLAGS+x} ]]; then
    rustdocflags_was_set=0
  fi
  local original_rustdocflags="${RUSTDOCFLAGS-}"
  export RUSTDOCFLAGS="${RUSTDOCFLAGS:--D warnings}"
  cargo doc --no-deps
  if [[ $rustdocflags_was_set -eq 0 ]]; then
    unset RUSTDOCFLAGS
  else
    export RUSTDOCFLAGS="$original_rustdocflags"
  fi
}

for step in "${REQUESTED_STEPS[@]}"; do
  run_step "$step" "step_${step}"
done

if [[ ${#failures[@]} -ne 0 ]]; then
  printf '[verify] failed steps: %s\n' "${failures[*]}" >&2
  exit 1
fi

log "all checks passed"
