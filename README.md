# Rusty Memory MCP

[![Crates.io](https://img.shields.io/crates/v/rustymcp.svg?style=flat-square)](https://crates.io/crates/rustymcp)
[![docs.rs](https://docs.rs/rustymcp/badge.svg)](https://docs.rs/rustymcp/0.1.1)
[![Release](https://img.shields.io/badge/release-v0.1.1-blue?style=flat-square)](https://github.com/CaliLuke/rusty-mcp/releases/tag/v0.1.1)
[![CI](https://github.com/CaliLuke/rusty-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/CaliLuke/rusty-mcp/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-PolyForm%20Noncommercial-blue?style=flat-square)](LICENSE)

Rusty Memory MCP is a no‑fuss memory server for coding agents. It chunks text, generates embeddings, and stores vectors in Qdrant. You can run it as:

- An MCP server over stdio for editors/agents
- A local HTTP service for simple scripts

If you want to hack on the codebase or learn how it works internally, jump to Developer Docs below.

## Quick Start (MCP in your editor)

1. Install the binaries

   ```bash
   cargo install rustymcp
   ```

   This installs:
   - `rusty_mem_mcp` (MCP server)
   - `rustymcp` (HTTP server)
   - `metrics-post` (helper used by scripts)

2. Start Qdrant (vector database)

   ```bash
   docker run --pull=always -p 6333:6333 qdrant/qdrant:latest
   ```

3. Configure environment

   Copy `.env.example` to `.env` and edit values, or provide the same variables via your MCP client config:

   Required variables:
   - `QDRANT_URL` (e.g. `http://127.0.0.1:6333`)
   - `QDRANT_COLLECTION_NAME` (e.g. `rusty-mem`)
   - `EMBEDDING_PROVIDER` (`ollama` or `openai` — used for logging today)
   - `EMBEDDING_MODEL` (free‑form, e.g. `nomic-embed-text`)
   - `EMBEDDING_DIMENSION` (must match your model, e.g. `768`)

4. Launch the MCP server

   ```bash
   rusty_mem_mcp
   ```

5. Add to your agent

   - Codex CLI (`~/.codex/config.toml`):

     ```toml
     [mcp_servers.rusty_mem]
     command = "rusty_mem_mcp" # or use the full path to the binary
     args = []
     transport = "stdio"

       [mcp_servers.rusty_mem.env]
       QDRANT_URL = "http://127.0.0.1:6333"
       QDRANT_COLLECTION_NAME = "rusty-mem"
       EMBEDDING_PROVIDER = "ollama"
       EMBEDDING_MODEL = "nomic-embed-text"
       EMBEDDING_DIMENSION = "768"
     ```

   - JSON clients (Kilo, Cline, Roo Code):

     ```json
     {
       "mcpServers": {
         "rusty": {
           "command": "rusty_mem_mcp",
           "args": [],
           "transport": "stdio",
           "env": {
             "QDRANT_URL": "http://127.0.0.1:6333",
             "QDRANT_COLLECTION_NAME": "rusty-mem",
             "EMBEDDING_PROVIDER": "ollama",
             "EMBEDDING_MODEL": "nomic-embed-text",
             "EMBEDDING_DIMENSION": "768"
           }
         }
       }
     }
     ```

6. Try it

   From your agent, use:
   - `get-collections` → list Qdrant collections
   - `new-collection` → create or resize a collection
   - `push` → index text into a collection
   - `metrics` → view counters (`documentsIndexed`, `chunksIndexed`, `lastChunkSize`)

## Optional: Run the HTTP server

If you prefer HTTP, set the same environment and run:

```bash
rustymcp
```

By default the server binds the first free port in `4100–4199` (or `SERVER_PORT` if set). Example:

```bash
curl -sS -X POST http://127.0.0.1:4100/index \
  -H 'Content-Type: application/json' \
  -d '{"text":"hello from http"}'
```

Returns `{ "chunks_indexed": <number>, "chunk_size": <number> }` on success.

Having trouble? See `docs/Troubleshooting.md`.

## Developer Docs

If you want to extend Rusty Memory or understand the architecture:

- Configuration reference and client snippets: `docs/Configuration.md`
- Architecture and module overview: `docs/Design.md`
- Editor integration examples: `docs/Editors.md`
- Development guide (style, tests, scripts): `docs/Development.md`

## License

Licensed under the [PolyForm Noncommercial License 1.0.0](LICENSE). Free for non‑commercial use.

## Support

Issues and PRs are welcome. If anything in the Quick Start is unclear, please open an issue so we can make onboarding even smoother.
