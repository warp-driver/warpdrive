use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use rand::RngCore;
use utils::filesystem::workspace_path;

/// Path (relative to the workspace root) of the staged `mock_submit_xlm`
/// WASM that `task stellar-build` produces. Both `test-warpdrive-e2e` and
/// `debug-warpdrive-e2e` depend on `stellar-build`, so this file is
/// guaranteed to exist when the e2e suite runs.
const STAGED_MOCK_SUBMIT_XLM_WASM: &str =
    "examples/build/contracts/stellar/warpdrive_stellar_mock_submit_xlm.wasm";

const STELLAR_RPC_URL_TESTNET: &str = "https://soroban-testnet.stellar.org";
const STELLAR_NETWORK_PASSPHRASE_TESTNET: &str = "Test SDF Network ; September 2015";
const STELLAR_WALLET_ALIAS: &str = "warpdrive-e2e-testnet";

/// Per-test stellar mock submit contract client for the **ed25519
/// (stellar-handler) path**. Mirrors `SimpleStellarSubmitEthClient` but
/// targets the `mock_submit_xlm` crate, which implements
/// `StellarHandlerInterface::verify_xlm` (XDR-encoded envelope, ed25519
/// signatures) instead of the eth-shaped `verify_eth` (ABI envelope,
/// secp256k1 signatures).
///
/// The constructor takes both `admin` and `verification_contract` (the
/// xlm handler has a real admin gating `upgrade` / `propose_admin`, unlike
/// the eth mock which only stores `verification_contract`). We pass the
/// e2e wallet alias as the admin — it's the same identity signing the
/// deploy tx, so there's no separate identity to manage.
///
/// **Read API**: not implemented yet. The xlm handler stores payloads
/// keyed by 20-byte `event_id` (extracted from the envelope) via
/// `payload(event_id) -> Option<Bytes>`, not by `u64` `trigger_id` the
/// way the eth mock does (`get_data` / `is_valid_trigger_id`). When the
/// first ed25519 e2e test gets added, wire reads through `payload(...)`
/// and update `stellar_wait_for_task_to_land` (in `e2e/helpers.rs`) to
/// route by scheme.
#[derive(Clone, Debug)]
pub struct SimpleStellarSubmitXlmClient {
    config_dir: PathBuf,
}

impl SimpleStellarSubmitXlmClient {
    pub fn new(_chain: warpdrive_types::ChainKey) -> Self {
        Self {
            // Reuse the same shared wallet config dir as the trigger client;
            // it manages the funded testnet identity that's used for both
            // build/deploy operations.
            config_dir: workspace_path().join(".stellar-e2e"),
        }
    }

    /// Deploy a fresh `mock_submit_xlm` instance bound to
    /// `verification_contract` (the test stack's `ed25519_verification`).
    /// `admin` is the strkey `G...` address that owns the deployed
    /// contract's upgrade / admin-transfer flow — for the e2e tests this
    /// is the same wallet doing the deploy.
    ///
    /// The WASM is built ahead of time by `task stellar-build` and staged
    /// at [`STAGED_MOCK_SUBMIT_XLM_WASM`].
    pub async fn deploy(&self, admin: &str, verification_contract: &str) -> Result<String> {
        self.ensure_wallet()?;

        let wasm_path = self.contract_artifact_path();
        if !wasm_path.exists() {
            bail!(
                "staged mock_submit_xlm wasm not found at {}; run `task stellar-build` \
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
        workspace_path().join(STAGED_MOCK_SUBMIT_XLM_WASM)
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
