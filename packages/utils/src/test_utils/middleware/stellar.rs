use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use tempfile::TempDir;
use tokio::process::Command;
use warpdrive_types::StellarChainConfig;

use crate::test_utils::middleware::evm::validate_docker_container_id;

/// Pinned image tag for the Warpdrive Stellar middleware.
/// Bump in lockstep with the warpdrive-contracts repo.
pub const STELLAR_MIDDLEWARE_IMAGE: &str = "ghcr.io/warp-driver/warpdrive-stellar-middleware:0.2.5";

/// Long-lived container that wraps the warpdrive-stellar-middleware image.
/// One container per test run; each `deploy_service_manager` call shells in
/// via `docker exec` and produces a fresh stack of 7 contracts.
///
/// **Test runtime note:** stellar e2e tests run against the public
/// Soroban testnet (`https://soroban-testnet.stellar.org`) — every
/// admin operation (deploy × 7 contracts, `add-signer`, `set-threshold`,
/// `set-project-spec-repo`, mock_submit deploy, `verify_eth`) is a
/// real network round-trip plus a ledger-close wait (~5s each). The
/// upshot is the typical stellar test runs ~90–120s end-to-end
/// dominated by network latency, not anything in our code. EVM/Cosmos
/// hit local anvil/wasmd nodes so they're sub-second per op.
///
/// If we add many more stellar tests, batching deploys (one stack
/// shared across tests, with per-test `mock_submit` + signer set) is
/// the natural optimization — same shape `PoaMiddleware` uses on the
/// EVM side.
#[derive(Clone)]
pub struct StellarMiddleware {
    inner: Arc<StellarMiddlewareInner>,
}

struct StellarMiddlewareInner {
    container_id: String,
    /// Host tmpdir bind-mounted into the container at `/out`. Each deploy
    /// writes its manifest here so the host can read it back.
    out_dir: TempDir,
}

impl StellarMiddleware {
    const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
    const DEPLOY_TIMEOUT: Duration = Duration::from_secs(360);
    const RUNTIME_CALL_TIMEOUT: Duration = Duration::from_secs(120);

    pub async fn new(chain_config: StellarChainConfig, deployer_secret: &str) -> Result<Self> {
        let out_dir = TempDir::new().context("creating stellar middleware out dir")?;
        println!("Created {}", out_dir.path().display());

        // We pass the deployer secret straight to the container as BYOK env
        // vars; the container is the sole signer of admin txs (deploy,
        // add-signer, set-threshold, set-project-spec-repo). The G... is
        // derived inside the container, so we just need to compute it here
        // so the env var is populated.
        let secret_key = stellar_strkey::ed25519::PrivateKey::from_string(deployer_secret)
            .map_err(|e| anyhow::anyhow!("invalid stellar deployer secret: {e:?}"))?;
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_key.0);
        let deployer_address =
            stellar_strkey::ed25519::PublicKey(signing_key.verifying_key().to_bytes()).to_string();

