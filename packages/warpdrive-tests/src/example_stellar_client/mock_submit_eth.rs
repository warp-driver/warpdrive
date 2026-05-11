use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use rand::RngCore;
use stellar_xdr::curr::{Limits, ReadXdr, ScVal, ScSymbol};
use utils::filesystem::workspace_path;

/// Path (relative to the workspace root) of the staged `mock_submit_eth`
/// WASM that `task stellar-build` produces. Both `test-warpdrive-e2e` and
/// `debug-warpdrive-e2e` depend on `stellar-build`, so this file is
/// guaranteed to exist when the e2e suite runs.
const STAGED_MOCK_SUBMIT_ETH_WASM: &str =
    "examples/build/contracts/stellar/warpdrive_stellar_mock_submit_eth.wasm";

const STELLAR_RPC_URL_TESTNET: &str = "https://soroban-testnet.stellar.org";
const STELLAR_NETWORK_PASSPHRASE_TESTNET: &str = "Test SDF Network ; September 2015";
const STELLAR_WALLET_ALIAS: &str = "warpdrive-e2e-testnet";

/// Per-test stellar mock submit contract client for the **secp256k1
/// (ethereum-handler) path**.
///
/// Wraps `examples/contracts/stellar/mock_submit_eth`, which mirrors the
/// real `EthereumHandler`: storage is keyed by 20-byte `event_id` (not by
/// `trigger_id` the way EVM `SimpleSubmit` does), and the contract emits
/// a `Verified` event with the event_id as a topic on every successful
/// `verify_eth`. Reading a test's payload back is therefore a two-step
/// flow:
///
///   1. Poll the contract's `Verified` events via Soroban RPC to discover
///      the `event_id` (we never see it before submission — it's derived
///      from `service_id + workflow_id + bincode(trigger_data)` by
///      WarpDrive's aggregator and we can't reconstruct it without the
///      canonical chain-derived `TriggerData`).
///   2. Call the contract's `payload(event_id)` view function to read the
///      stored bytes.
///
/// Each test deploys a fresh contract (random salt), so there's exactly
/// one `Verified` event per contract and the event-poll narrows to a
/// single match.
#[derive(Clone, Debug)]
pub struct SimpleStellarSubmitEthClient {
    config_dir: PathBuf,
}

impl SimpleStellarSubmitEthClient {
    pub fn new(_chain: warpdrive_types::ChainKey) -> Self {
        Self {
            // Reuse the same shared wallet config dir as the trigger client;
            // it manages the funded testnet identity that's used for both
            // build/deploy operations.
            config_dir: workspace_path().join(".stellar-e2e"),
        }
    }

    /// Deploy a fresh `mock_submit_eth` instance bound to
    /// `verification_contract` (the test stack's `secp256k1_verification`).
    /// `admin` is the strkey `G...` address that owns the deployed
    /// contract's upgrade / admin-transfer flow — for the e2e tests this
    /// is the same wallet doing the deploy.
    pub async fn deploy(&self, admin: &str, verification_contract: &str) -> Result<String> {
        self.ensure_wallet()?;

        let wasm_path = self.contract_artifact_path();
        if !wasm_path.exists() {
            bail!(
                "staged mock_submit_eth wasm not found at {}; run `task stellar-build` \
                 (or invoke the e2e suite via `task test-warpdrive-e2e` / \
                 `task debug-warpdrive-e2e`, which both depend on it)",
                wasm_path.display()
            );
        }
        let wasm_path = wasm_path
            .to_str()
            .ok_or_else(|| anyhow!("invalid wasm path: {}", wasm_path.display()))?;

        let salt = random_salt_hex();

        let output = self.run_stellar(&[
            "contract",
            "deploy",
            "--source-account",
            STELLAR_WALLET_ALIAS,
            "--wasm",
            wasm_path,
            "--rpc-url",
            STELLAR_RPC_URL_TESTNET,
            "--network-passphrase",
            STELLAR_NETWORK_PASSPHRASE_TESTNET,
            "--salt",
            &salt,
            "--",
            "--admin",
            admin,
            "--verification_contract",
            verification_contract,
        ])?;

        let contract_id = output.trim().trim_matches('"').to_string();
        if contract_id.is_empty() {
            bail!("stellar contract deploy returned empty contract id");
        }
        Ok(contract_id)
    }

