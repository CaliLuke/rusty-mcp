# Editor Integrations

This guide shows how to wire the Rusty Memory MCP server into popular editors and agents. All examples assume the binary is installed via:

```bash
cargo install rustymcp
```

and that you configured environment variables as described in [Configuration](./Configuration.md).

## Claude Desktop

Claude Desktop reads MCP servers from `~/.claude/claude_desktop_config.json`.

```json
{
  "mcpServers": {
    "rusty_mem": {
      "command": "rusty_mem_mcp",
      "args": [],
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

Notes:

- Claude launches MCP servers as child processes using stdio transport.
- If the binary is not on PATH, set `command` to the full path (e.g., `/Users/you/.cargo/bin/rusty_mem_mcp`).

## Codex CLI

Add the server to your `config.toml`:

```toml
[mcp_servers.rusty_mem]
command = "rusty_mem_mcp" # use full path if not on PATH
args = []
transport = "stdio"

  [mcp_servers.rusty_mem.env]
  QDRANT_URL = "http://127.0.0.1:6333"
  QDRANT_COLLECTION_NAME = "rusty-mem"
  EMBEDDING_PROVIDER = "ollama"
  EMBEDDING_MODEL = "nomic-embed-text"
  EMBEDDING_DIMENSION = "768"
```

## Kilo, Cline, Roo Code (JSON)

Most JSON-based clients share a compatible `mcpServers` shape. Use this as a starting point in the editor’s settings file and adjust the paths as needed.

```json
{
  "mcpServers": {
    "rusty_mem": {
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

## Tips

- Ensure `~/.cargo/bin` is on your `PATH` if your editor resolves commands via PATH.
- On Windows, escape backslashes in JSON paths (e.g. `"C:\\Users\\you\\.cargo\\bin\\rusty_mem_mcp.exe"`).
- If your editor requires a working directory, you can set `cwd` to any folder (not required for Rusty Memory to operate).
- The server exits if required environment variables are missing; check logs or the editor’s MCP console.
