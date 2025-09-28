# Repository Guidelines

## Project Structure & Module Organization

- `src/` holds the Rust server: `main.rs` wires routes and config, `api.rs` defines Axum handlers, `processing.rs` manages chunking, and `qdrant.rs` wraps vector-store access. Supporting modules live alongside (e.g., `config.rs`, `metrics.rs`, `embedding/`).
- `src/bin/metrics_post.rs` exposes the metrics ingestion helper binary.
- `docs/` captures product notes and engineering references.
- `scripts/` contains reusable automation such as formatting and metrics hooks; generated artifacts are written into `reports/`.
- `docs/QUALITY_MANUAL.md` (process and quality gates) and `docs/Configuration.md` (environment guide) are the primary references when onboarding changes.

## Build, Test, and Development Commands

- `cargo build` compiles the core server and auxiliary binaries.
- `cargo run` starts the HTTP server (port from `SERVER_PORT` or the 4100–4199 scanner).
- `cargo run --bin rusty_mem_mcp` launches the MCP server with the current configuration.
- `cargo test` exercises unit and integration tests; pass `-- --nocapture` to inspect stdout.
- `./scripts/verify.sh` batches `fmt`, `clippy`, doc builds, and tests (accepts subcommands like `fmt` or `test`).
- `./scripts/metrics.sh` refreshes coverage, complexity, and dependency reports under `reports/`.

## Coding Style & Naming Conventions

- Format Rust via `cargo fmt --all`; `scripts/verify.sh fmt` enforces `cargo fmt --all --check` in CI.
- Run `cargo clippy --all-targets --all-features -D warnings` before opening a PR.
- Keep module and file names snake_case; types in UpperCamelCase; functions and fields snake_case.
- Markdown and Markdown-in-rustdoc should pass `dprint fmt`; TOML manifests should stay Taplo-formatted through `scripts/hook-taplo.sh`.

## Testing Guidelines

- Prefer colocated `#[cfg(test)]` modules for unit coverage; integration tests belong in `tests/` if they grow beyond in-module scope.
- Mirror test names after the behavior under test (e.g., `search_returns_top_match`).
- Use `cargo llvm-cov` via `./scripts/metrics.sh` for coverage snapshots and keep functional coverage steady when adding features.
- When adding new endpoints, check in API contract examples alongside the implementation and add smoke tests that hit the Axum router.

## Commit & Pull Request Guidelines

- History is currently empty; adopt short, imperative commit subjects (e.g., `Add Qdrant collection bootstrap`) and include rationale in the body when changing behavior.
- Group logical changes per commit; avoid bundling formatting with feature work.
- Pull requests should summarize behavior changes, list validation commands (e.g., `./scripts/verify.sh`), and link any tracked issues.
- Attach screenshots or sample payloads when modifying API contracts so reviewers can trace request/response changes quickly.

## Project-specific reminder

- `rusty-mem` communicates with Qdrant over HTTP on port 6333. Ensure the REST endpoint stays reachable when launching the MCP server.
- MCP transport **must be stdio (child process)**. Do **not** attempt to wire SSE/HTTP transports—the agent clients we integrate with (Codex CLI, Kilo Code) only support stdio today, and SSE is deprecated across the ecosystem. Always use `rmcp`'s child-process / stdio transport helpers.
