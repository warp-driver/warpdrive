---
name: warp-drive
description: >
  Develops, deploys, and manages WarpDrive (WebAssembly-based verifiable off-chain
  compute for Stellar/Soroban) components and services using the wavs MCP server.
  Use when the user wants to build a WASM component, deploy a new service, update
  an existing service, or manage service lifecycle. Triggers on: warp-drive, wavs,
  wasm component, Circuit, service manager, deploy service, scaffold component, Vectr,
  circuit.
---

# WarpDrive Developer Guide

WarpDrive runs off-chain computation as sandboxed WebAssembly (WASI) components triggered by on-chain events (EVM/Cosmos contract events, cron, block intervals, or manual inputs); execution results are submitted back on-chain through a ServiceManager contract.

---

## MCP Setup

The `wavs:` MCP tools below require `warpdrive-mcp` to be running and registered with Claude Code.

**One-command setup (recommended)**
```bash
npx @warpdrive/mcp@latest
```
Interactive wizard: installs the binary, prompts for URL + token + credentials,
writes `~/.claude.json` and `~/.warpdrive/warpdrive.toml`, installs skill files.

**Using the WarpDrive desktop app**
The app auto-starts `warpdrive-mcp`. Use the "Register with Claude Code" button in
Settings → MCP Server to register for any project path without leaving the app.

**WarpDrive repo users**
```bash
just setup-claude-mcp [/path/to/project]
```

**CLI / manual setup**
```bash
# 1. Start a WarpDrive node
just start-warpdrive-dev

# 2. Run warpdrive-mcp (in a separate terminal)
./target/release/warpdrive-mcp --warpdrive-url http://localhost:8000 --token <token>

# 3. Register with Claude Code
npx @warpdrive/mcp@latest
```

> **Local tools** (`scaffold_component`, `build_component`, `get_wit_interface`) work without MCP — useful for component development without a running node.

---

## Choose Your Flow

| User Intent | Follow |
|-------------|--------|
| Build a new component from scratch | [`flows/component-dev.md`](flows/component-dev.md) |
| Deploy a new service with an on-chain contract | [`flows/deployment.md`](flows/deployment.md) |
| Update a deployed service with a new component | [`flows/update-service.md`](flows/update-service.md) |

When in doubt, start with **component-dev** — it ends with a deployment step.

---

## MCP Tool Categories

| Category | Tools | Auth Required |
|----------|-------|---------------|
| **Read** | `get_node_info`, `get_health`, `list_services`, `get_service` | None |
| **Write** | `deploy_service`, `delete_service` | `--token` |
| **Dev** | `upload_component`, `save_service`, `simulate_trigger`, `deploy_dev_service`, `query_kv` | Dev endpoints enabled |
| **Chain-write** | `set_service_uri`, `deploy_service_manager`, `deploy_poa_service_manager`, `register_operator` | `WARPDRIVE_MCP_CHAIN_CREDENTIAL` env var |
| **Local** | `get_wit_interface`, `scaffold_component`, `build_component` | None |

Full tool reference: [`reference/mcp-tools.md`](reference/mcp-tools.md)

---

## Key Configuration

For on-chain operations, credentials are read from `~/.warpdrive/warpdrive.toml` (the WarpDrive home config):

```toml
[warpdrive]
mcp_chain_credential = "0x<private-key>"
signing_mnemonic = "word1 word2 ... word12"
```

The WarpDrive app "Register with Claude" button and `just setup-claude-mcp` write this file automatically.

Dev endpoints must be enabled in `warpdrive.toml` under `[warpdrive]`:
```toml
dev_endpoints_enabled = true   # Required for upload, save, simulate, deploy_dev
```

---

## Reference

- [`reference/mcp-tools.md`](reference/mcp-tools.md) — All 20 tools with auth requirements and parameter notes
- [`reference/service-json.md`](reference/service-json.md) — Service/trigger JSON formats + simulate examples
