# Dead Weight

Candidates for removal. Nothing here has been deleted yet — this is a decision queue against the criteria: **"does it serve (1) full Soroban support or (2) maintained Ethereum compatibility?"** Everything else earns its place or gets dropped.

Organized by category. Each entry lists the scope, the reason it's suspected dead weight, and what to do next.

---

## 1. Cosmos / CosmWasm stack — large, actively in the hot path

**Scope (~32 files, ~3 workspace packages, 4 top-level deps)**

- Rust packages:
  - `examples/contracts/cosmwasm/mock/api/`
  - `examples/contracts/cosmwasm/mock/service-handler/`
  - `examples/contracts/cosmwasm/trigger/api/`
  - `examples/contracts/cosmwasm/trigger/simple/`
  - `examples/components/cosmos-query/`
- Subsystem code inside the core node:
  - `packages/warpdrive/src/subsystems/trigger/streams/cosmos_stream.rs`
  - `packages/warpdrive/src/subsystems/submission/` (Cosmos branches in `data.rs`, `error.rs`)
  - `packages/warpdrive/src/subsystems/trigger.rs` (Cosmos config paths)
  - `packages/engine/src/bindings/**/host.rs` (`CosmosChainConfig` bindings)
  - `packages/cli/src/clients.rs`, `packages/cli/src/command/service.rs` (Cosmos chain branches)
- Tests:
  - `packages/layer-tests/src/example_cosmos_client/` (entire dir)
  - `packages/layer-tests/src/e2e/handles/cosmos.rs`
  - Cosmos assertions across `packages/layer-tests/src/e2e/*`
  - `packages/utils/src/test_utils/middleware/cosmos.rs`
- Types:
  - `packages/types/src/contracts/cosmwasm/` (entire subtree)
  - Cosmos variants in `packages/types/src/chain_config.rs`, `service.rs`, `aggregator_types.rs`, `signing.rs`
  - `cosmwasm` feature in `warpdrive-types` (default-enabled)
- Workspace deps to drop:
  - `cosmwasm-schema`, `cosmwasm-std`, `cw-storage-plus`, `cw2`
  - `layer-climb`, `layer-climb-address`, `layer-climb-config`, `layer-climb-cli`, `tendermint-*` (transitive)
- Env vars: `WARPDRIVE_AGGREGATOR_COSMOS_CREDENTIAL`, `WARPDRIVE_COSMOS_SUBMISSION_MNEMONIC`, `WARPDRIVE_CLI_COSMOS_MNEMONIC`
- Triggers / submission chains marked `TriggerData::CosmosContractEvent`, `Submission::Cosmos`, etc.

**Why suspected dead weight**

Soroban is Stellar-native and is its own thing. Cosmos SDK / CosmWasm is a separate universe. There is no path from "keep Ethereum working + ship Soroban" that runs through keeping Cosmos support alive. Cosmos pulls in the entire `layer-climb` stack, `cosmwasm-*`, and `tendermint-*` — those alone represent hundreds of transitive dependencies and meaningful CI time.

**Friction cost**

- `cosmwasm` is the **default feature** on `warpdrive-types`, so every downstream crate pulls the Cosmos types whether it uses them or not.
- All trigger/submission enums carry Cosmos variants, forcing exhaustive-match churn whenever any handler changes.
- Test matrix in `layer-tests.toml` combines Cosmos × EVM × P2P modes; dropping Cosmos cuts the e2e runtime substantially.
- `rust-version = "1.86.0"` on `warpdrive-types` is pinned *only* because of a CosmWasm compat issue (see comment in `packages/types/Cargo.toml:7`).

**Next action (when approved)**

1. Delete the four `examples/contracts/cosmwasm/*` packages and the `cosmos-query` component.
2. Strip Cosmos variants from `trigger`, `submission`, `chain_config`, `service.rs`.
3. Delete `packages/types/src/contracts/cosmwasm/` and the `cosmwasm` Cargo feature.
4. Remove `cosmos_stream.rs`, `submission/data.rs` Cosmos arm, `handles/cosmos.rs`, `example_cosmos_client/`.
5. Drop `layer-climb*`, `cosmwasm-*`, `cw-*`, `tendermint-*` from workspace `Cargo.toml`.
6. Drop `WARPDRIVE_AGGREGATOR_COSMOS_CREDENTIAL` env, CLI arg, config field.

**Caveat**: Do not touch until confirmed. Some test infrastructure (`mock_submissions`, `mock_trigger_manager`) asserts symmetry between chain backends — may need to fall back to EVM-only mocks.

---

## 2. atproto / BlueSky jetstream — speculative, un-shipped

**Scope**

- `packages/warpdrive/src/subsystems/trigger/streams/atproto_jetstream.rs` (11KB, largest stream file)
- `TriggerData::AtprotoEvent` variant + `TriggerDataAtprotoEvent` in WIT types
- References in: trigger config, service config, args, test_registry, lookup

