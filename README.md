# WarpDrive

![Banner](docs/images/warpdrive.png)

[![Project Status: Active -- The project has reached a stable, usable state and is being actively developed.](https://img.shields.io/badge/repo%20status-Active-green.svg?style=flat-square)](https://www.repostatus.org/#active)

**WarpDrive delivers enterprise-grade, verifiable off-chain compute to the Stellar ecosystem.**

WarpDrive is a trusted compute layer that turns arbitrary off-chain data and processes into provably correct on-chain actions. It leverages Soroban's smart-contract capabilities, modern WASI execution, and flexible vector governance to create a scalable, composable platform for real-world financial and compliance use cases.

Run bots, oracles, automation, and more on Stellar with EigenLayer-grade security.

## What WarpDrive Does

Smart contracts can't reach beyond the ledger — they can't fetch external data, react to real-world events, or coordinate complex multi-step processes without trusting a single vector. WarpDrive removes that constraint by providing cryptographically verifiable off-chain compute that settles back to Stellar.

**Core building blocks:**

- **Projects** — independent, sandboxed deployments, each with its own governance and trust model. Projects share no state and interact only through Soroban contract calls.
- **Project Specification Repository** — immutable, content-addressable (IPFS) spec describing all circuits, WASI payloads, and policy modules. The on-chain Project Root stores a single hash; upgrades happen by pushing a new spec and bumping that hash.
- **Verifiable Computers (Vectrs)** — decentralized execution nodes that pull the spec, run circuits inside sandboxed Wasmtime WASI runtimes, and emit signed attestations. Signing keys live outside the sandbox — WASI code cannot forge signatures.
- **Circuits** — a full unit of off-chain work: an input (on-chain event, cron tick, web2 API), a transform (WASI component in Rust/Go/JS), and an output (Soroban verification contract, EVM chain, IPFS, or another circuit for multi-stage workflows).
- **Aggregator** — collects attestations from Vectrs and batches them into a single on-chain submission. Cannot forge signatures; multiple aggregators can run to remove censorship concerns.
- **Verification Module** — Soroban contract that validates attestation proofs and translates verified payloads into contract calls on the Stellar ledger.
- **Security Module** — defines who can attest and with what weight. Supports PoA (fixed trusted vector set), PoS / EigenLayer-style restaking, or any custom algorithm that maps public keys to weights.
- **Composable Workflows** — chain circuits together off-chain without touching the blockchain between stages. Multi-stage computations keep the same security guarantees as simple circuits; only the final result settles on-chain.

## Use Cases

- **DeFi automation** — off-chain limit orders, protocol treasury rebalancing, and cross-protocol integrations (e.g. Phoenix × Blend yield optimization on XLM-USDC).
- **Flexible oracles** — time-weighted average prices with multi-round composition (commit/reveal-style), outlier punishment, and web2 data sources.
- **User onboarding** — control on-chain actions from Telegram, email (with DKIM verification), BlueSky posts, or OAuth, without requiring users to install a wallet.

For subsystem-level design, see [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

---

## Key Features

- Write circuits in Rust (more languages coming soon)
- Compile to WebAssembly (WASM) for portability and near-native speed
- Deploy as lightweight, hot-swappable WASI components
- Trigger execution from Soroban / Stellar events, EVM events, cron, or web2 APIs
- Run off-chain at native speed in the WarpDrive WASI runtime (~400µs startup overhead; benchmarked at >10,000 events/sec)
- Bring results verifiably on-chain via threshold-signed attestations
- Dynamically manage multiple circuits for flexible, composable applications

---

## Development

### Prerequisites

- **Rust** (toolchain pinned in `rust-toolchain.toml`) — install via [rustup](https://rustup.rs/)
- **Docker** — required for WASI component builds, CosmWasm builds, and telemetry services
- **Node.js 20+** and **pnpm** — required for the desktop app (`task app-*` recipes)
- **Foundry** (`forge`, `cast`, `anvil`) — required for Solidity builds and the local EVM testnet. Install: `curl -L https://foundry.paradigm.xyz | bash && foundryup`
- **Go 1.23+** — only needed if you're working on the Go WASI bindings under `wasi/go/`

### Install Task

This project uses [Task](https://taskfile.dev) to run builds, tests, and release workflows:

```bash
# macOS
brew install go-task

# Linux (via install script)
sh -c "$(curl --location https://taskfile.dev/install.sh)" -- -d -b /usr/local/bin

# Or via Go
go install github.com/go-task/task/v3/cmd/task@latest
```

### Install wkg and wasm-tools

`wkg` is the WASM package manager (used for WIT fetch / build / publish). `wasm-tools` is used by the WIT build step to strip custom sections:

```bash
cargo install wkg wasm-tools

# Point wkg at the default registry (one-time)
wkg config --default-registry wa.dev
```

### Fetch WIT dependencies

WIT packages under `wit-definitions/` pull standard WASI interfaces from `wa.dev`. Cross-package references within the repo (`warpdrive:types`, `warpdrive:vectr`) are resolved locally via `wkg.toml` overrides.

```bash
task wit-deps-fetch
```

### Environment

Copy `.env.example` to `.env` and fill in credentials:

```bash
cp .env.example .env
```

### Common commands

List all available tasks:

```bash
task --list
```

```bash
task lint                    # Check formatting and clippy
task lint-fix                # Auto-fix formatting and clippy issues
task wasi-build              # Build all WASI components in Docker
task solidity-build          # Compile Solidity contracts and copy ABIs
task cosmwasm-build          # Compile CosmWasm example contracts
task wit-build               # Build WIT packages
task test-warpdrive-e2e      # Run on-chain integration tests
task ts-bindings             # Generate TypeScript bindings
task start-dev               # Start warpdrive with telemetry (Jaeger + Prometheus)
task start-warpdrive-dev     # Run warpdrive in dev mode
task start-anvil             # Start local EVM testnet
task app-dev                 # Start Tauri desktop app with hot reload
```

See [`Taskfile.yml`](Taskfile.yml) for the full list.

---

## Release Workflow

Typical release flow:

1. **Make your changes** — develop circuits, fix bugs, update WIT definitions, etc.
2. **Set the version** — update version numbers across `Cargo.toml` and all WIT package definitions:
   ```bash
   task set-version VERSION=v2.7.0
   ```
3. **Create PR and merge** — get your changes reviewed and merged to main.
4. **Push tags** — after merge, create and push git tags:
   ```bash
   task push-tag VERSION=v2.7.0
   ```

This creates both a standard tag (`v2.7.0`) and a Go module tag (`wasi/go/v2.7.0`). The standard tag triggers CI to publish the `warpdrive-types` and `warpdrive-wasi-utils` crates to crates.io, WASM components to wa.dev, and TypeScript bindings to NPM.

---
For more guides, architecture details, and examples, see the [docs folder](docs/README.md).
