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
  - Extend ingestion outcome to report dedupe counters using `chunk_hash` (e.g., `inserted`, `updated`, `skipped_duplicates`).

- `src/qdrant.rs`
  - Extend `index_points` to upsert vectors with the full payload:
    - `memory_id` (UUID), `project_id`, `memory_type`, `timestamp`, `source_uri`, `chunk_hash`, `tags`, `text`.
  - Add helper to create payload indexes: `project_id`, `memory_type`, `tags`, `timestamp`.
  - Add `search_points(query_vector, filter, limit)` composing Qdrant filters.
  - Keep current HTTP transport; defer gRPC client migration for later.
  - Add snapshot helpers to enumerate distinct payload values for MCP resources:
    - `list_projects()` → distinct `project_id`
    - `list_tags(project_id?)` → distinct tag values (scoped when provided)

- `src/embedding/`
  - Add Ollama embedding client (env‑controlled); preserve deterministic fallback.
  - Enforce vector dimension at collection creation time.

- `src/mcp.rs`
  - New tool: `search` with input: `{ query_text, project_id?, memory_type?, tags?, time_range?: { start, end }, limit?, score_threshold?, collection? }` and sensible defaults.
  - Update `push` to accept optional metadata (`project_id`, `memory_type`, `tags`, `source_uri`) and return dedupe counters.
  - Alias `index` → `push` for discoverability.
  - Enable MCP resources and expose read‑only resources for discovery and preflight:
    - `mcp://rusty-mem/memory-types` → `["episodic","semantic","procedural"]`
    - `mcp://rusty-mem/projects` → distinct `project_id`
    - `mcp://rusty-mem/projects/{project_id}/tags` → distinct tags under a project
    - `mcp://rusty-mem/health` → embedding provider/model + Qdrant reachability and vector size
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

## Local Stack (Qdrant & Ollama)

Baseline assumptions for all milestones:
- Qdrant is already running locally and kept always‑on at `http://127.0.0.1:6333` (persistent storage). Do not include container start steps in milestone notes; only validate connectivity.
- Ollama may be used for live embeddings when `EMBEDDING_PROVIDER=ollama`; start it only if not already running.

Quick validations (use these before testing):
- Qdrant: `curl -s http://127.0.0.1:6333/collections | jq .`
- Ollama: `curl -s http://127.0.0.1:11434/api/tags | jq '.models[].name'`

- If bootstrapping a new machine: Qdrant via Docker (recommended)
  - One‑liner (ephemeral storage):
    - `docker run --pull=always -p 6333:6333 qdrant/qdrant:latest`
  - With persistent storage:
    - `docker volume create qdrant_storage`
    - `docker run --pull=always -p 6333:6333 -v qdrant_storage:/qdrant/storage qdrant/qdrant:latest`
  - Health/sanity checks:
    - `curl -s http://127.0.0.1:6333/collections | jq .` (should return a JSON object)
  - Env expected by the server:
    - `QDRANT_URL=http://127.0.0.1:6333`

- Ollama (for real embeddings; optional for unit tests but required for live checks when `EMBEDDING_PROVIDER=ollama`)
  - Install/start: `ollama serve` (macOS: `brew install ollama`), default URL `http://127.0.0.1:11434`.
  - Pull an embedding model (example): `ollama pull nomic-embed-text`
  - Health/sanity checks:
    - `curl -s http://127.0.0.1:11434/api/tags | jq '.models[].name'`
  - Env expected by the server:
    - `EMBEDDING_PROVIDER=ollama`
    - `EMBEDDING_MODEL=<your-embedding-model>` (e.g., `nomic-embed-text`)
    - `EMBEDDING_DIMENSION=<matching-dimension>` (must match your model; see the model card)
    - `OLLAMA_URL=http://127.0.0.1:11434` (optional; defaults to this value)
  - Notes
    - If the model’s dimension does not match `EMBEDDING_DIMENSION`, ingestion/search will error with a helpful message. Adjust the env to match the model.

- Project env quickstart
  - Copy defaults: `cp .env.example .env` and tweak as needed (especially `EMBEDDING_MODEL` and `EMBEDDING_DIMENSION`).
  - Start MCP server: `cargo run --bin rusty_mem_mcp` (reads `.env`).
  - Start HTTP server (if needed): `cargo run` (port from `SERVER_PORT` or 4100–4199 scanner).

- Troubleshooting
  - Qdrant not reachable: confirm container runs; `curl -s http://127.0.0.1:6333/collections` should succeed.
  - Ollama model missing/dimension mismatch: run `ollama pull <model>` and ensure `EMBEDDING_DIMENSION` matches; restart the server.

