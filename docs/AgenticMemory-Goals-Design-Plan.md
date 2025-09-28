# Agentic Memory: Goals, Principles, Design, and Execution Plan

This document defines the project goals, the first‑principles approach, the target design, and a staged plan (with gates, acceptance criteria, test steps, and git workflow) to evolve Rusty Memory.

## 1) Project Goals

- Reliable, scoped retrieval for agents across projects via a cognitive memory model.
- Local‑first operation with minimal dependencies; MCP‑first surface over stdio.
- Ship incrementally to unlock value early while preserving backward compatibility.
- Maintain simplicity and testability; avoid premature complexity.

Success is measured by agents retrieving relevant memories filtered by project, memory type, tags, and time, with stable ingestion (`/index`, `push`) and predictable performance locally.

## 2) First‑Principles Approach

- Cognitive model as the foundation:
  - Tripartite memory: `episodic` (events), `semantic` (facts), `procedural` (how‑to).
  - Retrieval must filter by `memory_type` to reduce noise and improve precision.
- Hybrid search, not just vectors:
  - Semantic similarity + payload filters (`project_id`, `memory_type`, `tags`, `timestamp`).
  - Create payload indexes upfront for speed and determinism.
- Universal payload:
  - Enforce a consistent application‑level schema so filters and tooling stay simple.
- MCP‑first ergonomics:
  - Add `search` and later `summarize` as MCP tools first; mirror to HTTP once stable.
- Determinism and safety:
  - Keep a deterministic embedding fallback for tests; assert vector dimension alignment.
- Incremental delivery without regressions:
  - Additive changes; document any intentional command/contract updates in release notes/README.

## 3) Design Overview (Target Code Changes)

Key modules to evolve are listed with concrete responsibilities.

- `src/processing.rs`
  - Compute `timestamp` (server time) and `chunk_hash` (sha256 of normalized content).
  - Assemble the universal payload per chunk; keep existing `text` field for compatibility.
  - Accept optional overrides for `project_id`, `memory_type`, `tags`, `source_uri` from higher layers.

- `src/qdrant.rs`
  - Extend `index_points` to upsert vectors with the full payload:
    - `memory_id` (UUID), `project_id`, `memory_type`, `timestamp`, `source_uri`, `chunk_hash`, `tags`, `text`.
  - Add helper to create payload indexes: `project_id`, `memory_type`, `tags`, `timestamp`.
  - Add `search_points(query_vector, filter, limit)` composing Qdrant filters.
  - Keep current HTTP transport; defer gRPC client migration for later.

- `src/embedding/`
  - Add Ollama embedding client (env‑controlled); preserve deterministic fallback.
  - Enforce vector dimension at collection creation time.

- `src/mcp.rs`
  - New tool: `search` with input: `{ query_text, project_id?, memory_type?, tags?, time_range?: { start, end }, limit? }`.
  - Later tool: `summarize` for consolidation (time‑bounded episodic → new semantic memory).
  - Preserve existing tools (`push`, `get-collections`, `new-collection`, `metrics`).

- `src/api.rs` (optional mirror after MCP stabilizes)
  - `POST /search` with the same schema as the MCP `search` tool.

### Universal Payload (Application‑Level)

```
{
  memory_id: string (uuid),
  project_id: string (default: "default"),
  memory_type: "episodic" | "semantic" | "procedural" (default: "semantic"),
  timestamp: string (RFC3339),
  source_uri?: string,
  chunk_hash: string (sha256),
  tags?: string[],
  text: string
}
```

### Filter Semantics

- `project_id` → exact match.
- `memory_type` → exact match.
- `tags` → contains‑any or contains‑all (start with contains‑any for simplicity).
- `time_range` → `timestamp` range (`start`, `end`) when supplied.

## 4) Staged Plan with Gates, Tests, and Git Workflow

Each milestone lists tasks, acceptance criteria, how to test, expected results, and git steps.

### M0 – Baseline & Branching

- Tasks
  - Create a feature branch; run quick smoke to capture baseline.
- Acceptance Criteria
  - CI/`./scripts/verify.sh` passes on the branch with no changes.
- How to Test
  - `./scripts/verify.sh fmt clippy test`.
  - Launch Qdrant locally; run `cargo run` and exercise `POST /index` with sample text.
- Expected Results
  - `/index` returns `chunks_indexed` and `chunk_size`; Qdrant shows points with payload `{ text }`.
- Git Steps
  - `git checkout -b feature/agentic-memory-mvp`
  - Commit baseline README/plan updates as needed.

### M1 – Schema, Dedupe, Payload Indexes (No API Changes)

- Tasks
  - Extend processing to compute `timestamp` and `chunk_hash`.
  - Extend Qdrant payload; add helper to create payload indexes.
