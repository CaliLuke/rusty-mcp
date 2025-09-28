# Configuration Guide

This document explains every configuration option supported by Rusty Memory and walks through the two most common ways to wire the server into an MCP-aware client. The aim is to keep the setup approachable even if this is your first time running an agent with a memory backend.

## Environment variables

Rusty Memory reads its configuration from environment variables once at startup. The easiest way to manage them is to copy `.env.example` to `.env` and edit the values. The table below lists each variable, what it does, and typical values.

| Variable                   | Description                                                                                                                    | Example                           |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------ | --------------------------------- |
| `QDRANT_URL`               | Base URL for the Qdrant HTTP API.                                                                                              | `http://127.0.0.1:6333`           |
| `QDRANT_COLLECTION_NAME`   | Default collection name used when `push` does not provide one.                                                                 | `rusty-mem`                       |
| `QDRANT_API_KEY`           | Optional API key for secured Qdrant deployments. Leave empty for local installs.                                               | `supersecretapikey`               |
| `EMBEDDING_PROVIDER`       | Logical provider name used for logging and future provider-specific behaviour. Accepted values today: `ollama`, `openai`.      | `ollama`                          |
| `EMBEDDING_MODEL`          | Model identifier passed to the embedding client. For Ollama, this is the model tag; for cloud providers, use their model slug. | `nomic-embed-text`                |
| `EMBEDDING_DIMENSION`      | Vector length expected by the target collection. Must match the embedding model’s output dimension.                            | `768`                             |
| `TEXT_SPLITTER_CHUNK_SIZE` | Maximum token count per chunk. The chunker counts whitespace-delimited tokens.                                                 | `1024`                            |
| `SERVER_PORT`              | Optional fixed HTTP port. When unset, the server picks the first free port in `4100-4199`.                                     | `4123`                            |
| `OLLAMA_ENDPOINT`          | Base URL of the Ollama server. Required when `EMBEDDING_PROVIDER=ollama`.                                                      | `http://127.0.0.1:11434`          |
| `RUSTY_MEM_LOG_FILE`       | Optional absolute path for structured logs. When omitted, logs go to `logs/rusty-mem.log`.                                     | `/Users/you/rusty-mem.log`        |
| `RUST_LOG`                 | Standard Rust logging filter if you need more or less verbosity.                                                               | `rusty_mem=debug,tower_http=info` |

### Switching to hosted providers

If you prefer OpenAI or another hosted provider, update the environment variables accordingly:

```env
EMBEDDING_PROVIDER=openai
EMBEDDING_MODEL=text-embedding-3-small
EMBEDDING_DIMENSION=1536
OPENAI_API_KEY=sk-...
```

Rusty Memory reads provider-specific credentials directly from the environment, so no code changes are required.

## MCP configuration templates

Most agent platforms accept either TOML (Codex CLI style) or JSON (Kilo, Cline, Roo Code). The sections below show complete examples. Adjust the paths to match your local checkout and the environment variables you just configured.

### TOML example (Codex CLI)

```toml
[mcp_servers.rusty_mem]
command = "/Users/you/code/rusty-mem/target/release/rusty_mem_mcp"
args = []
cwd = "/Users/you/code/rusty-mem"
transport = "stdio"

  [mcp_servers.rusty_mem.env]
  QDRANT_URL = "http://127.0.0.1:6333"
  QDRANT_COLLECTION_NAME = "rusty-mem"
  EMBEDDING_PROVIDER = "ollama"
  EMBEDDING_MODEL = "nomic-embed-text"
  EMBEDDING_DIMENSION = "768"
  TEXT_SPLITTER_CHUNK_SIZE = "1024"
  OLLAMA_ENDPOINT = "http://127.0.0.1:11434"
```

**Step-by-step for new users**

1. Build the release binary once with `cargo build --release --bin rusty_mem_mcp`.
2. Copy the snippet above into your Codex `config.toml`.
3. Replace the `/Users/you/...` paths with the actual location of your checkout.
4. Save the file and restart Codex; the MCP server appears in the server list.

### JSON example (Kilo Code, Cline, Roo Code)

```json
{
  "mcpServers": {
    "rusty": {
      "command": "/Users/you/code/rusty-mem/target/release/rusty_mem_mcp",
      "args": [],
      "cwd": "/Users/you/code/rusty-mem",
      "transport": "stdio",
      "env": {
        "QDRANT_URL": "http://127.0.0.1:6333",
        "QDRANT_COLLECTION_NAME": "rusty-mem",
        "EMBEDDING_PROVIDER": "ollama",
        "EMBEDDING_MODEL": "nomic-embed-text",
        "EMBEDDING_DIMENSION": "768",
        "TEXT_SPLITTER_CHUNK_SIZE": "1024",
        "OLLAMA_ENDPOINT": "http://127.0.0.1:11434"
      }
    }
  }
}
```

**Step-by-step for new users**

1. Ensure the binary path and working directory are correct for your machine.
2. If you are running on Windows, remember to escape backslashes (`"C:\\Users\\you\\code"`).
3. Paste the configuration into the MCP section of your editor.
4. Reload the agent; the server should appear immediately.

### Tips for first-time users

- If the agent says it cannot reach Qdrant, check that the `QDRANT_URL` host/port are accessible from the agent machine.
- Ollama must have the requested model pulled (`ollama pull nomic-embed-text`). The server logs a clear error if the model is missing.
- To disable file logging during experiments, set `RUSTY_MEM_LOG_FILE=/dev/null` before launching the binary.

## Verifying your setup

1. Launch the MCP binary manually. You should see logs similar to `Initializing embedding client` followed by `Service initialized as client` once an MCP host connects.
2. From your agent, run the `get-collections` tool. An empty array is fine the first time.
3. Run the `push` tool with a short piece of text. Check the `metrics` tool—`documentsIndexed` and `chunksIndexed` should increment.
4. Inspect Qdrant (via UI or `curl`) to confirm the collection contains points.

You are now ready to use Rusty Memory as the backing store for your coding agent. If you run into issues or have ideas that would help new developers, please open an issue—accessibility and simplicity are the project’s north stars.
