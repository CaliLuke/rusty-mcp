#!/usr/bin/env bash
set -euo pipefail

REPORTS_DIR="reports"
SOFT_RUN="${METRICS_SOFT:-0}"

if [ "$SOFT_RUN" = "1" ]; then
  REPORTS_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t rusty_mem_metrics)"
  cleanup_reports() {
    rm -rf "$REPORTS_DIR"
  }
  trap cleanup_reports EXIT
else
  mkdir -p "$REPORTS_DIR"
fi

declare -a SUMMARY

log() {
  echo "[metrics] $*"
}

record_summary() {
  SUMMARY+=("$1")
}

have_rustup() {
  command -v rustup >/dev/null 2>&1
}

toolchain_installed() {
  local name="$1"
  have_rustup && rustup toolchain list | grep -q "^$name"
}

component_installed() {
  local component="$1"
  have_rustup && rustup component list --installed | grep -Eq "${component}"
}

CARGO_BIN="${CARGO_BIN:-$HOME/.cargo/bin/cargo}"
if [ ! -x "$CARGO_BIN" ]; then
  CARGO_BIN="$(command -v cargo)"
fi

run_cargo() {
  local toolchain="$1"
  shift
  if [ -n "$toolchain" ] && have_rustup; then
    rustup run "$toolchain" "$CARGO_BIN" "$@"
  else
    "$CARGO_BIN" "$@"
  fi
}

run_metrics_post() {
  "$CARGO_BIN" run --quiet --bin metrics-post -- "$@"
}

GEIGER_TOOLCHAIN_NAME=${GEIGER_TOOLCHAIN_NAME:-nightly}
UDEPS_TOOLCHAIN_NAME=${UDEPS_TOOLCHAIN_NAME:-nightly}

if [ "$SOFT_RUN" = "1" ]; then
  log "Skipping coverage (soft run)"
  record_summary "coverage: skipped (soft run)"
elif command -v cargo-llvm-cov >/dev/null 2>&1; then
  if ! component_installed "llvm-tools"; then
    log "Skipping coverage (install with: rustup component add llvm-tools-preview)"
    record_summary "coverage: skipped (install llvm-tools-preview)"
  else
    log "Running cargo llvm-cov (summary only)"
    if [ -z "${LLVM_COV:-}" ] || [ ! -x "${LLVM_COV:-}" ] || [ -z "${LLVM_PROFDATA:-}" ] || [ ! -x "${LLVM_PROFDATA:-}" ]; then
      active_toolchain="${RUSTUP_TOOLCHAIN:-$(rustup show active-toolchain 2>/dev/null | awk 'NR==1 {print $1}')}"
      if [ -z "$active_toolchain" ]; then
        active_toolchain=$(rustup default 2>/dev/null | awk 'NR==1 {print $1}')
      fi
      host_triple=$(rustc -Vv | awk '/host/ {print $2}')
      tools_dir="$HOME/.rustup/toolchains/${active_toolchain}/lib/rustlib/${host_triple}/bin"
      if [ -x "$tools_dir/llvm-cov" ] && [ -x "$tools_dir/llvm-profdata" ]; then
        export LLVM_COV="$tools_dir/llvm-cov"
        export LLVM_PROFDATA="$tools_dir/llvm-profdata"
      fi
    fi
    if cargo llvm-cov --summary-only --json --output-path "$REPORTS_DIR/coverage.json"; then
      if run_metrics_post coverage --input "$REPORTS_DIR/coverage.json" --output "$REPORTS_DIR/coverage.txt"; then
        record_summary "coverage: ok"
      else
        log "coverage summary generation failed"
        record_summary "coverage: failed (summary)"
      fi
    else
      log "coverage command failed"
      record_summary "coverage: failed"
    fi
  fi
else
  log "Skipping coverage (cargo-llvm-cov not installed)"
  record_summary "coverage: skipped"
fi

if [ "$SOFT_RUN" = "1" ]; then
  log "Skipping geiger (soft run)"
  record_summary "geiger: skipped (soft run)"
elif command -v cargo-geiger >/dev/null 2>&1; then
  if [ -n "$GEIGER_TOOLCHAIN_NAME" ] && ! toolchain_installed "$GEIGER_TOOLCHAIN_NAME"; then
    log "Skipping geiger (install toolchain: rustup toolchain install ${GEIGER_TOOLCHAIN_NAME})"
    record_summary "geiger: skipped (missing ${GEIGER_TOOLCHAIN_NAME})"
  else
    log "Running cargo geiger"
    tmp_raw=$(mktemp)
    if run_cargo "$GEIGER_TOOLCHAIN_NAME" geiger --package rusty-mem --include-tests --output-format Json > "$tmp_raw" 2>&1; then
      if run_metrics_post geiger --input "$tmp_raw" --output "$REPORTS_DIR/geiger.md" --crate-name rusty-mem; then
        record_summary "geiger: ok"
      else
        log "cargo geiger output could not be parsed"
        record_summary "geiger: failed"
      fi
    else
      log "cargo geiger failed"
      cat <<'EOF' > "$REPORTS_DIR/geiger.md"