## 4) Staged Plan with Gates, Tests, and Git Workflow

Each milestone lists tasks, acceptance criteria, how to test, expected results, and git steps.

<!-- Note: Baseline smoke checks and branch policy are consolidated under
     "Git Workflow & Release Checklist" and "Validation Protocol" below. -->

### M1 – Status

✅ Completed. Processing now stamps each chunk with RFC3339 `timestamp` and deterministic `chunk_hash`, and Qdrant receives the expanded universal payload (`memory_id`, `project_id`, `memory_type`, `timestamp`, `chunk_hash`, `text`). Payload indexes for `project_id`, `memory_type`, `tags`, `timestamp`, and `chunk_hash` are provisioned during startup. `/index` and the MCP `push` tool remained backward compatible.

### M2 – Status

✅ Completed. Ollama is integrated as the primary embedding provider when `EMBEDDING_PROVIDER=ollama` (configurable via `OLLAMA_URL` and model env vars). The service validates vector dimensions, surfaces helpful connection errors, and falls back to the deterministic encoder for other providers. README/config docs updated; MCP integration test harness still mocks Qdrant, but all final validation was performed against live Ollama + Qdrant.

### M3 – Status

✅ Completed. `push`/`index` now accept optional metadata (`project_id`, `memory_type`, `tags`, `source_uri`) with defaults, intra-request dedupe is enforced before embedding, Qdrant payloads persist the overrides, and the ingestion outcome reports `inserted`, `updated`, and `skippedDuplicates` alongside `chunksIndexed`/`chunkSize`. Both HTTP `/index` and the MCP tool surface the richer response so clients see dedupe effects immediately.

### M4 – MCP Resources (Memory Types, Health)

✅ Completed. The MCP server now advertises the resources capability and returns `mcp://rusty-mem/memory-types` and `mcp://rusty-mem/health` via `listResources`, each served as JSON text when fetched through `readResource`. The memory-types snapshot enumerates the allowed values and default, while the health payload reports the live embedding configuration plus a Qdrant reachability probe (including default-collection presence and any error message). Host instructions and the README walk agents through `listResources`/`readResource`, and unit tests cover the resource serializers. Live validation: `cargo test --test live_validation -- --ignored --nocapture` (requires Qdrant at `http://127.0.0.1:6333` and Ollama at `http://127.0.0.1:11434`).

### M5 – MCP Resources (Projects and Tags)

- Tasks
  - Add resources:
    - `mcp://rusty-mem/projects`
    - `mcp://rusty-mem/projects/{project_id}/tags`
  - Implement snapshot helpers in `qdrant.rs` to enumerate distinct values (scoped for tags).

- Acceptance Criteria
  - `projects` returns at least one entry after ingestion; `.../tags` returns tags for a known project.
  - Resource responses are read‑only and fast on small local datasets.

- How to Test
  - Ingest two docs with different `project_id` and tags; read both resources and confirm enumeration matches payloads.

- Expected Results
  - Agents can discover projects and tags to propose filters without new tool calls.

- Git Steps
  - Commit message: `Add MCP resources for projects and per‑project tags`

### M6 – Search (MCP, Minimal)

