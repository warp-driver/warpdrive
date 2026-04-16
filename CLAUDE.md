# CLAUDE.md

For context on this codebase, read the `docs/` directory and the `Taskfile.yml`.

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What Is WarpDrive

WarpDrive (WebAssembly-based Actively Validated Services) is a platform for running Actively Validated Services (Circuit). It executes Circuit logic as sandboxed WebAssembly (WASI) components, bridges blockchain events (EVM and Cosmos) with off-chain computation, and coordinates multi-vector consensus.

## Quick Check

Run the following before each commit to ensure that the repo works.

```bash
task wasi-build
cargo check --all-targets --all-features
cargo build
cargo test
```

## Build, Lint, and Test Commands

All build automation is in `Taskfile.yml`. Run `task --list` to see all targets.

### Rust

```bash
task lint           # Check formatting and clippy (non-mutating)
task lint-fix       # Auto-fix formatting and clippy issues
cargo build         # Debug build
cargo build --release
```

### WASI Components (WebAssembly)

```bash
task wasi-build                    # Build all WASI components in Docker
task wasi-build COMPONENT=echo-data # Build a single component
task generate-checksums            # Regenerate checksums.txt
```

### Smart Contracts

```bash
task solidity-build    # Forge build for Solidity contracts
task cosmwasm-build    # Docker-based CosmWasm build
```

### Desktop App (Tauri + React)

```bash
task app-dev           # Full Tauri dev with hot reload
task app-dev-frontend  # Vite frontend dev server only
task app-build-release # Release build
task app-build-frontend # Vite build only
```

### Tests

E2E integration tests run on-chain with a live WarpDrive node:

```bash
task test-warpdrive-e2e
# or directly:
cargo test -p warpdrive-tests
```

To run a subset of tests, edit `packages/warpdrive-tests/warpdrive-tests.toml` to isolate specific cases.

### Running the Stack

```bash
task start-dev           # WarpDrive + Jaeger + Prometheus (full dev stack)
task start-warpdrive-dev # WarpDrive only with dev config
task start-anvil         # Local EVM testnet on :8545
task start-jaeger        # Tracing UI at http://localhost:16686
task start-prometheus    # Metrics UI at http://localhost:9090
```

Development tools for sending triggers and deploying services:
```bash
task dev-tool -- deploy-service --sleep-ms 10
task dev-tool -- send-triggers --count 1000
```

## Architecture

### Core Node (`packages/warpdrive/`)

The main WarpDrive node is a Tokio-based async server centered around a **dispatcher** (`packages/warpdrive/src/dispatcher.rs`) that orchestrates four subsystems via Crossbeam channels:

1. **Trigger Manager** (`subsystems/trigger/`) — Monitors EVM and Cosmos blockchain events; routes events to registered services via cron, timer, or on-chain triggers. Uses libp2p for P2P trigger distribution.

2. **Engine** (`subsystems/engine/`) — Executes WASM components in isolated Wasmtime WASI runtimes. Each Circuit service runs as a sandboxed component with restricted system access.

3. **Aggregator** (`subsystems/aggregator/`) — Collects execution results from multiple vectors and handles consensus before submission.

4. **Submission** (`subsystems/submission/`) — Routes verified results to on-chain contracts (EVM or Cosmos), managing signing and transaction submission.

An HTTP API server (Axum) on top handles service registration, health checks, and administration.

### Key Packages

- `packages/types/` — Shared types, WIT interfaces, contract ABIs, and generated TypeScript bindings
- `packages/cli/` — CLI for deploying services, executing components, and EigenLayer integration
- `packages/engine/` — Wasmtime wrapper and WASI component lifecycle management
- `packages/aggregator/` — Standalone aggregation service
- `packages/warpdrive-tests/` — E2E test suite; config in `warpdrive-tests.toml`
- `packages/dev-tool/` — Dev utilities for local testing

### Desktop App (`app/`)

Tauri 2 desktop app with a React 19 + Vite 7 frontend. The Tauri backend in `app/src-tauri/` bridges to the WarpDrive node. State management uses Zustand; blockchain interaction uses Viem.

### Examples

- `examples/components/` — WASI component source code (echo, kv-store, aggregator, cosmos-query, etc.)
- `examples/contracts/` — Example Solidity and CosmWasm contracts
- `examples/build/components/` — Compiled WASM output; `checksums.txt` tracks SHA256 hashes

## Environment

Copy `.env.example` to `.env`. Key variables:

```
RUST_LOG="info,warpdrive=debug"
WARPDRIVE_SIGNING_MNEMONIC="..."
WARPDRIVE_AGGREGATOR_EVM_CREDENTIAL="..."
WARPDRIVE_AGGREGATOR_COSMOS_CREDENTIAL="..."
```

## Documentation

Detailed docs live in `docs/`:
- `ARCHITECTURE.md` — Subsystem design details
- `LOCAL_DEV.md` — Development workflow and telemetry
- `API.md` — HTTP API reference
- `ASYNC_NOTES.md` — Async design patterns used throughout
- `P2P.md` — libp2p/Hyperswarm networking
- `WIT_AUTHORING_NOTES.md` — Writing WIT component interfaces