        tracing::info!(
            "Starting stellar middleware container with deployer address {} on RPC {}",
            deployer_address,
            chain_config.rpc_url
        );
        let output = tokio::time::timeout(
            Self::STARTUP_TIMEOUT,
            Command::new("docker")
                .args([
                    "run",
                    "-d",
                    "--rm",
                    "--network",
                    "host",
                    "-e",
                    &format!("RPC_URL={}", chain_config.rpc_url),
                    "-e",
                    &format!("NETWORK_PASSPHRASE={}", chain_config.network_passphrase),
                    "-e",
                    &format!("DEPLOYER_SECRET={deployer_secret}"),
                    "-e",
                    &format!("DEPLOYER_ADDRESS={deployer_address}"),
                    "-v",
                    &format!("{}:/out", out_dir.path().display()),
                    STELLAR_MIDDLEWARE_IMAGE,
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await
        .context("timed out starting stellar middleware container")??;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to start stellar middleware container: {}", stderr);
        }

        let container_id = String::from_utf8(output.stdout)
            .context("stellar middleware container id is not valid UTF-8")?
            .trim()
            .to_string();

        validate_docker_container_id(&container_id).await?;

        Ok(Self {
            inner: Arc::new(StellarMiddlewareInner {
                container_id,
                out_dir,
            }),
        })
    }

    /// Set the service URI on a deployed `project_root`. The stellar analogue
    /// of EVM's `setServiceURI` / Cosmos's `set_service_uri`. Shells into
    /// the long-lived container's `cli.sh set-project-spec-repo`, which
    /// signs the admin tx using the BYOK secret we passed at startup. The
    /// admin secret never leaves the container.
    pub async fn set_service_uri(&self, deploy_file_path: &str, uri: &str) -> Result<()> {
        self.cli_exec(&[
            "set-project-spec-repo",
            "--repo",
            uri,
            "--deploy-file",
            deploy_file_path,
        ])
        .await
    }

    /// Register (or update) a signer on the security contract that matches
    /// the given scheme. Mirrors `cli.sh add-signer`.
    pub async fn add_signer(
        &self,
        deploy_file_path: &str,
        scheme: SignerScheme,
        key_hex: &str,
        weight: u32,
    ) -> Result<()> {
        let weight = weight.to_string();
        self.cli_exec(&[
            "add-signer",
            "--scheme",
            scheme.as_str(),
            "--key",
            key_hex,
            "--weight",
            &weight,
            "--deploy-file",
            deploy_file_path,
        ])
        .await
    }

    /// Set the consensus threshold (numerator/denominator) on the security
    /// contract that matches the given scheme. Mirrors `cli.sh set-threshold`.
    pub async fn set_threshold(
        &self,
        deploy_file_path: &str,
        scheme: SignerScheme,
        numerator: u32,
        denominator: u32,
    ) -> Result<()> {
        let numerator = numerator.to_string();
        let denominator = denominator.to_string();
        self.cli_exec(&[
            "set-threshold",
            "--scheme",
            scheme.as_str(),
            "--numerator",
            &numerator,
            "--denominator",
            &denominator,
            "--deploy-file",
            deploy_file_path,
        ])
        .await
    }

    /// Run `/warpdrive/cli.sh <args>` inside the long-lived container.
    async fn cli_exec(&self, args: &[&str]) -> Result<()> {
        let mut docker_args: Vec<&str> =
            vec!["exec", &self.inner.container_id, "/warpdrive/cli.sh"];
        docker_args.extend_from_slice(args);
        let res = tokio::time::timeout(
            Self::RUNTIME_CALL_TIMEOUT,
            Command::new("docker")
                .args(&docker_args)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()?
                .wait(),
        )
        .await
        .with_context(|| format!("timed out running cli.sh {:?}", args))??;
        if !res.success() {
            bail!("cli.sh {:?} failed (exit {res})", args);
        }
        Ok(())
    }

    pub async fn deploy_service_manager(
        &self,
        scheme: SignerScheme,
    ) -> Result<StellarServiceManager> {
        // Unique filename per deploy so concurrent deploys don't collide.
        let id = uuid::Uuid::now_v7();
        let filename = format!("deploy-{id}.json");
        let in_container_path = format!("/out/{filename}");
        let host_path = self.inner.out_dir.path().join(&filename);

        let res = tokio::time::timeout(
            Self::DEPLOY_TIMEOUT,
            Command::new("docker")
                .args([
                    "exec",
                    &self.inner.container_id,
                    "/warpdrive/cli.sh",
                    "deploy",
                    "--variant",
                    scheme.deploy_variant(),
                    "--output-path",
                    &in_container_path,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()?
                .wait(),
        )
        .await
        .context("timed out deploying stellar service manager")??;

        if !res.success() {
            bail!("Stellar middleware deploy failed (exit {res})");
        }

        // Image 0.2.3+ runs the deploy as root inside the container, so
        // manifests land in `/out` owned by root and the host can't read
        // them via the bind mount. Mirror the warpdrive-contracts
        // Taskfile pattern — `chown -R` `/out` to the host UID/GID
        // inside the container, then read normally. Use the tmpdir we
        // own as the source of truth for the host UID/GID (it was
        // created by this process, so its owner *is* the test user).
        use std::os::unix::fs::MetadataExt;
        let host_meta = tokio::fs::metadata(self.inner.out_dir.path())
            .await
            .context("stat stellar middleware host out dir")?;
        let chown_arg = format!("{}:{}", host_meta.uid(), host_meta.gid());

        let chown_res = tokio::time::timeout(
            Self::RUNTIME_CALL_TIMEOUT,
            Command::new("docker")
                .args([
                    "exec",
                    &self.inner.container_id,
                    "chown",
                    "-R",
                    &chown_arg,
                    "/out",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()?
                .wait(),
        )
        .await
        .context("timed out chown'ing /out in stellar middleware container")??;

        if !chown_res.success() {
            bail!("chown of /out in stellar middleware container failed (exit {chown_res})");
        }

        let manifest_text = tokio::fs::read_to_string(&host_path)
            .await
            .with_context(|| format!("reading stellar deploy manifest {host_path:?}"))?;
        let manifest: StellarDeployManifest =
            serde_json::from_str(&manifest_text).context("parsing stellar deploy manifest")?;

        Ok(StellarServiceManager {
            project_root: manifest.contracts.project_root(),
            contracts: manifest.contracts,
            deploy_file_path: in_container_path,
            admin: manifest.admin,
        })
    }
}

impl Drop for StellarMiddlewareInner {
    fn drop(&mut self) {
        tracing::debug!(
            "Cleaning up stellar middleware container: {}",
            self.container_id
        );
        if let Err(e) = std::process::Command::new("docker")
            .args(["rm", "-f", &self.container_id])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut cmd| cmd.wait())
        {
            tracing::warn!("Failed to remove stellar middleware container: {:?}", e);
        }
    }
}

/// Signature scheme of a deployed security/verification contract pair on
/// the warpdrive Stellar stack. Maps 1:1 to `cli.sh add-signer --scheme`,
/// and 1:1 (via `deploy_variant`) to `cli.sh deploy --variant`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SignerScheme {
    Secp256k1,
    Ed25519,
}

impl SignerScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            SignerScheme::Secp256k1 => "secp256k1",
            SignerScheme::Ed25519 => "ed25519",
        }
    }