**Why suspected dead weight**

The product proposal describes BlueSky posts as an example use case, and there's a working proof-of-concept *elsewhere* (HyperCerts attestations). But inside this repo it's an input stream that nothing production actually uses. Neither Soroban support nor Ethereum compatibility depends on it.

**Friction cost**

- Every `TriggerData` match arm must handle atproto events.
- The jetstream subscriber opens a websocket to an external host on startup even when no service uses it — depends on config but easy to misuse.
- Largest single stream file in the codebase; dragging it through the Operators→Vectors rename touched multi-line imports.

**Next action**

Delete the stream file, the `TriggerData::AtprotoEvent` variant, and the `TriggerDataAtprotoEvent` WIT type. If we later need a social-media trigger, reintroduce it scoped to one concrete product feature.

---

## 3. WASI Go bindings — 112 generated files, no consumers in-repo

**Scope**

- `wasi/go/` directory (~112 files)
- `go.mod` / `go.sum` that claim a Go module `github.com/warp-driver/warpdrive/wasi/go`
- README suggesting Vectrs in Go

**Why suspected dead weight**

These are auto-generated `wit-bindgen-go` outputs re-exposing the `wavs:operator` WIT world for Go component authors. None of the in-repo examples use them — every example component is Rust. The README tells you to `wit-bindgen-go generate` from the upstream `wavs-wasi` registry anyway, so these files are a snapshot that will drift from the real source of truth. The full rename pass had to skip `wasi/` entirely because the generated Go imports reference the external `Lay3rLabs/wavs-wasi` module path.

**Friction cost**

- 112 files out of the rename audit. Every future "grep for wavs" lands here first.
- The `/wavs/` path in import strings contradicts the repo's own rename.
- CI builds nothing in this tree — it's shipped content only.

**Next action**

Delete `wasi/go/` entirely. Point component authors at the upstream `wavs-wasi` repo for Go bindings. Keep Rust as the first-class component language.

---

## 4. POA middleware alongside EigenLayer middleware — pick one

**Scope**

- `packages/utils/src/test_utils/middleware/evm/middleware_poa.rs`
- `packages/utils/src/test_utils/middleware/evm/middleware_eigen.rs`
- Separate Docker images: `ghcr.io/lay3rlabs/wavs-middleware` (Eigenlayer) and `ghcr.io/lay3rlabs/poa-middleware`
- Tauri app exposes both `deploy_service_manager` (PoA Simple) and `deploy_poa_service_manager` (PoA full registry)
- MCP tools: `warpdrive_deploy_service_manager` + `warpdrive_deploy_poa_service_manager`

**Why suspected dead weight — partial**

We have at least three middleware flavors wired in:
1. `SimpleServiceManager` (lightweight PoA) — self-contained, just an operator whitelist
2. `POAStakeRegistry` (full PoA middleware) — external wavs-middleware Docker image
3. EigenLayer / restaking — external wavs-middleware Docker image

The Stellar proposal collapses these into "Security Module" pluggability (PoA → static list, PoS → EigenLayer-style restaking). For Ethereum compat we probably want exactly one of the PoA flavors — not both — and EigenLayer. Keeping three middleware frontends multiplies deployment tests.

**Next action**

Pick one of the two PoA flavors. The simpler `SimpleServiceManager` matches the proposal's "small whitelist" PoA; the full POAStakeRegistry matches "dynamic operator set with slashing" which overlaps with EigenLayer. Drop whichever we don't champion.

---

## 5. Example components that exist as regression test fodder

**Scope**

- `examples/components/echo-data/`
- `examples/components/echo-block-interval/`
- `examples/components/echo-cron-interval/`
- `examples/components/square/`
- `examples/components/permissions/`
- `examples/components/kv-store/`
- `examples/components/chain-trigger-lookup/`
- `examples/components/simple-aggregator/` (with `gas_oracle.rs` — Ethereum-specific)
- `examples/components/timer-aggregator/`
- `examples/components/cosmos-query/`

**Why suspected dead weight — partial**

Multiple components are variations on the same "echo X" template that exist only so `layer-tests` can exercise a trigger kind. A realistic sample set for Stellar devs is probably 2–3 components (one Stellar event → submit, one cron → submit, one composition workflow). The current 10-component zoo is mostly historical.

**Next action**

Keep: 1 chain-event component (rewritten for Soroban), 1 cron component, 1 aggregator/composition component. Delete: `echo-block-interval`, `echo-cron-interval`, `echo-data` (once tests migrate), `square`, `cosmos-query` (covered by the Cosmos removal). Reassess `permissions`, `kv-store`, `chain-trigger-lookup`, `simple-aggregator/gas_oracle.rs` — each is a distinct feature demo; decide per-component whether the feature it demos is a Stellar-first capability.

---

## 6. `examples/components/simple-aggregator/src/gas_oracle.rs`

**Scope**: one Rust file inside the simple-aggregator component.

