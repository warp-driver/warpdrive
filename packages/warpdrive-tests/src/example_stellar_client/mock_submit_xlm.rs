use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

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
/// **Read API**: the xlm handler stores payloads keyed by 20-byte
/// `event_id` (extracted from the envelope) via
/// `payload(event_id) -> Option<Bytes>`, not by `u64` `trigger_id` the
/// way the eth mock does. The flow mirrors the eth mock: each handler
/// is freshly deployed per test and emits a single `Verified` event on
/// every successful `verify_xlm`, so polling that event yields the
/// event_id which we then pass to `payload(...)`.
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

    /// Poll Soroban RPC for the contract's `Triggered` event matching
    /// `trigger_id` and return the embedded event_id as a 40-char hex
    /// string. Prefer this over `wait_for_verified_event_id` for ed25519
    /// flows because the `Triggered { trigger_id, event_id }` event lets
    /// us topic-filter on the trigger we actually fired instead of
    /// relying on "this handler only has one event" — and crucially, the
    /// trigger_id is round-tripped through the
    /// `warpdrive_client::MessageWithId` payload encoded by
    /// `stellar_encode_trigger_output`.
    pub async fn wait_for_triggered_event_id(
        &self,
        contract_id: &str,
        trigger_id: u64,
        start_ledger: u32,
        timeout: Duration,
    ) -> Result<String> {
        super::events::wait_for_triggered_event_id(
            STELLAR_RPC_URL_TESTNET,
            contract_id,
            trigger_id,
            start_ledger,
            timeout,
        )
        .await
    }

    /// Poll Soroban RPC for the contract's first `Verified` event and
    /// return the embedded event_id as a 40-char hex string. Mirrors the
    /// eth client's helper of the same name — the `Verified` event shape
    /// is shared (`warpdrive_shared::interfaces::handler::Verified`).
    pub async fn wait_for_verified_event_id(
        &self,
        contract_id: &str,
        start_ledger: u32,
        timeout: Duration,
    ) -> Result<String> {
        super::events::wait_for_verified_event_id(
            STELLAR_RPC_URL_TESTNET,
            contract_id,
            start_ledger,
            timeout,
        )
        .await
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

fn parse_optional_bytes_output(raw: &str) -> Result<Vec<u8>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "null" {
        bail!("stellar mock_submit_xlm returned no data for event_id");
    }
    if let Ok(s) = serde_json::from_str::<String>(trimmed) {
        return decode_bytes_payload(&s);
    }
    decode_bytes_payload(trimmed.trim_matches('"'))
}

fn decode_bytes_payload(s: &str) -> Result<Vec<u8>> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    const_hex::decode(s).with_context(|| format!("failed to hex-decode bytes payload: {s}"))
}