- Tasks
  - Extend `QdrantService` with `search_points` that issues `POST /collections/{collection}/points/search` (see [Qdrant Search API](https://api.qdrant.tech/v-1-15-x/api-reference/search/points)): serialize `{ "vector": <query>, "limit": <k>, "filter": { ... }, "with_payload": true }` and map responses to a Rust DTO containing `{ id, score, payload }`.
  - Add a small filter builder (`build_search_filter`) that composes `must` clauses for optional fields:
    - `project_id` and `memory_type` → `{"key": "…", "match": {"value": …}}`
    - `tags` → `{"key": "tags", "match": {"any": [...]}}` (Qdrant payload match supports `any` per docs).
    - `time_range` → `{"key": "timestamp", "range": {"gte": start, "lte": end}}`.
  - Introduce `ProcessingService::search_memories` that (a) embeds `query_text` with the configured embedding client (single vector, dimension check), (b) calls `QdrantService::search_points`, (c) normalizes hits into `{ id, score, text, project_id, memory_type, tags, timestamp, source_uri }` records.
  - Update `RustyMemMcpServer::describe_tools` to register a `search` tool with input schema `{ query_text, project_id?, memory_type?, tags?, time_range?: { start, end }, limit?, score_threshold?, collection? }` and implement the handler that delegates to `ProcessingService::search_memories`.
  - Defaults: `limit=5`, `score_threshold=0.25`; omit optional filters by default.
  - Keep ingestion (`push`) behavior unchanged; only add read paths. Re‑use the existing payload indexes (project_id/memory_type/tags/timestamp) created in M1.

- Acceptance Criteria
  - Issuing `search` with no filters returns top‑k results ordered by score and includes the stored payload fields (text, metadata).
  - Supplying each optional filter narrows results as expected (e.g., constraining `project_id` eliminates other projects; `tags` performs contains-any semantics).
  - MCP `listTools` shows the new `search` entry with a description that mirrors the schema, and the handler returns structured JSON (no plain strings).
  - Error surfaces are actionable: empty `query_text` → `INVALID_PARAMS`; unreachable Qdrant → surfaced as MCP internal error with the upstream status text.

- How to Test
  - Live verification (mandatory): ingest at least two documents into Qdrant with different `project_id`, `memory_type`, and `tags`, then call the MCP `search` tool through Codex CLI or another host using real Ollama embeddings and a running Qdrant instance. Capture the exact commands and outputs in the PR notes.
  - Unit tests: cover `build_search_filter` for each optional field (including combined filters) and ensure the produced JSON matches Qdrant’s schema; add a `search_points` happy-path test using `httpmock` to validate request body serialization and response parsing.
  - Integration test: extend `tests/mcp_integration.rs` to perform an end-to-end search against the mock Qdrant server (verify handler wiring) but reiterate in comments that final manual checks must target the live service.
  - Manual curl sanity (documented in PR): `curl -s http://localhost:6333/collections/<collection>/points/search -d '{"vector": [...], "limit": 3, "filter": {...}}' | jq` to confirm the Qdrant payload contract aligns with what the client sends.

- Expected Results
  - Search results include payload echoes (`text`, `project_id`, etc.) and deterministic scores from Qdrant.
  - Filters reduce the result set without causing errors; range filters respect ISO‑8601 timestamps.
  - Logs show `Requesting embeddings from Ollama` when the query is embedded, demonstrating real-provider usage.

- Git Steps
  - Commit message: `Add MCP search (minimal) with payload filters and sensible defaults`

### M7 – Search UX Enhancements

- Tasks
  - Enrich `search` response with `context` (joined snippets with inline `[id]` citations) and `used_filters` echo for transparency.
  - Input ergonomics: accept aliases (`type`→`memory_type`, `project`→`project_id`, `k`→`limit`) and coerce `tags: string` → `tags: [string]`.
  - Tighten JSON schema: add `enum` for `memory_type`, `default` values, and `examples` (happy‑path and advanced) in `describe_tools`.

- Acceptance Criteria
  - Responses include `context` and `used_filters` when present.
  - Aliases and coercions work; canonical output remains consistent.
  - Tool description and schema reflect enums, defaults, and examples.

- How to Test
  - Unit: schema serialization; alias/coercion parsing tests.
  - Live: compare a call using aliases vs canonical parameters; verify identical hits.

- Expected Results
  - Agents can drop `context` directly into prompts and learn parameters from examples.

- Git Steps
  - Commit message: `Enhance search response/context and input ergonomics`

### M8 – Error Ergonomics & Instructions

- Tasks
  - Standardize error mapping (`INVALID_PARAMS` for bad inputs, internal errors for upstream issues) with short remediation hints.
  - Update `get_info.instructions` with a concise quickstart (discover resources → push → search) and examples.

- Acceptance Criteria
  - Invalid inputs produce actionable error messages; quickstart renders in MCP hosts.

- How to Test
  - Force validation errors (empty `query_text`, bad time range) and observe messages.

- Expected Results
  - Friendlier failures that guide agents and users to fix calls quickly.

- Git Steps
  - Commit message: `Standardize error ergonomics and update instructions`
  - PR should attach: sample MCP `search` invocations (JSON in/out), curl transcript hitting the live Qdrant search endpoint, and a note confirming Ollama was used for the query embedding.

### M9 – HTTP Mirror for Search (Optional)

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

### M10 – Summarize (Meta‑cognition)

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

### M11 – Hardening & (Optional) gRPC Migration

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
  - Use a single release branch for this effort: `release/agentic-memory`.
  - Land each milestone as one or more small commits on this branch.
  - Open one PR targeting `main` when the entire release is ready; keep a running PR description (draft) with validation notes.
  - At branch creation, run `./scripts/verify.sh fmt clippy test` to capture a green baseline.
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
