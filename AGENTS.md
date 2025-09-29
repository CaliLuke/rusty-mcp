# Repository Guidelines

> ⚠️ **Always use Context7 for docs:** when you need library or API documentation, call the Context7 MCP (`resolve-library-id` → `get-library-docs`) instead of web searches. This repo mounts `/Users/luca/.codex/AGENTS.md` precisely to keep that workflow obvious—follow it by default.
> ⚠️ **Mandatory live verification:** all final validation of MCP tools **must** run against live models/services (no mocks, no hermetic stubs). Run your end-to-end checks with the real embedding provider before signing off on any milestone.

## Project Structure & Module Organization

- `src/` holds the Rust server: `main.rs` wires routes and config, `api.rs` defines Axum handlers, `processing.rs` manages chunking, and `qdrant.rs` wraps vector-store access. Supporting modules live alongside (e.g., `config.rs`, `metrics.rs`, `embedding/`).
- `src/bin/metrics_post.rs` exposes the metrics ingestion helper binary.
- `docs/` captures product notes and engineering references.
- `scripts/` contains reusable automation such as formatting and metrics hooks; generated artifacts are written into `reports/`.
- `docs/Development.md` (process and quality gates) and `docs/Configuration.md` (environment guide) are the primary references when onboarding changes.

## Build, Test, and Development Commands

- `cargo build` compiles the core server and auxiliary binaries.
- `cargo run` starts the HTTP server (port from `SERVER_PORT` or the 4100–4199 scanner).
- `cargo run --bin rusty_mem_mcp` launches the MCP server with the current configuration.
- `cargo test` exercises unit and integration tests; pass `-- --nocapture` to inspect stdout.
- `./scripts/verify.sh` batches `fmt`, `clippy`, doc builds, and tests (accepts subcommands like `fmt` or `test`).
- `./scripts/metrics.sh` refreshes coverage, complexity, and dependency reports under `reports/`.
- Context7 MCP quickstart:
  - Resolve docs: `context7__resolve-library-id(libraryName: "<crate>")`
  - Fetch sections: `context7__get-library-docs(context7CompatibleLibraryID: "</org/project>", topic: "<focus>")`
  - Prefer this flow over manual web searches unless the docs genuinely aren’t in Context7.

## CI checks via GitHub MCP

- To check the latest CI run quickly, use the GitHub MCP:
  - List latest runs for the workflow: `list_workflow_runs(owner: "CaliLuke", repo: "rusty-mcp", workflow_id: "ci.yml", perPage: 1)`
  - Get job statuses for that run: `list_workflow_jobs(owner: "CaliLuke", repo: "rusty-mcp", run_id: <from previous>)`
  - If needed, fetch just failed job logs: `get_job_logs(owner: "CaliLuke", repo: "rusty-mcp", run_id: <id>, failed_only: true, return_content: true)`

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