    /// `cli.sh deploy --variant` value. The middleware image names the
    /// secp256k1 stack "ethereum" (after the handler) and the ed25519 stack
    /// "stellar". `--variant ethereum` is the cli.sh default but we always
    /// pass it explicitly so the call is unambiguous on either side.
    pub fn deploy_variant(self) -> &'static str {
        match self {
            SignerScheme::Secp256k1 => "ethereum",
            SignerScheme::Ed25519 => "stellar",
        }
    }
}

/// A deployed warpdrive Stellar service manager stack.
/// `project_root` is the contract that fronts the rest; it's what gets stored
/// in `ServiceManager::Stellar { address }`. The other contract ids and the
/// in-container `deploy_file_path` are kept around because subsequent operator
/// management commands (`add-signer`, `set-threshold`) take `--deploy-file`.
#[derive(Clone, Debug)]
pub struct StellarServiceManager {
    pub project_root: stellar_strkey::Contract,
    pub contracts: StellarContracts,
    pub deploy_file_path: String,
    pub admin: String,
}

#[derive(Clone, Debug, Deserialize)]
struct StellarDeployManifest {
    admin: String,
    #[serde(default)]
    #[allow(dead_code)]
    rpc_url: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    network_passphrase: Option<String>,
    #[serde(flatten)]
    contracts: StellarContracts,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "variant", content = "contracts")]
pub enum StellarContracts {
    #[serde(rename = "ethereum")]
    SecpContracts(SecpContracts),
    #[serde(rename = "stellar")]
    EdContracts(EdContracts),
}