- Keep `/index` response shape unchanged for M1 to avoid churn; later milestones may update commands as needed (documented).
- Acceptance Criteria
  - Ingested points contain new payload fields and still include `text`.
  - Indexes created for `project_id`, `memory_type`, `tags`, `timestamp`.
  - No change to `/index` request/response or MCP `push` tool.
- How to Test
  - Unit: hash computation and payload assembly.
    - `cargo test -p rustymcp processing::chunk_hash_*` (or aligned test names).
  - Integration: ingest sample; inspect Qdrant payload fields via HTTP API/UI.
  - Run `./scripts/verify.sh` locally and ensure no API diffs.
- Expected Results
  - Payload shows `memory_id`, `project_id(default)`, `memory_type(semantic)`, `timestamp`, `chunk_hash`, `text`.
  - Ingestion metrics still increment correctly.
- Git Steps
  - Commit message: `Add universal payload, dedupe, and payload indexes (backward compatible)`
  - Push branch and open PR; ensure CI passes.

### M2 – Real Embeddings via Ollama (Env‑Controlled)

- Tasks
  - Add an Ollama-backed embedding client:
    - Use `ollama-rs` (`Ollama::new(host, port)`); default host `http://localhost` & port `11434` when env not set.
    - Implement `EmbeddingClient` for an `OllamaClient` that issues `GenerateEmbeddingsRequest::new(model, texts.into())` and unwraps `GenerateEmbeddingsResponse.embeddings`.
    - Map `EMBEDDING_PROVIDER=ollama` to this client; keep AiLib deterministic fallback for other providers/tests.
    - Add config wires: `OLLAMA_URL` (or split into host/port) and default model (e.g., `mxbai-embed-large`).
  - Enforce vector dimension alignment:
    - Validate the length of each Ollama embedding matches `EMBEDDING_DIMENSION`; surface clear errors otherwise.
  - Graceful errors & retry posture:
    - Bubble up connection failures with actionable messages (e.g., “Set OLLAMA_URL=http://localhost:11434 and ensure Ollama is running”).
    - Ensure deterministic fallback remains default when provider != `ollama`.
- Acceptance Criteria
  - With `EMBEDDING_PROVIDER=ollama`, the service successfully requests embeddings via Ollama and indexes them.
  - When Ollama is unreachable, ingestion aborts with a descriptive error (no silent fallback).
  - With `EMBEDDING_PROVIDER=openai` (or other), behavior remains identical to deterministic fallback.
- How to Test
  - Configure `.env`:
    - `EMBEDDING_PROVIDER=ollama`
    - `EMBEDDING_MODEL=mxbai-embed-large`
    - `OLLAMA_URL=http://127.0.0.1:11434`
  - Ingest sample; confirm Qdrant vectors match `EMBEDDING_DIMENSION`.
  - Stop Ollama and re-run ingestion to verify error messaging.
  - Switch provider back to deterministic fallback and confirm identical embeddings for the same payload via snapshot (hash or equality check).
  - `./scripts/verify.sh fmt clippy test` must pass; add targeted unit tests around the client adapter (mock via `ollama_rs` traits or by feature flagging).
- Expected Results
  - Logs show “using Ollama embeddings” with host/model details.
  - Ingestion metrics increment normally; payload unchanged.
- Git Steps
  - Commit message: `Integrate Ollama embeddings with deterministic fallback`
  - Update `docs/Configuration.md`/`README.md` with new env vars and usage notes.

### M3 – Search (MCP First)

- Tasks
  - Add `search_points` to Qdrant client; compose payload filters.
  - Add MCP `search` tool: input `{ query_text, project_id?, memory_type?, tags?, time_range?, limit? }`.
  - Return results: `[{ id, score, text, project_id, memory_type, tags, timestamp, source_uri }]`.
  - Do not change `/index` or `push` behavior.
- Acceptance Criteria
  - Search returns relevant results and respects filters.
  - Tool listed in MCP `listTools`; documentation shows input schema.
- How to Test
  - Ingest 2–3 small docs across different `project_id` and `memory_type`.
  - Call MCP `search` (from host IDE/CLI) with and without filters; validate top‑k.
  - Unit: filter composition helper → correct Qdrant JSON.
  - Integration: ingest → search; assert returned payload fields and scores present.
- Expected Results
  - Filtered results change predictably (e.g., restricting to `project_id` reduces hits).
- Git Steps
  - Commit message: `Add MCP search with payload filters`
  - PR with test notes and sample tool calls.

### M4 – HTTP Mirror for Search (Optional)

- Tasks
  - Add `POST /search` mirroring the MCP schema.
  - Document the endpoint; keep it optional for clients that prefer HTTP.
- Acceptance Criteria
  - Endpoint behaves identically to MCP search in inputs/outputs.
- How to Test
  - `curl`/`httpie` POST to `/search` with sample payloads; compare with MCP results.
  - `./scripts/verify.sh` must pass; add a minimal integration test if appropriate.
- Expected Results
  - HTTP and MCP surfaces produce consistent results for the same inputs.
- Git Steps
  - Commit message: `Mirror search as HTTP endpoint`

### M5 – Summarize (Meta‑cognition)

- Tasks
  - MCP `summarize`: consolidate time‑bounded episodic memories into a new semantic memory (tag `summary`).
  - Optional HTTP mirror later.
- Acceptance Criteria
  - Tool returns `{ summary, source_memory_ids }` and upserts a new semantic memory.
- How to Test
  - Create a few episodic entries across a time window; run `summarize`; ensure a new semantic memory is created and discoverable via search.
  - Keep deterministic fallback for tests; Ollama can be toggled for local quality checks.
- Expected Results
  - Summary quality acceptable on small local models; linkage to sources recorded.
- Git Steps
  - Commit message: `Add MCP summarize (episodic → semantic consolidation)`

### M6 – Hardening & (Optional) gRPC Migration

- Tasks
  - Stress test search and ingestion; profile; add targeted caching/limits if needed.
  - Plan/execute migration to `qdrant-client` (gRPC) if performance/ergonomics justify it.
- Acceptance Criteria
  - No regressions to ingestion/search; improved latency or cleaner code if migrating.
- How to Test
  - Bench simple workloads locally; compare before/after.
- Git Steps
  - Commit message: `Migrate to qdrant-client (gRPC) [optional]`

## Release Guardrails

- Keep `main` releasable: run the full validation protocol before pushing or tagging.
- Document command surface changes (HTTP or MCP) in PR descriptions or release notes so downstream consumers—presently just us—understand the new behavior.
- When altering request/response schemas, update examples in `README.md` and `docs/Configuration.md` as part of the change.

## Test Commands and Examples

Baseline ingestion (HTTP):

```bash
curl -sS -X POST localhost:4100/index \
  -H 'Content-Type: application/json' \
  -d '{"text":"My sample document about Axum and Qdrant"}' | jq
```

MCP search (example request body shown by host when calling the tool):

```json
{
  "query_text": "current architecture plan",
  "project_id": "default",
  "memory_type": "episodic",
  "tags": ["architecture"],
  "time_range": { "start": "2025-01-01T00:00:00Z", "end": "2025-12-31T23:59:59Z" },
  "limit": 5
}
```

Repo checks:

```bash
./scripts/verify.sh fmt clippy test
./scripts/metrics.sh   # optional metrics suite
```

## Validation Protocol (Must Precede Every Commit)

- Final end-to-end verification **must use live services**. Never rely on mocked embedding/Qdrant responses when declaring a milestone complete—rerun the tooling with the real provider enabled and document the commands.
- Run the full fast suite (`./scripts/verify.sh fmt clippy test`) before committing any milestone work. If a commit spans multiple milestones, rerun after each logical chunk of changes.
- Execute any live surface checks required by the milestone (e.g., `/index` against a local Qdrant, MCP tool smoke tests) and capture the commands + results in commit or PR notes.
- Do not commit until all required tests pass locally; fix failures first, then repeat the validation loop.
- Preserve logs of manual checks in `/tmp` or the PR description so reviewers can trace the steps.

## Git Workflow & Release Checklist

- Branching
  - Feature branches: `feature/agentic-memory-<milestone>` (e.g., `feature/agentic-memory-m3-search`).
- Commits
  - Short, imperative subjects; one logical change per commit.
  - Include rationale in body for behavior changes.
- Pull Requests
  - Summarize behavior changes; list validation commands and sample requests.
  - Link issues; attach screenshots/snippets when adding/altering endpoints/tools.
- Pre‑merge gates
  - `./scripts/verify.sh` green; `cargo clippy -D warnings` clean.
  - Compatibility tests pass for `/index` and MCP `push`.
- Release hygiene
  - Update `docs/Configuration.md` for any new env vars.
  - Update `README.md` usage examples if new tools/endpoints are added.
  - Tag only when a milestone is complete and stable.


### Cross-Cutting Tasks

- Maintain `docs/Configuration.md` alongside each milestone; every new env var or CLI flag should land in the same PR.
- Extend `scripts/verify.sh` or add targeted scripts when new behaviors (e.g., MCP search) need reproducible smoke tests.
- Update release notes after each milestone so MCP and HTTP consumers know when to retest integrations.
- Documentation drift -> Treat docs updates as required checklist items in PR templates; reviewers must confirm new surfaces are documented.
