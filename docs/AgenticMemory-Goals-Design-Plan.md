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
  - Present `push` as the canonical ingestion tool (no separate alias).
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

### Completed Milestones (M1–M8)

- M1: Added RFC3339 `timestamp` and deterministic `chunk_hash`; expanded universal payload persisted to Qdrant. Created payload indexes for `project_id`, `memory_type`, `tags`, `timestamp`, `chunk_hash`. Kept `/index` and MCP `push` backward compatible.
- M2: Integrated Ollama embeddings (`EMBEDDING_PROVIDER=ollama`), validated vector dimensions, and retained deterministic fallback. Performed live validation against Ollama + Qdrant.
- M3: Enriched ingestion with optional metadata (`project_id`, `memory_type`, `tags`, `source_uri`) and intra-request dedupe. Responses now include `inserted`, `updated`, `skippedDuplicates`, `chunksIndexed`, `chunkSize` across HTTP and MCP.
- M4: Exposed MCP resources for `mcp://rusty-mem/memory-types` and `mcp://rusty-mem/health` with JSON payloads; documented usage and added unit coverage. Live checks require reachable Qdrant (`http://127.0.0.1:6333`) and Ollama (`http://127.0.0.1:11434`).
- M5: Added MCP resources for `mcp://rusty-mem/projects` and `mcp://rusty-mem/projects/{project_id}/tags`, backed by Qdrant scroll + payload indexes. Validated live (Ollama + Qdrant), and introduced `schemars` for derived resource schemas.
- M6: Delivered the MCP `search` tool with semantic filtering. Implemented reusable filter builders, Qdrant query support, processing-layer `search_memories`, and an integration test. Live validation ingests real memories and executes filtered searches against Qdrant + Ollama.
- M7: Polished MCP `search` ergonomics by accepting aliases (`type`/`project`/`k`), coercing scalar tags, returning a prompt-ready `context`, and echoing `used_filters`; documented schema defaults/examples and shipped unit coverage alongside live validation.
- M8: Standardized error ergonomics for MCP `search`, including stringent input validation, actionable `invalid_params` messages, normalized responses with both `score_threshold` keys, JSON resource payloads (plus `mcp://rusty-mem/settings`), refreshed instructions, and live verification covering success, bad-input, and provider-failure paths.

### M9 – Summarize (Meta-cognition)

Goal: Provide an MCP `summarize` tool that consolidates time‑bounded episodic memories into a new semantic memory, preserving provenance to the source episodic memories, and validating end‑to‑end against live services.

- Scope
  - Primary surface: MCP tool `summarize` (stdio transport only, consistent with MCP guidance).
  - Optional mirror: HTTP `POST /summarize` added later after MCP stabilizes.

- Design Directives
  - Abstractive by default using a local LLM via Ollama when available; deterministic extractive fallback when the provider is disabled/unavailable.
  - Additive changes only: extend the universal payload with optional provenance without breaking existing readers.
  - Make the operation idempotent within a short window by hashing inputs to avoid duplicate summaries.
  - Preserve search ergonomics: the resulting semantic memory must be discoverable via existing MCP `search` with filters (`project_id`, `memory_type=semantic`, `tags` contains `summary`).

- Data Model Additions (additive)
  - New optional field in the universal payload (persisted to Qdrant):
    - `source_memory_ids?: string[]` – provenance of episodic memories consolidated into the summary.
  - Tags convention:
    - Append `"summary"` to `tags` for the new semantic memory.
  - Idempotency key:
    - Compute `summary_key = sha256(project_id + time_range.start + time_range.end + joined(source_memory_ids))` and store it as a tag (e.g., `summary:<hash>`), or as an additional optional payload field `summary_key?: string`. Prefer a tag for fast negative lookups.

- MCP Tool: `summarize`
  - Input schema (Serde + Schemars; see hints below):
    - `{ project_id?: string, memory_type?: string = "episodic", tags?: string[] | string, time_range: { start: RFC3339, end: RFC3339 }, limit?: number = 50, strategy?: "auto" | "abstractive" | "extractive" = "auto", provider?: "ollama" | "none", model?: string, max_words?: number = 250, score_threshold?: number, collection?: string }`
    - Notes:
      - Coerce scalar `tags` to `[tags]` as done in M7 for `search`.
      - `limit` caps the number of episodic memories considered before summarization.
      - `strategy=auto` chooses `abstractive` if `SUMMARIZATION_PROVIDER` is available and healthy, else `extractive`.
  - Output schema:
    - `{ summary: string, source_memory_ids: string[], upserted_memory_id: string, used_filters: { ...echoed }, strategy: string, provider?: string, model?: string }`
  - Errors (normalized like M8):
    - `invalid_params` for bad/missing `time_range`, `limit <= 0`, or empty result set.
    - `provider_unavailable` when `abstractive` is requested but Ollama is not reachable.
    - `upsert_failed` if Qdrant write fails.

