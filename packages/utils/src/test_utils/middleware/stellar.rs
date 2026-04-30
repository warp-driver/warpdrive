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
pub const STELLAR_MIDDLEWARE_IMAGE: &str =
    "ghcr.io/warp-driver/warpdrive-stellar-middleware:8c0a535";

/// Long-lived container that wraps the warpdrive-stellar-middleware image.
/// One container per test run; each `deploy_service_manager` call shells in
/// via `docker exec` and produces a fresh stack of 7 contracts.
#[derive(Clone)]
pub struct StellarMiddleware {
    inner: Arc<StellarMiddlewareInner>,
}

struct StellarMiddlewareInner {
    container_id: String,
    /// Host tmpdir mounted into the container at `/out`. Each deploy writes
    /// its manifest here so the host can read it back.
    out_dir: TempDir,
}

impl StellarMiddleware {
    const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
    const DEPLOY_TIMEOUT: Duration = Duration::from_secs(180);

    pub async fn new(chain_config: StellarChainConfig) -> Result<Self> {
        let out_dir = TempDir::new().context("creating stellar middleware out dir")?;

        let output = tokio::time::timeout(
            Self::STARTUP_TIMEOUT,
            Command::new("docker")
                .args([
                    "run",
                    "-d",
                    "-e",
                    &format!("RPC_URL={}", chain_config.rpc_url),
                    "-e",
                    &format!("NETWORK_PASSPHRASE={}", chain_config.network_passphrase),
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

    pub async fn deploy_service_manager(&self) -> Result<StellarServiceManager> {
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

        let manifest_text = tokio::fs::read_to_string(&host_path)
            .await
            .with_context(|| format!("reading stellar deploy manifest {host_path:?}"))?;
        let manifest: StellarDeployManifest = serde_json::from_str(&manifest_text)
            .context("parsing stellar deploy manifest")?;

        Ok(StellarServiceManager {
            project_root: manifest.contracts.project_root,
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
    contracts: StellarContracts,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StellarContracts {
    pub secp256k1_security: stellar_strkey::Contract,
    pub secp256k1_verification: stellar_strkey::Contract,
    pub ethereum_handler: stellar_strkey::Contract,
    pub ed25519_security: stellar_strkey::Contract,
    pub ed25519_verification: stellar_strkey::Contract,
    pub stellar_handler: stellar_strkey::Contract,
    pub project_root: stellar_strkey::Contract,
}