**Why**: Ethereum L1 gas-price heuristic. Useful for Ethereum compat, not for Stellar (fees work differently on Soroban). Either move to an `eth_*` namespace or drop.

---

## 7. `packages/warpdrive-mcp` "wavs_env_*" passthrough env vars

**Scope**: ~30 env var names the MCP server whitelists and forwards into WASI components — `WARPDRIVE_ENV_ANTHROPIC_API_KEY`, `WARPDRIVE_ENV_OPENAI_API_KEY`, `WARPDRIVE_ENV_GROQ_API_KEY`, `WARPDRIVE_ENV_HUGGINGFACE_API_KEY`, `WARPDRIVE_ENV_MISTRAL_API_KEY`, `WARPDRIVE_ENV_REPLICATE_API_KEY`, `WARPDRIVE_ENV_TOGETHER_API_KEY`, `WARPDRIVE_ENV_OLLAMA_BASE_URL`, `WARPDRIVE_ENV_LM_STUDIO_BASE_URL`, plus `WARPDRIVE_ENV_COINGECKO_API_KEY`, `WARPDRIVE_ENV_INFURA_API_KEY`, `WARPDRIVE_ENV_ALCHEMY_API_KEY`, `WARPDRIVE_ENV_ETHERSCAN_API_KEY`, `WARPDRIVE_ENV_THEGRAPH_API_KEY`, `WARPDRIVE_ENV_PINATA_JWT`, `WARPDRIVE_ENV_GITHUB_TOKEN`, …

**Why**: A whitelist of provider-specific API keys that gets hardcoded into node configuration. The node should not need to know the name of every possible LLM provider. Replace with a generic passthrough (`WARPDRIVE_ENV_<ANYTHING>` auto-forwarded) or require explicit per-service config.

**Next action**: Replace the hardcoded list with a prefix-based rule. Ten-line change; removes a permanent nuisance.

---

## 8. `packages/wavs/tests/wavs_systems/` (now `warpdrive/tests/warpdrive_systems/`)

**Scope**: a parallel mock node (MockApp, MockAggregator, MockSubmissions, MockTriggerManager, MockService, MockConfig) used by a handful of node-internal tests.

**Why suspected dead weight — not sure yet**

These mocks re-implement subsystem traits to let tests exercise node wiring without spinning up a real node. Either:
- they're load-bearing and should stay (keep), or
- they've been superseded by `layer-tests/` (which uses real nodes over real chains) and should go.

The rename touched 7 files in this directory. Worth auditing for actual coverage contribution: if the same paths are already hit by `layer-tests`, delete the mocks.

**Next action**: Coverage diff between `cargo test -p warpdrive` and `cargo test -p layer-tests`. Keep only what adds unique coverage.

---

## 9. `lookupchain` / trigger-lookup protocol — EVM-only extra hop

**Scope**: `examples/components/chain-trigger-lookup/` + `packages/warpdrive/src/subsystems/trigger/lookup.rs`

**Why**: A helper component that runs "go look up this trigger on chain before firing". Useful when triggers carry only an ID and the full event has to be refetched. Keeps EVM compat if kept; not needed for Stellar which emits event payloads directly. Decide: is this a reusable primitive or EVM-only patch? If EVM-only, namespace it; don't scatter `lookup` variants through every subsystem.

---

## 10. Duplicate contract-ABI mirrors

**Scope**: `packages/warpdrive/tests/contracts/solidity/abi/` and `examples/contracts/solidity/abi/` both hold JSON ABIs built from the middleware sources. Kept in sync by `just contracts-build` / `just download-solidity`.

**Why**: Two copies of the same generated artifacts. Either tests or examples (not both) should own the canonical copy; the other side `include!`s it. Low urgency but classic drift risk.

---

## Summary table

| Item | Lines | Deps | Risk of keep | Action |
|---|---|---|---|---|
| Cosmos / CosmWasm | ~32 files, many variants | 8 deps | HIGH — default feature, everywhere | Drop post-Stellar-MVP |
| atproto jetstream | 1 file + variant | 0 deps | MED — runtime overhead | Drop |
| `wasi/go/` | 112 files | 0 deps | LOW — shipped-only | Drop (refer users to upstream wavs-wasi) |
| PoA vs Eigen middleware | 2 files + docker img | 0 deps | LOW — pick one | Consolidate |
| Extra echo components | 5–6 dirs | 0 deps | LOW | Trim to 3 canonical |
| `gas_oracle.rs` | 1 file | — | LOW | Move / drop |
| MCP env-var whitelist | — | — | LOW | Replace with prefix rule |
| `warpdrive_systems/` mocks | 7 files | — | MED — test coverage overlap unknown | Audit coverage first |
| Trigger `lookup` | 1 file + variant | — | LOW | Namespace / demote |
| Duplicate ABI mirrors | 2 dirs | — | LOW | Deduplicate |

Nothing here has been removed. Review and approve entries individually before cutting.
