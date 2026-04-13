# WarpDrive MCP Server

`warpdrive-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server that exposes WarpDrive platform operations to AI clients over stdio. It lets Claude Desktop, Cursor, VS Code, and other MCP-compatible clients query a live WarpDrive node, scaffold and build WASM components, upload binaries, deploy services, and simulate triggers — all from natural language.

---

## Security — Credential Storage

`mcp_chain_credential` (and `signing_mnemonic`) grant the ability to send on-chain transactions.
**Do not store them in a project-local `warpdrive.toml`** — that file may be inside a git repository
and risks accidental commit.

Recommended storage (in priority order):

1. **`~/.warpdrive/warpdrive.toml`** — user-level file outside any git repo. Set automatically by the WarpDrive
   app "Register with Claude" button and `just setup-claude-mcp`. Works with all MCP clients:
   ```toml
   [warpdrive]
   mcp_chain_credential = "0x..."
   signing_mnemonic = "word1 word2 ... word12"
   ```

2. **Env vars in MCP client config** — set in the `"env"` block for Claude Code, Cursor, VS Code.
   Use when you need per-project overrides or want to avoid even `~/.warpdrive/warpdrive.toml`:
   ```json
   "env": {
     "WARPDRIVE_MCP_CHAIN_CREDENTIAL": "your-key-or-mnemonic",
     "WARPDRIVE_SIGNING_MNEMONIC": "your mnemonic words"
   }
   ```

3. **CLI arg** — `--mcp-chain-credential`. Avoid (visible in `ps aux`).

`warpdrive.toml` in a project directory **will not** be read for credentials — `warpdrive-mcp` intentionally
skips project-local paths. Non-credential config (ports, chain endpoints, dev flags) is safe there.

---

## Quick Start

### 1. Build the binary

```bash
cargo build --release -p warpdrive-mcp
# Binary: ./target/release/warpdrive-mcp
```

### 2. Start a WarpDrive node (if you don't have one running)

```bash
just start-warpdrive-dev
```

### 3. Add to your client config (see below)

### 4. Test with MCP Inspector

```bash
npx @modelcontextprotocol/inspector ./target/release/warpdrive-mcp
```

---

## Client Configuration

### Claude Desktop (macOS)

Edit `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "warp-drive": {
      "command": "/path/to/WarpDrive/target/release/warpdrive-mcp",
      "args": ["--warpdrive-url", "http://localhost:8000", "--token", "your-token-here"]
    }
  }
}
```

Credentials (`mcp_chain_credential`, `signing_mnemonic`) are read automatically from `~/.warpdrive/warpdrive.toml`.

### Claude Desktop (Linux)

Edit `~/.config/Claude/claude_desktop_config.json` with the same structure above.

### Cursor

Edit `.cursor/mcp.json` in your project root (or `~/.cursor/mcp.json` globally):

```json
{
  "mcpServers": {
    "warp-drive": {
      "command": "/path/to/WarpDrive/target/release/warpdrive-mcp",
      "args": ["--warpdrive-url", "http://localhost:8000", "--token", "your-token-here"]
    }
  }
}
```

### VS Code (Copilot / MCP extension)

Edit `.vscode/mcp.json` in your workspace:

```json
{
  "servers": {
    "warp-drive": {
      "type": "stdio",
      "command": "/path/to/WarpDrive/target/release/warpdrive-mcp",
      "args": ["--warpdrive-url", "http://localhost:8000", "--token", "your-token-here"]
    }
  }
}
```

### CLI flags (alternative to env vars)

```bash
./target/release/warpdrive-mcp \
  --warpdrive-url http://localhost:8000 \
  --token your-bearer-token
```

---

## Tool Reference

Tools are grouped into five categories.

### Read tools — no auth required

| Tool | Description | Parameters |
|------|-------------|------------|
| `warpdrive_get_node_info` | Node info: service count, chain keys, aggregator config, P2P status | none |
| `warpdrive_get_health` | Health status of all configured chain RPC endpoints | none |
| `warpdrive_list_services` | All registered services with workflows, triggers, and components | none |
| `warpdrive_get_service` | Full config for one service | `chain` (e.g. `"evm:31337"`), `address` (contract address) |

### Write tools — require `--token`

| Tool | Description | Parameters |
|------|-------------|------------|
| `warpdrive_deploy_service` | Register a service from its ServiceManager | `service_manager_json` (see format below) |

`service_manager_json` shapes:
```json
// EVM
{"evm": {"chain": "evm:31337", "address": "0xAbCd..."}}

