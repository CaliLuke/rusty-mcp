# Rusty Memory MCP

[![Crates.io](https://img.shields.io/crates/v/rustymcp.svg?style=flat-square)](https://crates.io/crates/rustymcp)
[![docs.rs](https://docs.rs/rustymcp/badge.svg)](https://docs.rs/rustymcp)
[![CI](https://github.com/CaliLuke/rusty-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/CaliLuke/rusty-mcp/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-PolyForm%20Noncommercial-blue?style=flat-square)](LICENSE)

Rusty Memory MCP (crate: `rustymcp`) is a compact memory server for agents:

- Splits text into semantic chunks with model‑aware token budgets.
- Generates embeddings and stores vectors in Qdrant.
- Exposes both an HTTP API and an MCP stdio server.

See internals and architecture in docs/Design.md.

## Quick start

4. **Install the CLI (recommended)**
   Skip local builds by installing the published binary:

   ```bash
   cargo install rustymcp
   ```

   This places three executables in `~/.cargo/bin`:
   - `rustymcp` → HTTP server entrypoint
   - `rusty_mem_mcp` → MCP stdio server
   - `metrics-post` → helper used by the metrics script

5. **Run the MCP server**

   ```bash
   cargo run --bin rusty_mem_mcp
   ```

   or, if you installed the crate, launch it directly:

   ```bash
   ~/.cargo/bin/rusty_mem_mcp
   ```

   Building from source once also works:

   ```bash
   cargo build --release --bin rusty_mem_mcp
   ./target/release/rusty_mem_mcp
   ```

6. **Run the HTTP server** (optional if you only need MCP)

```bash
cargo run
```

The server listens on `SERVER_PORT` when that variable is set; otherwise it scans `4100-4199` and binds the first available port. Successful `POST /index` calls return `{ "chunks_indexed": <count>, "chunk_size": <tokens> }` so callers can observe the automatic budget.

or, using the installed binary:

```bash
~/.cargo/bin/rustymcp
```

7. **Point your agent at the server**
   - Codex CLI (TOML):

     ```toml
     [mcp_servers.rusty_mem]
     command = "/full/path/to/target/release/rusty_mem_mcp"
     cwd = "/full/path/to/rusty-mcp"
     transport = "stdio"

       [mcp_servers.rusty_mem.env]
       QDRANT_URL = "http://127.0.0.1:6333"
       QDRANT_COLLECTION_NAME = "rusty-mem"
       EMBEDDING_PROVIDER = "ollama"
       EMBEDDING_MODEL = "nomic-embed-text"
       EMBEDDING_DIMENSION = "768"
       OLLAMA_ENDPOINT = "http://127.0.0.1:11434"
     ```

   - JSON-based clients (Kilo, Cline, Roo Code):

     ```json
     {
       "mcpServers": {
         "rusty": {
           "command": "/full/path/to/target/release/rusty_mem_mcp",
           "args": [],
           "cwd": "/full/path/to/rusty-mcp",
           "transport": "stdio",
           "env": {
             "QDRANT_URL": "http://127.0.0.1:6333",
             "QDRANT_COLLECTION_NAME": "rusty-mem",
             "EMBEDDING_PROVIDER": "ollama",
             "EMBEDDING_MODEL": "nomic-embed-text",
             "EMBEDDING_DIMENSION": "768",
             "OLLAMA_ENDPOINT": "http://127.0.0.1:11434"
           }
         }
       }
     }
     ```

   `TEXT_SPLITTER_CHUNK_SIZE` is now optional; the server infers a sensible value from the embedding model when the variable is omitted.

8. **Use the built-in tools**
   - `get-collections` → list available Qdrant collections (`{}` payload).
   - `new-collection` → create or resize (`{ "name": "docs", "vector_size": 768 }`).
   - `push` → ingest (`{ "text": "my note", "collection": "docs" }`). The response echoes `chunksIndexed` and the effective `chunkSize`.
   - `metrics` → check `{ "documentsIndexed": …, "chunksIndexed": … }`.
     When documents have been ingested, the payload also includes `lastChunkSize`.

## HTTP API quick reference

Endpoints (all return JSON on success unless noted):

- `POST /index` – Index a document into the default or provided collection.
  - Request: `{ "text": string, "collection"?: string }`
  - Response: `{ "chunks_indexed": number, "chunk_size": number }`
- `GET /collections` – List Qdrant collections.
- `POST /collections` – Create or resize a collection.
  - Request: `{ "name": string, "vector_size"?: number }`
- `GET /metrics` – Read counters: `{ "documents_indexed": number, "chunks_indexed": number, "last_chunk_size"?: number }`

## Configuration

Most deployments only set: `QDRANT_URL`, `QDRANT_COLLECTION_NAME`, `EMBEDDING_PROVIDER`, `EMBEDDING_MODEL`, and `EMBEDDING_DIMENSION`. See the full reference in [docs/Configuration.md](docs/Configuration.md).

## License

This project is licensed under the [PolyForm Noncommercial License 1.0.0](LICENSE). It is free for noncommercial use. Commercial use is not permitted by this license.

## Contributing

Contributions are welcome via pull requests. Please review the [Quality Manual](docs/QUALITY_MANUAL.md) for code style, testing, and automation expectations.

- Editor integrations (Claude, Codex CLI, Kilo, Cline, Roo Code) live in [docs/Editors.md](docs/Editors.md) to keep this README focused.