    /// Address of the funded testnet wallet used as the deploy source —
    /// also the natural choice for the handler's `admin` argument.
    pub fn wallet_address(&self) -> Result<String> {
        self.ensure_wallet()?;
        let output = self.run_stellar(&["keys", "address", STELLAR_WALLET_ALIAS])?;
        let trimmed = output.trim();
        if trimmed.is_empty() {
            bail!("stellar keys address returned empty output for {STELLAR_WALLET_ALIAS}");
        }
        Ok(trimmed.to_string())
    }

    /// Read the payload stored at `event_id_hex` on the per-test handler.
    /// `event_id_hex` is the bare 40-char hex form of the 20-byte event id
    /// (no `0x` prefix — that's what soroban-cli expects for `BytesN<20>`
    /// arguments).
    pub async fn payload(&self, contract_id: &str, event_id_hex: &str) -> Result<Vec<u8>> {
        let output = self.run_stellar(&[
            "contract",
            "invoke",
            "--id",
            contract_id,
            "--source-account",
            STELLAR_WALLET_ALIAS,
            "--rpc-url",
            STELLAR_RPC_URL_TESTNET,
            "--network-passphrase",
            STELLAR_NETWORK_PASSPHRASE_TESTNET,
            "--send",
            "no",
            "--",
            "payload",
            "--event_id",
            event_id_hex,
        ])?;
        parse_optional_bytes_output(&output)
    }

