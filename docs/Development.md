# Development Guide

This guide describes the tooling, automation, and expectations for working in the Rusty Memory repository.

## Toolchain and Prerequisites

- Install the latest stable Rust toolchain (edition 2024) with `rustup`, including the `rustfmt` and `clippy` components.
- Optional but recommended developer utilities:
  - [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) for coverage summaries.
  - [`rust-code-analysis-cli`](https://github.com/mozilla/rust-code-analysis) for complexity metrics.
  - [`cargo-machete`](https://github.com/bnjbvr/cargo-machete) and [`debtmap`](https://github.com/frewsxcv/debtmap) for dependency and technical-debt reports.
  - [`tokei`](https://github.com/XAMPPRocky/tokei), [`taplo-cli`](https://taplo.tamasfe.dev/cli/), and [`dprint`](https://dprint.dev/) for formatting checks.
- Install `prek` (`cargo install prek`) or `pre-commit` (`pipx install pre-commit` or `pip install pre-commit`) to run the Git hooks easily.

## Repository Layout

- `src/` contains the application code. `main.rs` hosts the HTTP server; `bin/rusty_mem_mcp.rs` exposes the MCP process.
- `src/qdrant/` wraps the vector-store client; the `scroller` module provides iterator-style access to the scroll API so callers no longer reimplement pagination loops.
- `src/processing/service.rs` owns the ingestion/summarization pipeline. Use `ProcessingService::try_new()` for synchronous construction, call `init().await` once to provision Qdrant, and fall back to `ProcessingService::new().await` when a convenience helper is acceptable.
- `scripts/` contains automation: `verify.sh`, `metrics.sh`, and hook wrappers invoked by the linters.
- `reports/` is reserved for metrics output. The commit-time metrics hook writes to a temporary directory so that the working tree stays clean.
- `tests/` currently houses the MCP integration test harness.

## Configuration and Runtime

The server loads configuration from environment variables via `dotenvy`:

| Variable                          | Purpose                                                                                            |
| --------------------------------- | -------------------------------------------------------------------------------------------------- |
| `QDRANT_URL`                      | Base URL for the Qdrant deployment.                                                                |
| `QDRANT_COLLECTION_NAME`          | Default collection used for indexing.                                                              |
| `QDRANT_API_KEY`                  | Optional API token forwarded to Qdrant.                                                            |
| `EMBEDDING_PROVIDER`              | `ollama` or `openai`.                                                                              |
| `EMBEDDING_MODEL`                 | Provider-specific model identifier (e.g. `nomic-embed-text`).                                      |
| `EMBEDDING_DIMENSION`             | Vector dimensionality expected by the target collection.                                           |
| `TEXT_SPLITTER_CHUNK_SIZE`        | Optional chunk-size override. Auto-derived from the embedding model when unset.                    |
| `TEXT_SPLITTER_CHUNK_OVERLAP`     | Token overlap (integer) applied between adjacent chunks; defaults to `0` for historical behaviour. |
| `TEXT_SPLITTER_USE_SAFE_DEFAULTS` | When set to `1`, halves the automatic chunk-size heuristic to improve retrieval specificity.       |
| `SERVER_PORT`                     | Optional fixed HTTP port. If absent, the server selects the first open port in `4100-4199`.        |
| `RUSTY_MEM_LOG_FILE`              | Optional absolute path for structured logs. Defaults to `logs/rusty-mem.log`.                      |

### Running surfaces

- HTTP API: `cargo run` launches the Axum server.
- MCP server: `cargo run --bin rusty_mem_mcp` (or execute the release binary at `target/release/rusty_mem_mcp`).

Both surfaces expose the derived chunk size: the HTTP `POST /index` response uses `chunk_size`, while the MCP `push` tool uses `chunkSize`. The metrics endpoint/tool include `lastChunkSize` alongside document and chunk counters.

The automatic chunk-size heuristic uses a quarter of the embedding model's context window by default and clamps values into the `[256, 1024]` range. Opting into `TEXT_SPLITTER_USE_SAFE_DEFAULTS=1` halves that budget (window/8) to bias toward smaller chunks for improved precision. `TEXT_SPLITTER_CHUNK_OVERLAP` controls a sliding overlap (in tokens) applied after semantic chunking; keeping it unset preserves the historical split behaviour.

## Git Hooks and Automation

Hooks are installed via `cargo husky` and re-dispatched to `prek` or `pre-commit` when available. Install them once:

```bash
cargo test --workspace --no-run
prek install        # or: pre-commit install
```

Commit-time hooks execute:

1. **`verify-fast`** (`scripts/hook-verify-fast.sh`) – runs `./scripts/verify.sh fmt test` with `RUSTY_MEM_LOG_FILE=/dev/null` so test runs do not touch the repository log file.
2. **`taplo fmt --check`** – validates TOML formatting.
3. **`dprint fmt --check (markdown)`** – ensures Markdown documents are formatted consistently.
4. **`metrics soft run`** – invokes `scripts/metrics.sh` with `METRICS_SOFT=1`, which writes reports to a temporary directory and summarises coverage, complexity, unused dependencies, and LOC. Failures indicate a real tool error.

Push-time hooks call `./scripts/verify.sh fmt clippy test doc`, guaranteeing that formatting, linting, unit/integration tests, and documentation builds pass prior to publishing.

You can manually mirror each stage:

```bash
./scripts/verify.sh          # run fmt, clippy, test, doc
./scripts/verify.sh fmt      # individual steps are supported
./scripts/metrics.sh         # full metrics suite (writes to reports/)
prek run --all-files         # evaluate all hooks locally
```

## Code Quality Expectations

- **Documentation:** `#![deny(missing_docs)]` is active; every public item must carry a Rustdoc comment. The doc build (and therefore the hooks) will fail if coverage regresses.
- **Inline comments:** Annotate non-trivial control flow or heuristics with a brief `// why` comment so future learners understand the rationale. Pull requests that introduce logic without commentary are expected to add it during review.
- **Formatting:** follow `rustfmt` defaults. Markdown changes must pass `dprint fmt`.
- **Linting:** `cargo clippy --all-targets --all-features -D warnings` is enforced.
- **Tests:** keep unit tests colocated with the code, ensure deterministic behaviour, and favour descriptive names.
- **Logging:** emit structured `tracing` events; prefer `info` for external actions, `debug` for internal milestones.
- **Unsafe code:** not permitted.

## Suggested Workflow

1. Set required environment variables (see the table above).
2. Develop the feature or fix.
3. Run `./scripts/verify.sh` to ensure formatting, linting, tests, and docs pass locally.
4. Optionally run `./scripts/metrics.sh` to refresh reports in `reports/`.
5. Execute `prek run --all-files` (or `pre-commit run --all-files`) to mirror the commit hooks.
6. Stage changes and commit once the hook suite and metrics look healthy.

## Troubleshooting

- **Hook reports “command not found”:** install the optional tooling listed above or edit the metrics script to skip the missing tool (the hook prints explicit instructions).
- **Temporary metrics output:** the soft metrics hook writes to a disposable directory; run `./scripts/metrics.sh` manually if you need persisted artefacts.
- **Logs clutter the working tree:** override `RUSTY_MEM_LOG_FILE` when running development commands to a throwaway path if required.
- **Prek cache reset:** `prek cache clean` removes cached hook environments when dependencies change.

Following these practices keeps the repository consistent and the automation green.
