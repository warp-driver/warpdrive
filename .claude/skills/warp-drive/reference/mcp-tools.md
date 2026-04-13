# MCP Tools Reference

All tools are exposed by the `wavs` MCP server. Prefix with `wavs:` when calling (e.g. `warp-drive:warpdrive_get_node_info`).

---

## Auth Requirements

| Tool | Token (`--token`) | Chain Cred | Dev Endpoints | Notes |
|------|:-----------------:|:----------:|:-------------:|-------|
| `warpdrive_get_node_info` | — | — | — | Service count, chain keys, aggregator config, P2P status |
| `warpdrive_get_health` | — | — | — | Health of all configured chain RPC endpoints |
| `warpdrive_list_services` | — | — | — | All registered services with workflows, triggers, components |
| `warpdrive_get_service` | — | — | — | Requires `chain` (e.g. `"evm:31337"`) + `address` params |
| `warpdrive_deploy_service` | ✓ | — | — | Reads service def from chain via ServiceManager |
| `warpdrive_delete_service` | ✓ | — | — | Permanently removes service from WarpDrive node |
| `warpdrive_set_service_uri` | — | ✓ | — | EVM only; calls `setServiceURI` on ServiceManager contract |
| `warpdrive_deploy_service_manager` | — | ✓ | — | EVM only; deploys `SimpleServiceManager.sol`; returns `address` |
| `warpdrive_deploy_poa_service_manager` | — | ✓ | — | EVM only; deploys `POAStakeRegistry` proxy via Docker; returns proxy `address` |
| `warpdrive_register_operator` | — | ✓ + mnemonic | — | EVM only; registers node's signing key on POAStakeRegistry; call AFTER `warpdrive_deploy_service` |
| `wavs_deploy_and_register` | ✓ | ✓ + mnemonic | — | POA convenience: `warpdrive_deploy_service` + `warpdrive_register_operator` in one call |
| `wavs_get_service_signer` | ✓ | — | — | Returns HD index + EVM address the node uses to sign envelopes for a service |
| `wavs_get_signing_address` | — | — (mnemonic only) | — | Derives EVM address at any HD index from signing mnemonic (no network, default HD 0) |
| `warpdrive_upload_component` | — | — | ✓ | Uploads `.wasm` binary; returns raw 64-char hex digest (no `sha256:` prefix) |
| `warpdrive_save_service` | — | — | ✓ | Saves service def to node store; returns URI |
| `warpdrive_simulate_trigger` | — | — | ✓ | Fires a test trigger against a deployed service |
| `warpdrive_deploy_dev_service` | — | — | ✓ | Registers service directly without on-chain contract |
| `warpdrive_query_kv` | — | — | ✓ | Reads a value from a service's KV store |
| `warpdrive_get_wit_interface` | — | — | — | Returns full WIT interface definitions (local, no network) |
| `warpdrive_scaffold_component` | — | — | — | Generates Cargo.toml + src/lib.rs skeleton (local) |
| `warpdrive_build_component` | — | — | — | Runs `cargo component build`; returns build output (local) |

**Legend:**
- Token: MCP server must be started with `--token <value>`; pass token in requests
- Chain Cred: `WARPDRIVE_MCP_CHAIN_CREDENTIAL` env var must be set in the MCP client's `"env"` block (or `~/.warpdrive/warpdrive.toml` as fallback)
- Dev Endpoints: `dev_endpoints_enabled = true` must be set under `[warpdrive]` in `warpdrive.toml`

---

## Tool Parameter Details

### warpdrive_get_service
```
chain:   "evm:31337" or "cosmos:mychain"
address: "0xServiceManagerAddress..."
```

### warpdrive_deploy_service / warpdrive_delete_service
```
service_manager_json: see reference/service-json.md
```

### warpdrive_set_service_uri
```
service_manager_json: {"evm": {"chain": "evm:31337", "address": "0x..."}}
uri:                  URI returned by warpdrive_save_service
rpc_url:              RPC endpoint for the chain (e.g. "http://localhost:8545")
```