    /// Poll Soroban RPC for the contract's first `Verified` event and
    /// return the embedded event_id as a 40-char hex string. We deliberately
    /// don't filter by topic on the server side — the contract is fresh per
    /// test and emits exactly one event, so a contract-id filter is plenty.
    ///
    /// `start_ledger` should be a sequence at-or-before the verify_eth
    /// transaction; using the latest ledger just before the trigger fires
    /// works fine and keeps the poll window tight.
    pub async fn wait_for_verified_event_id(
        &self,
        contract_id: &str,
        start_ledger: u32,
        timeout: Duration,
    ) -> Result<String> {
        let rpc = wasi_stellar_rpc_client::Client::new(STELLAR_RPC_URL_TESTNET)
            .map_err(|e| anyhow!("failed to construct stellar rpc client: {e:?}"))?;

        tokio::time::timeout(timeout, async {
            loop {
                let resp = rpc
                    .get_events(
                        wasi_stellar_rpc_client::EventStart::Ledger(start_ledger),
                        Some(wasi_stellar_rpc_client::EventType::Contract),
                        &[contract_id.to_string()],
                        &[],
                        None,
                    )
                    .await;
                match resp {
                    Ok(resp) => {
                        for event in resp.events {
                            if let Some(hex) = extract_verified_event_id(&event) {
                                return Ok(hex);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!(
                            "stellar get_events transient error while polling {}: {e:?}",
                            contract_id
                        );
                    }
                }
                tracing::debug!(
                    "Waiting for Verified event on stellar contract {}",
                    contract_id
                );
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        })
        .await
        .map_err(|_| anyhow!("Timeout waiting for Verified event on {}", contract_id))?
    }

    /// Latest ledger sequence — useful as a `start_ledger` baseline before
    /// firing a trigger so the subsequent event poll has a tight window.
    pub async fn current_ledger(&self) -> Result<u32> {
        let rpc = wasi_stellar_rpc_client::Client::new(STELLAR_RPC_URL_TESTNET)
            .map_err(|e| anyhow!("failed to construct stellar rpc client: {e:?}"))?;
        let info = rpc
            .get_latest_ledger()
            .await
            .map_err(|e| anyhow!("get_latest_ledger failed: {e:?}"))?;
        Ok(info.sequence)
    }

    fn ensure_wallet(&self) -> Result<()> {
        std::fs::create_dir_all(&self.config_dir)
            .with_context(|| format!("create stellar config dir {}", self.config_dir.display()))?;

        let known_keys = self.run_stellar(&["keys", "ls"])?;
        if known_keys
            .lines()
            .any(|line| line.trim() == STELLAR_WALLET_ALIAS)
        {
            return Ok(());
        }

        self.run_stellar(&[
            "keys",
            "generate",
            "--rpc-url",
            STELLAR_RPC_URL_TESTNET,
            "--network-passphrase",
            STELLAR_NETWORK_PASSPHRASE_TESTNET,
            "--fund",
            STELLAR_WALLET_ALIAS,
        ])?;

        Ok(())
    }

    fn contract_artifact_path(&self) -> PathBuf {
        workspace_path().join(STAGED_MOCK_SUBMIT_ETH_WASM)
    }

    fn run_stellar(&self, args: &[&str]) -> Result<String> {
        let output = Command::new("stellar")
            .arg("--config-dir")
            .arg(&self.config_dir)
            .args(args)
            .output()
            .with_context(|| format!("failed to run stellar command: {:?}", args))?;

        if !output.status.success() {
            bail!(
                "stellar command {:?} failed:\nstdout:\n{}\nstderr:\n{}",
                args,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }
}

/// Pulls the 20-byte event_id out of a `Verified` contract event.
///
/// `warpdrive_shared::interfaces::handler::Verified::publish` publishes the
/// event with `topics = [Symbol("verified"), BytesN<20>]` and an empty
/// payload (the body is `Verified::new(event_id)`). We accept both the
/// "event_id in topic[1]" shape and a fallback where the event_id is in the
/// event body — whichever the deployed Soroban runtime uses — so the
/// matcher is robust to upstream tweaks.
fn extract_verified_event_id(event: &wasi_stellar_rpc_client::Event) -> Option<String> {
    let topics: Vec<ScVal> = event
        .topic
        .iter()
        .filter_map(|t| ScVal::from_xdr_base64(t, Limits::none()).ok())
        .collect();
    let first_topic_is_verified = matches!(
        topics.first(),
        Some(ScVal::Symbol(ScSymbol(sym))) if sym.as_slice() == b"verified"
    );
    if !first_topic_is_verified {
        return None;
    }
    for tail in topics.iter().skip(1) {
        if let Some(hex) = scval_as_bytesn20_hex(tail) {
            return Some(hex);
        }
    }
    if let Ok(value) = ScVal::from_xdr_base64(&event.value, Limits::none()) {
        if let Some(hex) = scval_as_bytesn20_hex(&value) {
            return Some(hex);
        }
    }
    None
}

fn scval_as_bytesn20_hex(v: &ScVal) -> Option<String> {
    match v {
        ScVal::Bytes(b) if b.0.len() == 20 => Some(const_hex::encode(&b.0)),
        _ => None,
    }
}

fn random_salt_hex() -> String {
    let mut salt = [0u8; 32];
    rand::rng().fill_bytes(&mut salt);
    salt.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parses the stellar CLI's `Option<Bytes>` output. Soroban Bytes serialize
/// as a hex string in JSON. `None` shows up as `null`.
fn parse_optional_bytes_output(raw: &str) -> Result<Vec<u8>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "null" {
        bail!("stellar mock_submit_eth returned no data for event_id");
    }
    // Try JSON first (handles both quoted strings and bare hex objects).
    if let Ok(s) = serde_json::from_str::<String>(trimmed) {
        return decode_bytes_payload(&s);
    }
    decode_bytes_payload(trimmed.trim_matches('"'))
}

fn decode_bytes_payload(s: &str) -> Result<Vec<u8>> {
    // Soroban Bytes are emitted as a hex string by stellar-cli's JSON output.
    let s = s.strip_prefix("0x").unwrap_or(s);
    const_hex::decode(s).with_context(|| format!("failed to hex-decode bytes payload: {s}"))
}