// Cosmos
{"cosmos": {"chain": "cosmos:mychain", "address": "cosmos1..."}}
```

### Chain-write tools — require `WARPDRIVE_MCP_CHAIN_CREDENTIAL`

> **Note:** These tools send real on-chain transactions. They require `WARPDRIVE_MCP_CHAIN_CREDENTIAL`
> to be set in the MCP client's `"env"` block (or via `--mcp-chain-credential` arg).
> EVM only currently.

| Tool | Description | Parameters |
|------|-------------|------------|
| `warpdrive_set_service_uri` | Call `setServiceURI` on the ServiceManager contract to update the on-chain service URI | `service_manager_json`, `uri`, `rpc_url` |
| `warpdrive_deploy_service_manager` | Deploy a new `SimpleServiceManager` PoA contract on-chain; returns `address` and `tx_hash` | `rpc_url` |
| `warpdrive_deploy_poa_service_manager` | Deploy a full `POAStakeRegistry` (upgradeable proxy) via Docker; returns proxy `address` (use as service manager). Requires Docker with `ghcr.io/lay3rlabs/poa-middleware:1.0.1`. | `rpc_url` |
| `warpdrive_register_operator` | Register the WarpDrive node's signing key as an vector on a `POAStakeRegistry` and set the signing key mapping. Calls `registerOperator` (owner) + `updateOperatorSigningKey` (vector). Requires `WARPDRIVE_MCP_CHAIN_CREDENTIAL` + `WARPDRIVE_SIGNING_MNEMONIC`. | `service_manager_json`, `rpc_url`, `weight` (optional, default 100) |

To enable, set in `~/.warpdrive/warpdrive.toml` under `[warpdrive]` (recommended — works with all MCP clients):
```toml
mcp_chain_credential = "0x<private_key_or_mnemonic>"
signing_mnemonic = "<mnemonic>"
```

Or set env vars in your MCP client's `"env"` block (per-client override):
```json
"env": {
  "WARPDRIVE_MCP_CHAIN_CREDENTIAL": "0x<private_key_or_mnemonic>",
  "WARPDRIVE_SIGNING_MNEMONIC": "<mnemonic words>"
}
```

### Dev tools — require dev endpoints enabled in `warpdrive.toml`

> **Note:** Dev tools require `dev_endpoints_enabled = true` under `[warpdrive]` in `warpdrive.toml`. Restart the WarpDrive node after changing this.

| Tool | Description | Parameters |
|------|-------------|------------|
| `warpdrive_upload_component` | Upload a compiled `.wasm` binary; returns its digest | `file_path` (absolute path to `.wasm`) |
| `warpdrive_simulate_trigger` | Fire a trigger against a deployed service | `service_id`, `workflow_id`, `trigger_json`, `data_json`, `count` (optional) |
| `warpdrive_deploy_dev_service` | Register a service directly without an on-chain contract | `service_json` (full Service definition as JSON string) |
| `warpdrive_query_kv` | Read a value from a service's KV store | `service_id`, `bucket`, `key` |

`warpdrive_simulate_trigger` parameter shapes:

```json
// trigger_json examples
{"manual": null}
{"cron": {"schedule": "* * * * *", "start_time": null, "end_time": null}}
{"evm_contract_event": {"chain": "evm:31337", "address": "0x...", "event_hash": "0x<32-byte-keccak>"}}

