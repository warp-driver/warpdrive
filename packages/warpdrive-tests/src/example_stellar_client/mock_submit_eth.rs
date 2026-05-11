use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use rand::RngCore;
use utils::filesystem::workspace_path;

use crate::example_evm_client::TriggerId;

/// Path (relative to the workspace root) of the staged `mock_submit_eth`
/// WASM that `task stellar-build` produces. Both `test-warpdrive-e2e` and
/// `debug-warpdrive-e2e` depend on `stellar-build`, so this file is
/// guaranteed to exist when the e2e suite runs. We deliberately consume
/// the staged copy rather than the per-crate `target/` artifact so the
/// test process doesn't need to shell out to `stellar contract build`
/// (and doesn't need the soroban toolchain on PATH at test time).
const STAGED_MOCK_SUBMIT_ETH_WASM: &str =
    "examples/build/contracts/stellar/warpdrive_stellar_mock_submit_eth.wasm";

const STELLAR_RPC_URL_TESTNET: &str = "https://soroban-testnet.stellar.org";
const STELLAR_NETWORK_PASSPHRASE_TESTNET: &str = "Test SDF Network ; September 2015";
const STELLAR_WALLET_ALIAS: &str = "warpdrive-e2e-testnet";

/// Per-test stellar mock submit contract client for the **secp256k1
/// (ethereum-handler) path**. Builds `examples/contracts/stellar/mock_submit_eth`
/// and deploys a fresh instance bound to the test stack's
/// `secp256k1_verification`. The ed25519 (`mock_submit_xlm`) counterpart is
/// deployed via `SimpleStellarSubmitXlmClient` (see `mock_submit_xlm.rs`);
/// they are kept as separate crates because they implement different
/// handler interfaces (`verify_eth` ABI envelope vs `verify_xlm` XDR
/// envelope) and the test runner queries them differently.
///
/// Mirrors `SimpleStellarTriggerClient` in shape but each test deploys a
/// fresh contract (random salt, no alias) so each test has its own submit
/// destination bound to its own `ethereum_handler` — same per-test isolation
/// EVM/Cosmos give us via `SimpleSubmit` / `MockServiceHandler`.
#[derive(Clone, Debug)]
pub struct SimpleStellarSubmitEthClient {
    config_dir: PathBuf,
}

impl SimpleStellarSubmitEthClient {
    pub fn new(_chain: warpdrive_types::ChainKey) -> Self {
        // `chain` is accepted for symmetry with `SimpleStellarTriggerClient`
        // and so callers can stay chain-aware, but the staged WASM model
        // means we don't need it: the stellar CLI is configured with a
        // hard-coded testnet rpc + passphrase below, the staged artifact
        // lives at a fixed workspace path, and deploys use a random salt.
        Self {
            // Reuse the same shared wallet config dir as the trigger client;
            // it manages the funded testnet identity that's used for both
            // build/deploy operations.
            config_dir: workspace_path().join(".stellar-e2e"),
        }
    }

    /// Deploy a fresh `mock_submit_eth` instance bound to
    /// `verification_contract` (the test stack's `secp256k1_verification`).
    /// Returns the deployed contract id.
    ///
    /// The WASM is built ahead of time by `task stellar-build` and staged
    /// at [`STAGED_MOCK_SUBMIT_ETH_WASM`]; we just point `stellar contract
    /// deploy` at the staged copy. Errors fast with a clear message if the
    /// artifact isn't present — that almost always means the test was
    /// invoked via `cargo test -p warpdrive-tests` directly without the
    /// task wrapper.
    pub async fn deploy(&self, verification_contract: &str) -> Result<String> {
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
            "--verification_contract",
            verification_contract,
        ])?;

        let contract_id = output.trim().trim_matches('"').to_string();
        if contract_id.is_empty() {
            bail!("stellar contract deploy returned empty contract id");
        }
        Ok(contract_id)
    }

    /// Returns whether the given trigger_id has been validated and stored.
    pub async fn is_valid_trigger_id(
        &self,
        contract_id: &str,
        trigger_id: TriggerId,
    ) -> Result<bool> {
        let trigger_id = trigger_id.to_string();
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
            "is_valid_trigger_id",
            "--trigger_id",
            &trigger_id,
        ])?;
        let trimmed = output.trim();
        match trimmed {
            "true" => Ok(true),
            "false" => Ok(false),
            other => bail!("unexpected is_valid_trigger_id output: {other}"),
        }
    }

    /// Reads back the stored payload for a trigger_id. Errors if the trigger
    /// hasn't landed yet. Returns the raw bytes.
    pub async fn get_data(&self, contract_id: &str, trigger_id: TriggerId) -> Result<Vec<u8>> {
        let trigger_id = trigger_id.to_string();
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
            "get_data",
            "--trigger_id",
            &trigger_id,
        ])?;
        parse_optional_bytes_output(&output)
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
        bail!("stellar mock_submit_eth returned no data for trigger");
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
