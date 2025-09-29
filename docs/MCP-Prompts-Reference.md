# Rusty Memory MCP Surface Reference

This reference captures what the MCP server tells agents: the main instructions (the “server prompt”), tool descriptions and arguments, and resource definitions. Content is sourced from:

- `src/mcp/server.rs` (instructions, tool/resource descriptions)
- `src/mcp/schemas.rs` (argument schemas and defaults)
- `src/mcp/handlers/*` (validation rules, responses)
- `src/mcp/format.rs` (response field names)

---

## Main Instructions (Server Prompt)

```text
Use this server to index, search, and summarize project memories for agents. Index source text, then retrieve concise context via semantic search with project/type/tag/time filters; summarize time‑bounded entries when needed.
```

---

## Tools

### Search (search)

Purpose

- Semantic search over indexed memories. Keep `query_text` concise (≤ 512 chars). Prefer filters for precision.

Arguments

| Name              | Type     | Required | Default                          | Notes                                                                                             |
| ----------------- | -------- | -------- | -------------------------------- | ------------------------------------------------------------------------------------------------- |
| `query_text`      | string   | yes      | —                                | Text to embed and search                                                                          |
| `project_id`      | string   | no       | `default`                        | Filters results; also accepted as `project`                                                       |
| `memory_type`     | enum     | no       | —                                | `episodic`                                                                                        |
| `tags`            | string[] | no       | —                                | Contains-any; scalar coerced to array; must be non-empty strings                                  |
| `time_range`      | object   | no       | —                                | `{ start?: "2025-01-01T00:00:00Z", end?: "2025-12-31T23:59:59Z" }`; start ≤ end when both present |
| `limit`           | integer  | no       | `SEARCH_DEFAULT_LIMIT`           | 1..`SEARCH_MAX_LIMIT`; alias `k`                                                                  |
| `score_threshold` | number   | no       | `SEARCH_DEFAULT_SCORE_THRESHOLD` | 0.0..1.0                                                                                          |
| `collection`      | string   | no       | default collection               | Override target collection                                                                        |

Note

- Timestamp strings use RFC3339 (ISO‑8601) format, for example `YYYY-MM-DDTHH:MM:SSZ` or with an offset like `YYYY-MM-DDTHH:MM:SS-07:00`.

Response

- `results[]`: items include `id`, `score`, optional `text`, `project_id`, `memory_type`, `tags`, `timestamp`, `source_uri`.
- `context` (optional): prompt-ready text with `[id]` citations.
- `collection`, `limit`, `score_threshold` and `scoreThreshold` (compatibility), `used_filters` (echo of applied filters).

Compatibility & Aliases

- Aliases: `project` → `project_id`, `type` → `memory_type`, `k` → `limit`.
- Scalar `tags` are accepted and coerced into arrays.

---

### Index Document (push)

Purpose

- Split text into chunks, embed, and upsert into Qdrant.

Arguments

| Name          | Type     | Required | Default    | Notes                              |
| ------------- | -------- | -------- | ---------- | ---------------------------------- |
| `text`        | string   | yes      | —          | Document contents to index         |
| `collection`  | string   | no       | default    | Collection override                |
| `project_id`  | string   | no       | `default`  | Project label persisted in payload |
| `memory_type` | enum     | no       | `semantic` | `episodic`                         |
| `tags`        | string[] | no       | —          | Tags applied to each chunk         |
| `source_uri`  | string   | no       | —          | File path or URL for provenance    |

Response

- `{ status: "ok", collection, chunksIndexed, chunkSize, inserted, updated, skippedDuplicates }`.

---

### Summarize Memories (summarize)

Purpose

- Consolidate time-bounded episodic memories into a semantic summary with provenance.

Arguments

| Name          | Type     | Required | Default                   | Notes                                                                           |
| ------------- | -------- | -------- | ------------------------- | ------------------------------------------------------------------------------- |
| `project_id`  | string   | no       | `default`                 | Optional project scope                                                          |
| `memory_type` | enum     | no       | `episodic`                | `episodic`                                                                      |
| `tags`        | string[] | no       | —                         | Contains-any tag filter                                                         |
| `time_range`  | object   | yes      | —                         | `{ start: "2025-01-01T00:00:00Z", end: "2025-01-02T00:00:00Z" }`; both required |
| `limit`       | integer  | no       | `50`                      | Capped by `SEARCH_MAX_LIMIT`                                                    |
| `strategy`    | enum     | no       | `auto`                    | `auto`                                                                          |
| `provider`    | enum     | no       | —                         | `ollama`                                                                        |
| `model`       | string   | no       | —                         | Provider-specific model when abstractive                                        |
| `max_words`   | integer  | no       | `SUMMARIZATION_MAX_WORDS` | > 0                                                                             |
| `collection`  | string   | no       | default                   | Collection override                                                             |

