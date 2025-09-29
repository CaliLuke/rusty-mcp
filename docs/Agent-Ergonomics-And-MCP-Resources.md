# Agent Ergonomics Assessment and MCP Resources Plan

This note evaluates agent ergonomics of the current plan and proposes concrete improvements. It also recommends using MCP "resources" for static/discoverable data (projects, tags, memory types) instead of bespoke list-* tools.

## Summary

- The cognitive model, payload schema, and hybrid retrieval are solid.
- Ergonomics can improve with simpler defaults, flexible inputs, better result shapes, and richer tool metadata/examples.
- Use MCP resources (read-only, discoverable) for static/enumerable data to reduce tool surface area and improve planner reliability.

## Recommendations (Agent-Facing)

- Defaults that “just work”
  - `project_id`: default `"default"`
  - `memory_type`: default `"semantic"`
  - `limit`: default `5`, max cap `50`
  - `tags`: contains-any semantics by default
  - `score_threshold`: default `0.25` (configurable)
- Flexible inputs (aliases and coercions)
  - Accept `type` → `memory_type`, `project` → `project_id`, `k` → `limit`
  - Coerce `tags: string` → `tags: [string]`
- Ingestion parity with universal payload
  - Extend `push` to accept optional `project_id`, `memory_type`, `tags`, `source_uri`
  - Surface dedupe effects via `chunk_hash` counters: `inserted`, `updated`, `skippedDuplicates`
- Add `search` now (M3)
  - Input: `{ query_text, project_id?, memory_type?, tags?, time_range?: { start, end }, limit?, score_threshold?, collection? }`
  - Response: `{ hits: [...], context: string, used_filters: {...}, limit, score_threshold }`
    - Each hit: `{ id, score, text, project_id, memory_type, tags, timestamp, source_uri }`
    - `context` is a joined snippet with inline `[id]` citations for drop-in prompting
- Error ergonomics
  - Map empty/invalid to `INVALID_PARAMS` with actionable hints
  - Upstream failures as internal errors with remediation (“Qdrant unreachable: check http://localhost:6333”)
- Tool discoverability and guidance
  - Add concise “When to use” and 2 examples per tool (happy-path, advanced)
  - Keep a single ingestion tool (`push`) to avoid redundant aliases

## Use MCP Resources for Static/Discoverable Data

Instead of adding `list-tags`, `list-projects`, etc. as tools, expose read-only MCP resources that agents can discover with `listResources` and fetch via `readResource`. Benefits:

- Simpler surface: fewer tools; clients already support `listResources`
- Better planner behavior: static URIs are easy for LLMs to reason about
- Caching-friendly: hosts can cache resources between calls
- Clear semantics: read-only, no side effects

### Proposed Resources

- `mcp://rusty-mem/memory-types`
  - MIME: `application/json`
  - Body: `{ "memory_types": ["episodic", "semantic", "procedural"] }`
- `mcp://rusty-mem/projects`
  - MIME: `application/json`
  - Body: `{ "projects": ["default", "proj-A", ...] }`
  - Source: enumerate distinct `project_id` values from payload index
- `mcp://rusty-mem/projects/{project_id}/tags`
  - MIME: `application/json`
  - Body: `{ "project_id": "...", "tags": ["build", "docs", ...] }`
  - Source: distinct tag values for that project (contains-any semantics in search)
- `mcp://rusty-mem/health`
  - MIME: `application/json`
  - Body: `{ "embedding_provider": "ollama|deterministic", "embedding_model": "...", "qdrant": { "reachable": true, "collection": "...", "vector_size": 1536 } }`
  - Purpose: fast preflight for agents

Notes:

- Resources can be dynamic (derived from Qdrant), but remain read-only.
- If a tag list could be large, keep project-scoped URIs and paginate by splitting into multiple resources (e.g., `.../tags?page=2`) only if necessary.

## Tool Schemas and Metadata (MCP)

- `push` (Index Document)
  - Required: `text`
  - Optional: `project_id`, `memory_type` (enum), `tags`, `source_uri`, `collection`
  - Response: `{ status, collection, chunksIndexed, chunkSize, inserted, updated, skippedDuplicates }`
  - Annotations: `destructive: false`, `idempotent: best-effort`, `open_world: false`
- `search` (Retrieve with filters)
  - Required: `query_text`
  - Optional: `project_id`, `memory_type`, `tags`, `time_range { start, end }`, `limit`, `score_threshold`, `collection`
  - Response: `{ hits: [...], context, used_filters, limit, score_threshold }`
  - Annotations: `read_only: true`, `idempotent: true`
- `summarize` (Later, M5)
  - Input: `{ project_id?, time_range, tags? }`
  - Response: `{ summary, source_memory_ids }`

Enrich JSON Schemas with `enum`, `default`, and `examples` to help LLMs form correct calls.

## MCP Resources vs Tools: When to Use Which

- Use Resources for:
  - Static or enumerable data (memory types), environment/config snapshots (health), catalog-like discovery (projects/tags)
- Use Tools for:
  - Operations (index/push), queries with parameters (search), transformations (summarize)

This split keeps the tools minimal and focused, while resources provide context and discovery.

## Implementation Sketch (rmcp server)

- Capabilities: enable both tools and resources in `get_info`
- Implement `list_resources`/`read_resource` for the URIs above
- Keep existing tools (`push`, `get-collections`, `new-collection`, `metrics`); add `search` next
- Populate resource bodies as JSON with `application/json` MIME

## Quickstart (Agent-Facing)

1. Discover resources: `listResources` → read `memory-types`, `projects`, `.../tags`
2. Index content: call `push` with `text` and optional metadata
3. Search: call `search` with `query_text` (+ optional filters)
4. Use `context` from search for generation; or pivot to `summarize` by time window (later)

## Open Questions / Decisions

- Pagination for tag resources if cardinality grows (defer; start simple)
- Whether to alias `index` → `push` in `describe_tools` (likely yes; improves discoverability)
- `score_threshold` default value (start at 0.25; expose via config)

## Next Steps

- Implement `search` with defaults and response shape above
- Extend `push` inputs and outputs for metadata and dedupe counters
- Add MCP resources (memory-types, projects, project tags, health) and enable resource capability
- Update tool descriptions and examples in `describe_tools` and `get_info.instructions`
