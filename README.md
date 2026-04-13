# WarpDrive

![Banner](docs/images/wavs.png)

[![Project Status: Active -- The project has reached a stable, usable state and is being actively developed.](https://img.shields.io/badge/repo%20status-Active-green.svg?style=flat-square)](https://www.repostatus.org/#active)

**WarpDrive delivers enterprise-grade, verifiable off-chain compute to the Stellar ecosystem.**

WarpDrive is a trusted compute layer that turns arbitrary off-chain data and processes into provably correct on-chain actions. It leverages Soroban's smart-contract capabilities, modern WASI execution, and flexible operator governance to create a scalable, composable platform for real-world financial and compliance use cases.

Run bots, oracles, automation, and more on Stellar with EigenLayer-grade security.

## What WarpDrive Does

Smart contracts can't reach beyond the ledger — they can't fetch external data, react to real-world events, or coordinate complex multi-step processes without trusting a single operator. WarpDrive removes that constraint by providing cryptographically verifiable off-chain compute that settles back to Stellar.

**Core building blocks:**

- **Projects** — independent, sandboxed deployments, each with its own governance and trust model. Projects share no state and interact only through Soroban contract calls.
- **Project Specification Repository** — immutable, content-addressable (IPFS) spec describing all circuits, WASI payloads, and policy modules. The on-chain Project Root stores a single hash; upgrades happen by pushing a new spec and bumping that hash.
- **Verifiable Computers (Vectrs)** — decentralized execution nodes that pull the spec, run circuits inside sandboxed Wasmtime WASI runtimes, and emit signed attestations. Signing keys live outside the sandbox — WASI code cannot forge signatures.
- **Circuits** — a full unit of off-chain work: an input (on-chain event, cron tick, web2 API), a transform (WASI component in Rust/Go/JS), and an output (Soroban verification contract, EVM chain, IPFS, or another circuit for multi-stage workflows).
- **Aggregator** — collects attestations from Vectrs and batches them into a single on-chain submission. Cannot forge signatures; multiple aggregators can run to remove censorship concerns.
- **Verification Module** — Soroban contract that validates attestation proofs and translates verified payloads into contract calls on the Stellar ledger.
- **Security Module** — defines who can attest and with what weight. Supports PoA (fixed trusted operator set), PoS / EigenLayer-style restaking, or any custom algorithm that maps public keys to weights.
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

## Release Workflow

This project uses [just](https://github.com/casey/just) for managing releases. Typical workflow:

1. **Make your changes** — develop circuits, fix bugs, update WIT definitions, etc.
2. **Set the version** — update version numbers across `Cargo.toml` and all WIT package definitions:
   ```bash
   just set-version v3.0.0
   ```
3. **Create PR and merge** — get your changes reviewed and merged to main.
4. **Push tags** — after merge, create and push git tags:
   ```bash
   just push-tag v3.0.0
   ```

This creates both a standard tag (`v3.0.0`) and a Go module tag (`wasi/go/v3.0.0`). The standard tag triggers CI to publish the `warpdrive-types` and `warpdrive-wasi-utils` crates to crates.io, WASM components to wa.dev, and TypeScript bindings to NPM.

---

## Claude Code Integration

WarpDrive ships with a `/warp-drive` skill for [Claude Code](https://claude.ai/code) that teaches Claude the full WarpDrive component development workflow — scaffolding, building, uploading, and deploying circuits via the MCP tools.

Full Claude Code integration requires two independent steps:

1. **Install the skill** — teaches Claude the WarpDrive workflow and tool reference.
2. **Register `warpdrive-mcp`** — connects Claude Code to a live WarpDrive node so MCP tools actually work.

### Step 1: Install the skill

**In-repo (automatic):** If you're working inside this repository, the `/warp-drive` skill is available automatically. No installation needed.

**Global (repo cloned):**
```bash
just install-claude-skill
```

**Global (no clone needed):**
```bash
bash <(curl -fsSL https://raw.githubusercontent.com/warp-driver/warpdrive/main/.claude/skills/warp-drive/install.sh)
```

After installation, restart Claude Code to pick up the skill.

### Step 2: Register warpdrive-mcp with Claude Code

The skill's MCP tools require `warpdrive-mcp` to be running and registered for each project. Run once per project directory:

```bash
# From the WarpDrive repo — auto-detects the running warpdrive-mcp process:
just setup-claude-mcp /path/to/your-project
```

This writes the `mcpServers.wavs` entry into `~/.claude.json` for that project. Restart Claude Code (or reload MCP servers) afterwards.

See [MCP.md](MCP.md) for full setup and configuration details.

---
For more guides, architecture details, and examples, see the [docs folder](docs/README.md).