### warpdrive_deploy_service_manager / warpdrive_deploy_poa_service_manager
```
rpc_url: RPC endpoint for the chain (e.g. "http://localhost:8545")
```
Returns: contract address (use as `address` in service_manager_json)

### warpdrive_register_operator
**Call AFTER `warpdrive_deploy_service`** — queries the node for the service-specific signing key (HD index N) and registers it on-chain.
```
service_manager_json: {"evm": {"chain": "evm:31337", "address": "0x..."}}
weight:               optional uint64 (default: 100)
rpc_url:              RPC endpoint for the chain (e.g. "http://localhost:8545")
```

### wavs_deploy_and_register
POA convenience tool: equivalent to `warpdrive_deploy_service` + `warpdrive_register_operator`. The service URI must already be set on-chain.
```
service_manager_json: {"evm": {"chain": "evm:31337", "address": "0x..."}}
weight:               optional uint64 (default: 100)
rpc_url:              RPC endpoint for the chain (e.g. "http://localhost:8545")
```

### wavs_get_service_signer
Returns the HD index and EVM address the WarpDrive node uses to sign envelopes for a specific service. Essential for diagnosing POAStakeRegistry `InvalidSignature` errors.
```
service_manager_json: {"evm": {"chain": "evm:31337", "address": "0x..."}}
```
Returns: `HD index: N, EVM address: 0x...`

### wavs_get_signing_address
Derives the EVM address at a given HD index of the signing mnemonic without any network call. Useful for verifying which address will be registered.
```
hd_index: optional uint32 (default: 0)
```
Returns: `Signing address (HD index N): 0x...`

### warpdrive_upload_component
```
file_path: absolute path to .wasm file
```
Returns: `"Component uploaded.\nDigest: <64-char hex>"` — the digest is a raw hex string with **no** `sha256:` prefix. Use it directly in `component.source.digest`.

### warpdrive_save_service / warpdrive_deploy_dev_service
```
service_json: full service definition JSON string
```
See [`service-json.md`](service-json.md) for the full schema.

**`warpdrive_deploy_dev_service` vs `warpdrive_deploy_service`:**
- `warpdrive_deploy_dev_service`: Dev/testing only. Takes full Service JSON, no on-chain contract needed. Handles save+register in one call.
- `warpdrive_deploy_service`: Production. Requires an on-chain ServiceManager whose URI is already set (via `warpdrive_set_service_uri`). Takes only the contract address.

### warpdrive_simulate_trigger
```
service_id:   64-char hex string (from warpdrive_list_services)
workflow_id:  lowercase alphanumeric, 3–36 chars (e.g. "default")
trigger_json: trigger definition (see service-json.md)
data_json:    trigger data payload (see service-json.md)
count:        optional, how many times to fire (default: 1)
```

### warpdrive_query_kv
```
service_id: 64-char hex string
bucket:     KV bucket name (as passed to store::open in component)
key:        key within the bucket
```

### warpdrive_build_component
```
dir:     directory containing the component's Cargo.toml
release: optional bool (default: true)
```

### warpdrive_scaffold_component
```
name:         lowercase-with-hyphens component name
trigger_type: evm_contract_event | cosmos_contract_event | block_interval | cron | manual
description:  optional string
```

---

## MCP Server Configuration

The MCP server binary is `warpdrive-mcp`. Key CLI args:

| Arg | Description |
|-----|-------------|
| `--warpdrive-url <url>` | WarpDrive node HTTP API URL (e.g. `http://localhost:8000`) |
| `--token <token>` | Auth token (enables write tools) |

The WarpDrive node URL and token can also be found by inspecting the running `warpdrive-mcp` process:
```bash
ps aux | grep warpdrive-mcp
```

Environment variables:
- `WARPDRIVE_URL` — WarpDrive node URL
- `WARPDRIVE_TOKEN` — auth token
- `WARPDRIVE_MCP_CHAIN_CREDENTIAL` — credential for on-chain ops (falls back to `mcp_chain_credential` in `~/.warpdrive/warpdrive.toml`)
- `WARPDRIVE_SIGNING_MNEMONIC` — signing mnemonic (falls back to `signing_mnemonic` in `~/.warpdrive/warpdrive.toml`)