# Unsafe Code Report

`cargo geiger` failed. Run the tool manually for diagnostics.
EOF
      record_summary "geiger: failed"
    fi
    rm -f "$tmp_raw"
  fi
else
  log "Skipping unsafe analysis (cargo-geiger not installed)"
  record_summary "geiger: skipped"
fi

if command -v tokei >/dev/null 2>&1; then
  log "Running tokei"
  if tokei --output json --exclude "$REPORTS_DIR" --exclude target . > "$REPORTS_DIR/tokei.json"; then
    if run_metrics_post tokei --input "$REPORTS_DIR/tokei.json" --output "$REPORTS_DIR/loc.txt"; then
      record_summary "tokei: ok"
    else
      log "tokei summary generation failed"
      record_summary "tokei: failed"
    fi
  else
    log "tokei failed"
    record_summary "tokei: failed"
  fi
else
  log "Skipping tokei (tokei not installed)"
  record_summary "tokei: skipped"
fi

if command -v rust-code-analysis-cli >/dev/null 2>&1; then
  log "Running rust-code-analysis (maintainability & complexity)"
  RCA_DIR="$REPORTS_DIR/rust-code-analysis"
  rm -rf "$RCA_DIR"
  mkdir -p "$RCA_DIR"
  if rust-code-analysis-cli -m -O json -p src -o "$RCA_DIR"; then
    if run_metrics_post rca --input "$RCA_DIR" --output "$RCA_DIR/summary.md"; then
      record_summary "rust-code-analysis: ok"
    else
      log "rust-code-analysis summary generation failed"
      record_summary "rust-code-analysis: failed"
    fi
  else
    log "rust-code-analysis-cli failed"
    record_summary "rust-code-analysis: failed"
  fi
else
  log "Skipping rust-code-analysis (rust-code-analysis-cli not installed)"
  record_summary "rust-code-analysis: skipped"
fi

if command -v debtmap >/dev/null 2>&1; then
  log "Running debtmap (technical debt)"
  if debtmap analyze --format json src > "$REPORTS_DIR/debtmap.json"; then
    if run_metrics_post debtmap --input "$REPORTS_DIR/debtmap.json" --output "$REPORTS_DIR/debtmap.md"; then
      record_summary "debtmap: ok"
    else
      log "debtmap summary generation failed"
      record_summary "debtmap: failed"
    fi
  else
    log "debtmap analysis failed"
    record_summary "debtmap: failed"
  fi
else
  log "Skipping debtmap (debtmap CLI not installed)"
  record_summary "debtmap: skipped"
fi

if command -v cargo-machete >/dev/null 2>&1; then
  log "Running cargo machete"
  if cargo machete --with-metadata > "$REPORTS_DIR/machete.txt"; then
    record_summary "machete: ok"
  else
    status=$?
    if [ $status -eq 1 ]; then
      record_summary "machete: unused dependencies detected"
    else
      log "cargo machete failed (exit $status)"
      record_summary "machete: failed"
    fi
  fi
else
  log "Skipping unused dependency check (cargo-machete not installed)"
  record_summary "machete: skipped"
fi

if command -v git >/dev/null 2>&1 && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  if git rev-list --max-count=1 HEAD >/dev/null 2>&1; then
    CHURN_WINDOW="${CHURN_SINCE:-30 days ago}"
    log "Analyzing git churn (since ${CHURN_WINDOW})"
    tmp_churn=$(mktemp)
    if git log --since="$CHURN_WINDOW" --no-merges --numstat --format='commit:%H:%ct' > "$tmp_churn"; then
      if run_metrics_post churn --input "$tmp_churn" --json-output "$REPORTS_DIR/churn.json" --md-output "$REPORTS_DIR/churn.md" --since "$CHURN_WINDOW"; then
        record_summary "churn: ok"
      else
        log "git churn summary generation failed"
        record_summary "churn: failed"
      fi
    else
      log "git log failed while computing churn"
      record_summary "churn: failed"
    fi
    rm -f "$tmp_churn"
  else
    log "Skipping churn (no commits yet)"
    record_summary "churn: skipped (no history)"
  fi
else
  log "Skipping churn (git repository not detected)"
  record_summary "churn: skipped"
fi

log "Summary:"
for line in "${SUMMARY[@]}"; do
  log "  $line"
  done

log "Reports written to $REPORTS_DIR/"
