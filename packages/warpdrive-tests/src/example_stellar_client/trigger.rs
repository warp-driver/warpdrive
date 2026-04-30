use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use utils::filesystem::workspace_path;
use warpdrive_types::ChainKey;

use crate::example_evm_client::TriggerId;

const STELLAR_RPC_URL_TESTNET: &str = "https://soroban-testnet.stellar.org";
const STELLAR_NETWORK_PASSPHRASE_TESTNET: &str = "Test SDF Network ; September 2015";
const STELLAR_WALLET_ALIAS: &str = "warpdrive-e2e-testnet";
const STELLAR_CONTRACT_ALIAS: &str = "warpdrive-stellar-trigger-testnet";
const STELLAR_DEPLOY_SALT: &str =
    "0000000000000000000000000000000000000000000000000000000000001337";

#[derive(Clone, Debug)]
pub struct SimpleStellarTriggerClient {
    chain: ChainKey,
    config_dir: PathBuf,
}

impl SimpleStellarTriggerClient {
    pub fn new(chain: ChainKey) -> Self {
        Self {
            chain,
            config_dir: workspace_path().join(".stellar-e2e"),
        }
    }

    pub async fn deploy(&self) -> Result<String> {
        self.ensure_wallet()?;
        self.build_contract()?;

        if let Ok(contract_id) = self.contract_id() {
            return Ok(contract_id);
        }

        let wasm_path = self.contract_artifact_path();
        let wasm_path = wasm_path
            .to_str()
            .ok_or_else(|| anyhow!("invalid wasm path: {}", wasm_path.display()))?;

        self.run_stellar(&[
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
            "--alias",
            STELLAR_CONTRACT_ALIAS,
            "--salt",
            STELLAR_DEPLOY_SALT,
        ])?;

        self.contract_id()
    }

    pub async fn add_trigger(&self, contract_id: &str, data: &str) -> Result<TriggerId> {
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
            "--",
            "add_trigger",
            "--data",
            data,
        ])?;

        Ok(TriggerId::new(parse_u64_output(&output)?))
    }

    pub async fn get_trigger(&self, contract_id: &str, trigger_id: u64) -> Result<String> {
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
            "get_trigger",
            "--trigger-id",
            &trigger_id,
        ])?;

        parse_string_output(&output)
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

    fn build_contract(&self) -> Result<()> {
        let contract_dir = workspace_path().join("examples/contracts/stellar/trigger");

        let output = Command::new("stellar")
            .arg("--config-dir")
            .arg(&self.config_dir)
            .current_dir(&contract_dir)
            .arg("contract")
            .arg("build")
            .arg("--optimize")
            .output()
            .with_context(|| format!("failed to run stellar build for {}", self.chain))?;

        if output.status.success() {
            return Ok(());
        }

        bail!(
            "stellar contract build failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn contract_artifact_path(&self) -> PathBuf {
        workspace_path()
            .join("examples/contracts/stellar/trigger/target/wasm32v1-none/release/warpdrive_stellar_trigger.wasm")
    }

    fn contract_id(&self) -> Result<String> {
        let output = self.run_stellar(&[
            "--quiet",
            "contract",
            "alias",
            "show",
            "--rpc-url",
            STELLAR_RPC_URL_TESTNET,
            "--network-passphrase",
            STELLAR_NETWORK_PASSPHRASE_TESTNET,
            STELLAR_CONTRACT_ALIAS,
        ])?;

        let trimmed = output.trim();
        if trimmed.is_empty() {
            bail!("stellar contract alias show returned empty output");
        }

        Ok(trimmed.to_string())
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

fn parse_u64_output(raw: &str) -> Result<u64> {
    match serde_json::from_str::<u64>(raw) {
        Ok(value) => Ok(value),
        Err(_) => raw
            .trim()
            .parse::<u64>()
            .with_context(|| format!("failed to parse stellar u64 output: {raw}")),
    }
}

fn parse_string_output(raw: &str) -> Result<String> {
    if let Ok(value) = serde_json::from_str::<String>(raw) {
        return Ok(value);
    }

    let trimmed = raw.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        return Ok(trimmed[1..trimmed.len() - 1].to_string());
    }

    Ok(trimmed.to_string())
}

#[allow(dead_code)]
fn _assert_exists(path: &Path) -> Result<()> {
    if path.exists() {
        Ok(())
    } else {
        bail!("expected path to exist: {}", path.display())
    }
}
