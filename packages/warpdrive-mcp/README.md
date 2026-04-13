# warpdrive-mcp

[Model Context Protocol](https://modelcontextprotocol.io) server for the WarpDrive platform.

Exposes WarpDrive operations to AI assistants (Claude, Cursor, VS Code Copilot, etc.) — scaffold and build WASM components, upload binaries, deploy services (Vectr circuits), simulate triggers, and query a live WarpDrive node, all from natural language.

---

## Installation

### One-command setup (recommended)

```bash
npx @warpdrive/mcp@latest
```

Interactive wizard: installs the binary, prompts for URL + token + credentials, writes `~/.claude.json` and `~/.warpdrive/warpdrive.toml`, and installs Claude Code skill files.

### Global install

```bash
npm install -g @warpdrive/mcp
```

The postinstall script downloads the correct pre-built binary for your platform.

### Build from source

```bash
cargo build --release -p warpdrive-mcp
# Binary: ./target/release/warpdrive-mcp
```

---

## Running

```bash
warpdrive-mcp --warpdrive-url http://localhost:8000 --token <your-token>
```

### All flags

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--warpdrive-url` | `WARPDRIVE_URL` | `http://localhost:8000` | WarpDrive node HTTP API URL |
| `--token` | `WARPDRIVE_TOKEN` | — | Bearer token for write operations |
| `--mcp-chain-credential` | `WARPDRIVE_MCP_CHAIN_CREDENTIAL` | — | Private key (`0x…`) or BIP39 mnemonic for on-chain transactions |
| `--signing-mnemonic` | `WARPDRIVE_SIGNING_MNEMONIC` | — | BIP39 mnemonic for the WarpDrive node (Vectr) signing key |

All flags can be set as environment variables — useful for running standalone without exposing secrets in `ps aux`.

---

## Credential Storage

On-chain tools (`warpdrive_deploy_service_manager`, `warpdrive_register_operator`, `warpdrive_set_service_uri`, etc.) need `mcp_chain_credential` and/or `signing_mnemonic`. Recommended storage in priority order:

### 1. `~/.warpdrive/warpdrive.toml` (recommended — works with all MCP clients)

```toml
[warpdrive]
mcp_chain_credential = "0x<private-key>"
signing_mnemonic = "word1 word2 ... word12"
```

`warpdrive-mcp` reads this file automatically. Only `~/.warpdrive/warpdrive.toml` is searched — project-local `warpdrive.toml` files are intentionally skipped to prevent accidental credential commits.

The WarpDrive desktop app's "Register with Claude" button and `just setup-claude-mcp` write this file automatically.

### 2. Environment variables (per-client overrides)

Set in your shell or in the MCP client's `"env"` block:

```bash
export WARPDRIVE_MCP_CHAIN_CREDENTIAL="0x<private-key>"
export WARPDRIVE_SIGNING_MNEMONIC="word1 word2 ... word12"
```

### 3. CLI flags (avoid — visible in `ps aux`)

```bash
warpdrive-mcp --mcp-chain-credential 0x... --signing-mnemonic "word1 ..."
```

---

## Client Configuration

### Claude Code

```bash
# One-command setup (recommended)
npx @warpdrive/mcp@latest

# From the WarpDrive repo
just setup-claude-mcp /path/to/your-project
```

### Claude Desktop (macOS)

Edit `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "warp-drive": {
      "command": "warpdrive-mcp",
      "args": ["--warpdrive-url", "http://localhost:8000", "--token", "your-token"]
    }
  }
}
```

Credentials are read automatically from `~/.warpdrive/warpdrive.toml`.

### Claude Desktop (Linux)

Same config, edit `~/.config/Claude/claude_desktop_config.json`.

### Cursor

Edit `.cursor/mcp.json` (project) or `~/.cursor/mcp.json` (global):

```json
{
  "mcpServers": {
    "warp-drive": {
      "command": "warpdrive-mcp",
      "args": ["--warpdrive-url", "http://localhost:8000", "--token", "your-token"]
    }
  }
}
```

### VS Code (Copilot / MCP extension)

Edit `.vscode/mcp.json`:

```json
{
  "servers": {
    "warp-drive": {
      "type": "stdio",
      "command": "warpdrive-mcp",
      "args": ["--warpdrive-url", "http://localhost:8000", "--token", "your-token"]
    }
  }
}
```

---

## Tools

| Category | Tools | Auth |
|----------|-------|------|
| **Read** | `warpdrive_get_node_info`, `warpdrive_get_health`, `warpdrive_list_services`, `warpdrive_get_service` | None |
| **Write** | `warpdrive_deploy_service`, `warpdrive_delete_service` | `--token` |
| **Dev** | `warpdrive_upload_component`, `warpdrive_save_service`, `warpdrive_simulate_trigger`, `warpdrive_deploy_dev_service`, `warpdrive_query_kv` | Dev endpoints enabled |
| **Chain-write** | `warpdrive_deploy_service_manager`, `warpdrive_deploy_poa_service_manager`, `warpdrive_register_operator`, `warpdrive_set_service_uri` | `mcp_chain_credential` |
| **Local** | `warpdrive_get_wit_interface`, `warpdrive_scaffold_component`, `warpdrive_build_component`, `warpdrive_get_service_schema` | None |

Local tools run entirely on the client machine — no running WarpDrive node needed.

---

## Full Documentation

See [`MCP.md`](../../MCP.md) in the repo root for complete tool reference, parameter shapes, and end-to-end workflow examples.