- Implementation Plan (code‑level steps)
  1) Qdrant query for episodic scope
     - Build a filter with `must` conditions: `project_id`, `memory_type=episodic` (unless overridden), optional `tags` (contains‑any), and `timestamp` range.
     - Reuse existing filter builders from M6/M7 to ensure parity.
     - Sort by `timestamp` ascending and cap by `limit` (default 50).
     - Hints (Context7 – Qdrant filters): see “Filter conditions (must/match/range)” and range examples.
       - Library: /websites/api_qdrant_tech (Points search with filter)
  2) Summarization pipeline
     - Abstractive (Ollama):
       - Compose a single prompt with a system directive and a compact bullet list of the selected episodic texts (with timestamps when available) to bound tokens.
       - Call Ollama `POST /api/generate` with `{ model, prompt, stream: false }`.
       - Hints (Context7 – Ollama API): use /ollama/ollama docs for `/api/generate` and `options`.
     - Extractive fallback (deterministic):
       - Strategy: take the first N sentences by chronological order subject to `max_words` and produce a concise bullet summary (e.g., top 3–5 bullets). Keep it pure Rust, no network.
       - Minimal heuristic: split by sentence terminators, keep lines <= 180 chars, accumulate until `max_words`.
  3) Upsert semantic summary
     - Assemble universal payload with `memory_type=semantic`, `tags += ["summary"]`, `source_memory_ids = <episodic ids>` and optional `summary_key` tag for idempotency.
     - Generate `memory_id = Uuid::new_v4()`.
     - Reuse `index_points` to upsert the new memory in the current collection.
  4) Idempotency guard
     - Before upsert, perform a fast search in Qdrant for an existing point with tag `summary:<hash>` within the same `project_id` and `time_range`; if found, return that `memory_id` and existing `summary` text.
  5) MCP tool wiring
     - Add a new tool `summarize` to `src/mcp.rs` with the above input/output schema, using `schemars` for JSON Schema exposure and consistent error mapping like M8.
     - Echo `used_filters` in the response to aid debugging.

- Prompt Template (abstractive)
  - System directive (prepend):
    - “You summarize developer activity into concise, factual bullet points. Prefer neutral tone. Avoid speculation. Include dates if present. Return at most {max_words} words. Output a single paragraph.”
  - User content (assembled):
    - Header: “Summarize the following episodic notes for project ‘{project_id}’ between {start} and {end}.”
    - Bullet list (trimmed/escaped) of episodic `text` values in chronological order; include short `YYYY‑MM‑DD` date prefix if available.

- Env & Config
  - New env vars (document in `docs/Configuration.md`):
    - `SUMMARIZATION_PROVIDER=ollama|none` (default: `ollama` if reachable; else `none`).
    - `SUMMARIZATION_MODEL` (e.g., `llama3.1:8b` or `mistral`), default falls back to `EMBEDDING_MODEL` family if sensible.
    - `SUMMARIZATION_MAX_WORDS=250`.
    - `OLLAMA_BASE_URL` (reuse if already present, else default `http://127.0.0.1:11434`).

- Module/Code Changes (suggested layout)
  - `src/summarize.rs` – Summarization orchestrator
    - `summarize_episodic(project_id, tags, time_range, limit, strategy, provider, model, max_words) -> Result<SummaryResult>`
    - Contains: episodic fetch, provider decision, prompt assembly, abstractive/extractive selection, idempotency hash computation.
  - `src/llm/ollama.rs` – Lightweight API client (if not already present)
    - `generate(prompt: &str, model: &str, max_tokens?: Option<u32>) -> Result<String>` via `reqwest` async, non‑streaming.
  - `src/mcp.rs`
    - Add `summarize` tool wiring; derive `JsonSchema` for input/output; use existing error type variants.
  - `src/qdrant.rs`
    - Add helper `find_summary_by_key(project_id, summary_key)` and reuse existing index/upsert helpers.