impl StellarContracts {
    pub fn project_root(&self) -> stellar_strkey::Contract {
        match self {
            StellarContracts::SecpContracts(c) => c.project_root,
            StellarContracts::EdContracts(c) => c.project_root,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct SecpContracts {
    pub project_root: stellar_strkey::Contract,
    pub secp256k1_security: stellar_strkey::Contract,
    pub secp256k1_verification: stellar_strkey::Contract,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EdContracts {
    pub project_root: stellar_strkey::Contract,
    pub ed25519_security: stellar_strkey::Contract,
    pub ed25519_verification: stellar_strkey::Contract,
}

#[cfg(test)]
mod test {
    use super::*;

    const STELLAR_MANIFEST: &str = r#"{
  "admin": "GCW64HJIQNQAL5UICTGKLJS3K6ANFQDZEQ3GHLXHOC2MK3GTB2EKRNRC",
  "rpc_url": "https://soroban-testnet.stellar.org",
  "network_passphrase": "Test SDF Network ; September 2015",
  "variant": "stellar",
  "contracts": {
    "ed25519_security": "CA3JNPPPWEC6XDRYHSFCJPK3F47H22TPHL6C5SZLEVSYMLDVEB2XNSRF",
    "ed25519_verification": "CBWPVO6YUOTCIBCWOLSO2IJVASWVAVYG3JMTOVHMWX5EK42NGRCHFJFK",
    "project_root": "CAE4HP4DT5BJKYTHQ73O52G6LYQ2FNFASJ5EUGXO7TCA2ANGHX2FLKM4"
  }
}"#;

    const ETHEREUM_MANIFEST: &str = r#"{
  "admin": "GCW64HJIQNQAL5UICTGKLJS3K6ANFQDZEQ3GHLXHOC2MK3GTB2EKRNRC",
  "rpc_url": "https://soroban-testnet.stellar.org",
  "network_passphrase": "Test SDF Network ; September 2015",
  "variant": "ethereum",
  "contracts": {
    "secp256k1_security": "CAT7FK2S4DEZYMPKRSXVYVSTRIYQQNST3ZYJBIPJPXOERAVJBRNBMRV2",
    "secp256k1_verification": "CDT764X55DNVPYG6ICBFE2IJRXG4DYSNQN5NNQC3GXCO6Z4RJ6LCHHLW",
    "project_root": "CCREM2UGATC3XKUTW3JX5CTICSUP23RRKOZK7ODFWDFV2ZV5NVFYWOAU"
  }
}"#;

    #[test]
    fn parses_stellar() {
        let manifest: StellarDeployManifest = serde_json::from_str(STELLAR_MANIFEST).unwrap();
        assert_eq!(
            manifest.admin,
            "GCW64HJIQNQAL5UICTGKLJS3K6ANFQDZEQ3GHLXHOC2MK3GTB2EKRNRC"
        );
        match manifest.contracts {
            StellarContracts::EdContracts(c) => {
                assert_eq!(
                    c.project_root.to_string(),
                    "CAE4HP4DT5BJKYTHQ73O52G6LYQ2FNFASJ5EUGXO7TCA2ANGHX2FLKM4"
                );
                assert_eq!(
                    c.ed25519_verification.to_string(),
                    "CBWPVO6YUOTCIBCWOLSO2IJVASWVAVYG3JMTOVHMWX5EK42NGRCHFJFK"
                );
            }
            StellarContracts::SecpContracts(_) => {
                panic!("Ethereum contracts when expected stellar")
            }
        }
    }

    #[test]
    fn parses_ethereum() {
        let manifest: StellarDeployManifest = serde_json::from_str(ETHEREUM_MANIFEST).unwrap();
        assert_eq!(
            manifest.admin,
            "GCW64HJIQNQAL5UICTGKLJS3K6ANFQDZEQ3GHLXHOC2MK3GTB2EKRNRC"
        );
        match manifest.contracts {
            StellarContracts::EdContracts(_) => {
                panic!("Stellar contracts when expected ethereum")
            }
            StellarContracts::SecpContracts(c) => {
                assert_eq!(
                    c.project_root.to_string(),
                    "CCREM2UGATC3XKUTW3JX5CTICSUP23RRKOZK7ODFWDFV2ZV5NVFYWOAU"
                );
                assert_eq!(
                    c.secp256k1_verification.to_string(),
                    "CDT764X55DNVPYG6ICBFE2IJRXG4DYSNQN5NNQC3GXCO6Z4RJ6LCHHLW"
                );
            }
        }
    }
}