// data_json examples
{"Raw": [104, 101, 108, 108, 111]}
{"Cron": {"trigger_time": 0}}
{"EvmContractEvent": {"log": {...}}}
```

### Local tools — run entirely on the client machine

| Tool | Description | Parameters |
|------|-------------|------------|
| `warpdrive_get_wit_interface` | Returns the full WIT interface definitions (HTTP, KV, TLS, host functions, etc.) | none |
| `warpdrive_scaffold_component` | Generate a ready-to-build component scaffold (Cargo.toml + lib.rs) | `name`, `trigger_type`, `description` (optional) |
| `warpdrive_build_component` | Build a component with `cargo component build`; returns full stdout/stderr | `dir` (path to component), `release` (default: true) |

`warpdrive_scaffold_component` trigger types: `evm_contract_event` | `cosmos_contract_event` | `block_interval` | `cron` | `manual`

---

## Example Prompts

Paste any of these directly into Claude Desktop (or any MCP-connected client) after configuring the server.

**Inspect your node:**
> "What services are currently running on my WarpDrive node?"

> "Show me the health of my chain RPC endpoints."

> "Get the WIT interface so I know what APIs are available in WASM components."

**Scaffold and build:**
> "Scaffold a WASM component called `price-feed` that processes EVM contract events."

> "Build the component at `examples/components/price-feed`."

**Upload and deploy:**
> "Upload the wasm at `./target/wasm32-wasip2/release/price_feed.wasm`."

> "Deploy the service at address `0xABCD1234...` on chain `evm:31337`."

**Simulate:**
> "Simulate a raw trigger with data bytes `[104, 101, 108, 108, 111]` against service `abc123...` workflow `default`."

**Lifecycle:**
> "Pause the service at `0xABCD...` on `evm:31337` by updating its status to paused and setting the new URI on-chain."

---

## End-to-End Workflow

This sequence takes you from a blank slate to a running on-chain service.

```
1.  Start node          just start-warpdrive-dev
2.  Get WIT interface   warpdrive_get_wit_interface
                        → understand available APIs before writing code
3.  Scaffold            warpdrive_scaffold_component {name, trigger_type}
                        → writes Cargo.toml + src/lib.rs skeleton
4.  Implement           Edit src/lib.rs — decode trigger, compute, encode output
5.  Build               warpdrive_build_component {dir}
                        → cargo component build --release; read errors, fix, repeat
6.  Upload              warpdrive_upload_component {file_path}
                        → returns component digest (sha256:...)
7.  Deploy              warpdrive_deploy_service {service_manager_json}
                        → registers the service; WarpDrive reads config from chain
8.  Simulate            warpdrive_simulate_trigger {service_id, workflow_id, trigger_json, data_json}
                        → fires a test trigger; check logs for output
9.  Verify              warpdrive_list_services / warpdrive_get_node_info
                        → confirm the service is running
```

---

## System Prompt for Agentic Mode

Paste this as a system prompt to put an AI assistant into WarpDrive developer mode:

```
You are a WarpDrive (WebAssembly-based Actively Validated Services) developer assistant.
You have access to the following MCP tools via the wavs server:

Read (no auth): warpdrive_get_node_info, warpdrive_get_health, warpdrive_list_services, warpdrive_get_service
Write (require token): warpdrive_deploy_service, warpdrive_delete_service
Chain-write (require WARPDRIVE_MCP_CHAIN_CREDENTIAL): warpdrive_set_service_uri, warpdrive_deploy_service_manager
Dev (require dev endpoints): warpdrive_upload_component, warpdrive_simulate_trigger, warpdrive_deploy_dev_service, warpdrive_query_kv
Local: warpdrive_get_wit_interface, warpdrive_scaffold_component, warpdrive_build_component

When the user asks you to create a component, follow this workflow:
1. Call warpdrive_get_wit_interface to understand available APIs.
2. Call warpdrive_scaffold_component to generate the project skeleton.
3. Help the user implement the logic in src/lib.rs.
4. Call warpdrive_build_component — read the output, fix any errors, rebuild.
5. Call warpdrive_upload_component with the compiled .wasm path.
6. Call warpdrive_deploy_service with the ServiceManager JSON.
7. Call warpdrive_simulate_trigger to test the service.
8. Call warpdrive_list_services to confirm it's running.

Components are Rust libraries compiled to wasm32-wasip2. They implement the `Guest` trait,
decode trigger data with `decode_trigger_event`, process it, and return encoded responses
with `encode_trigger_output`. State is stored via wasi::keyvalue; outbound HTTP uses wasi::http.
The authoritative WIT interface definitions live in the wavs-wasi repo; use `warpdrive_get_wit_interface`
to retrieve the local copy bundled with this node.
```