Note

- Timestamp strings use RFC3339 (ISO‑8601) format, for example `YYYY-MM-DDTHH:MM:SSZ` or with an offset like `YYYY-MM-DDTHH:MM:SS+01:00`.

Response

- `{ summary, source_memory_ids, upserted_memory_id, strategy, provider?, model?, used_filters }`.

---

### List Collections (get-collections)

Purpose

- Discover Qdrant collections known to the server.

Arguments

- `{}` (no arguments).

Response

- `{ collections: string[] }`.

---

### Create/Resize Collection (new-collection)

Purpose

- Ensure a collection exists with the desired vector size.

Arguments

| Name          | Type    | Required | Default               | Notes            |
| ------------- | ------- | -------- | --------------------- | ---------------- |
| `name`        | string  | yes      | —                     | Collection name  |
| `vector_size` | integer | no       | `EMBEDDING_DIMENSION` | Vector dimension |

Response

- `{ status: "ok", vectorSize }`.

---

### Metrics Snapshot (metrics)

Purpose

- Observe ingestion counters.

Arguments

- `{}` (no arguments).

Response

- `{ documentsIndexed, chunksIndexed, lastChunkSize }` (lastChunkSize may be null before first ingestion).

---

## Resources

### Memory Types

- URI: `mcp://memory-types`
- Purpose: Supported `memory_type` values and default.
- Example payload:

```json
{ "memory_types": ["episodic", "semantic", "procedural"], "default": "semantic" }
```

### Health

- URI: `mcp://health`
- Purpose: Embedding configuration and Qdrant reachability snapshot.
- Example payload:

```json
{
  "embedding": { "provider": "ollama", "model": "...", "dimension": 768 },
  "qdrant": {
    "url": "http://127.0.0.1:6333",
    "reachable": true,
    "defaultCollection": "rusty-mem",
    "defaultCollectionPresent": true
  }
}
```

### Projects

- URI: `mcp://projects`
- Purpose: Distinct `project_id` values observed.
- Example payload:

```json
{ "projects": ["default", "repo-a"] }
```

### Settings

- URI: `mcp://settings`
- Purpose: Effective search defaults.
- Example payload:

```json
{ "search": { "default_limit": 5, "max_limit": 50, "default_score_threshold": 0.25 } }
```

### Usage

- URI: `mcp://usage`
- Purpose: Usage policy and recommended flows.
- Example payload (exact):

```json
{
  "title": "Rusty Memory MCP Usage",
  "policy": [
    "Do not paste or concatenate large documents in prompts.",
    "Index text with `push` first; use `search` to retrieve.",
    "Prefer filters: project_id, memory_type, tags, time_range.",
    "Keep query_text concise (<= 512 chars).",
    "Use summarize to consolidate episodic → semantic summaries."
  ],
  "flows": [
    {
      "name": "Ingest & Retrieve",
      "steps": [
        "push({ text, project_id?, memory_type?, tags? })",
        "search({ query_text, project_id?, memory_type?, tags?, time_range? })"
      ]
    },
    {
      "name": "Summarize",
      "steps": [
        "search episodic within time_range",
        "summarize({ project_id, time_range, tags?, limit?, max_words? })"
      ]
    }
  ]
}
```

### Project Tags (templated)

- URI template: `mcp://{project_id}/tags`
- Purpose: Distinct tags for a specific project.
- Example payload:

```json
{ "project_id": "repo-a", "tags": ["alpha", "beta"] }
```

---

## Validation & Defaults (At a Glance)

- Search defaults derive from env: `SEARCH_DEFAULT_LIMIT`, `SEARCH_MAX_LIMIT`, `SEARCH_DEFAULT_SCORE_THRESHOLD`.
- `project_id` defaults to `default` when omitted (both push/search/summarize sanitize it).
- Search `time_range` accepts either bound; summarize requires both.
- Responses include consistent field names; search duplicates `score_threshold` as `scoreThreshold` for compatibility.