- Context7 Doc Hints (implementation references)
  - Ollama HTTP API (generate endpoint)
    - Library: /ollama/ollama
    - Example request: `POST /api/generate` with `{"model":"...","prompt":"...","stream":false}`
    - Use `options` if you need to cap context length or set temperature.
  - Reqwest JSON client (async)
    - Library: /seanmonstar/reqwest
    - Add deps: `reqwest = { version = "0.12", features=["json"] }`, `tokio = { version = "1", features=["full"] }`.
    - Pattern: `Client::new().post(url).json(&payload).send().await?.error_for_status()?;`
  - Qdrant filters and range
    - Library: /websites/api_qdrant_tech
    - Build `must` with `FieldCondition`/`MatchValue` for `project_id`, `memory_type`, and `Range` for `timestamp`.
  - Schemars + Serde integration for tool schemas
    - Library: /gresau/schemars
    - `#[derive(Serialize, Deserialize, JsonSchema)]`, honor `#[serde(rename_all="camelCase")]`.
  - UUID generation
    - Library: /uuid-rs/uuid – use `Uuid::new_v4()`.

- Extractive Fallback – Minimal Deterministic Algorithm
  - Input: array of episodic `{text, timestamp}`.
  - Steps: split text into sentences by `[.!?]`, trim, dedupe by `sha256(sentence)`, preserve chronological order, accumulate sentences until `max_words`, join with `. `.
  - Advantages: zero dependencies, predictable output for tests.

- Acceptance Criteria (expanded)
  - Input validation: rejects missing/invalid `time_range` and `limit <= 0` with `invalid_params` and clear messages.
  - Provider detection: `strategy=auto` yields `abstractive` with Ollama reachable; otherwise `extractive`.
  - Summary upsert: produces one new semantic memory with tag `summary` and payload `source_memory_ids` and is retrievable by MCP `search`.
  - Idempotency: repeated `summarize` with identical filters within the same dataset returns the existing summary (same `upserted_memory_id`).
  - Response: includes `used_filters` and `strategy`.

- How to Test (step‑by‑step)
  1) Ingest episodic memories
     - Use MCP `push` to index 3–5 episodic items within a small time window and `project_id=default` with tags `["note","daily"]`.
  2) Run summarize (abstractive)
     - Ensure Ollama is running and a small model is available (e.g., `llama3.1:8b`).
     - Call MCP `summarize` with `time_range` covering the new memories, `strategy=auto`, `max_words=120`.
     - Expect: non‑empty `summary`, `source_memory_ids` matches episodic IDs, `upserted_memory_id` present, `strategy="abstractive"`.
  3) Verify search discoverability
     - Call MCP `search` with `memory_type="semantic"`, `tags=["summary"]`, `project_id=default` and confirm the returned memory includes the upserted ID.
  4) Idempotency check
     - Repeat the same `summarize` call and verify `upserted_memory_id` is unchanged.
  5) Fallback path
     - Stop/disable Ollama or set `provider=none`, rerun `summarize` with `strategy=auto`; expect `strategy="extractive"` and a deterministic summary string.

- Expected Results
  - Abstractive path yields concise, coherent summaries with acceptable quality on small local models.
  - Extractive fallback is stable across runs and bounded by `max_words`.
  - Provenance recorded via `source_memory_ids` and idempotency via `summary:<hash>` tag or `summary_key` payload.

- Git Steps
  - Commit message: `Add MCP summarize (episodic → semantic consolidation)`
  - PR description should include: schema snippets, prompt example, `./scripts/verify.sh` output, and live validation commands run.

- Sample MCP Payloads
  - Request
    - `{"project_id":"default","time_range":{"start":"2025-01-01T00:00:00Z","end":"2025-01-07T23:59:59Z"},"tags":["daily"],"limit":25,"strategy":"auto","max_words":150}`
  - Response (shape)
    - `{"summary":"...","source_memory_ids":["..."],"upserted_memory_id":"...","used_filters":{...},"strategy":"abstractive","provider":"ollama","model":"llama3.1:8b"}`

Notes
- This milestone adds an optional `source_memory_ids` and optional `summary_key` to the universal payload. Update `docs/Configuration.md` and, if you show payload examples elsewhere in this document, include these as optional fields.
- Keep transport strictly stdio for MCP. Avoid introducing SSE/HTTP for clients; the server may use HTTP to call Ollama.

### M10 – Hardening & (Optional) gRPC Migration

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
