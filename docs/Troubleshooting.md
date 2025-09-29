# Troubleshooting

Quick fixes for the most common issues when running Rusty Memory as an MCP server or HTTP service.

## MCP server not found / command not found

- Ensure Cargo bin is on your `PATH`:
  - macOS/Linux: `export PATH="$HOME/.cargo/bin:$PATH"`
  - Windows (PowerShell): `$env:Path += ";$env:USERPROFILE\.cargo\bin"`
- Use the full path in your MCP config if needed: `"/Users/you/.cargo/bin/rusty_mem_mcp"`.

## Qdrant connection errors (ECONNREFUSED, 404)

- Start Qdrant locally:

  ```bash
  docker run --pull=always -p 6333:6333 qdrant/qdrant:latest
  ```

- Verify the API responds:

  ```bash
  curl -sS http://127.0.0.1:6333/collections | head -n1
  ```

- Check your MCP config `env` block for `QDRANT_URL` typos and port.

## Missing environment variables

If `rusty_mem_mcp` exits immediately, required env vars may be missing. Minimal set:

```env
QDRANT_URL=http://127.0.0.1:6333
QDRANT_COLLECTION_NAME=rusty-mem
EMBEDDING_PROVIDER=ollama
EMBEDDING_MODEL=nomic-embed-text
EMBEDDING_DIMENSION=768
```

Prefer setting them directly in your MCP config’s `env` section so the server always sees them.

## Collection vector size mismatch

If Qdrant already has a collection with a different vector size than `EMBEDDING_DIMENSION`, indexing fails. Fix by ensuring the vector size:

- From your agent, call `new-collection` with the desired `vector_size` (e.g., `768` or `1536`).
- Or via HTTP:

  ```bash
  curl -sS -X POST http://127.0.0.1:4100/collections \
    -H 'Content-Type: application/json' \
    -d '{"name":"rusty-mem","vector_size":768}'
  ```

## HTTP server port issues

- By default the server binds the first free port in `4100–4199`.
- Pin a port with `SERVER_PORT=4123 rustymcp`.
- See which process holds a port (macOS/Linux): `lsof -i :4123`.

## Logging and where to look

- Default file log: `logs/rusty-mem.log` in the current working directory.
- Override path: set `RUSTY_MEM_LOG_FILE=/absolute/path/rusty-mem.log`.
- Increase verbosity: `RUST_LOG=rustymcp=debug,reqwest=info`.

## Windows path quirks in JSON configs

- Escape backslashes in JSON:

  ```json
  "command": "C:\\Users\\you\\.cargo\\bin\\rusty_mem_mcp.exe"
  ```

## Validate the end-to-end path

1. Launch Qdrant.
2. Start `rusty_mem_mcp`.
3. From your agent, run `get-collections` (empty is fine initially).
4. Run `push` with a short text; then `metrics` should show non-zero counters.

Still stuck? Open an issue with your OS, agent, config snippet (redact secrets), and logs.
