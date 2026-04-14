mod cosmos;
mod evm;

use std::{collections::HashMap, sync::Arc, time::Duration};

use cosmos::CosmosInstance;
use evm::EvmInstance;
use utils::{
    context::AppContext,
    telemetry::Metrics,
    test_utils::middleware::{
        cosmos::{CosmosMiddleware, CosmosMiddlewareKind},
        evm::EvmMiddleware,
    },
};
use warpdrive::dispatcher::Dispatcher;
use warpdrive::subsystems::aggregator::p2p::P2pConfig;
use warpdrive_cli::clients::HttpClient;
use warpdrive_types::{ChainKey, ChainKeyNamespace};

use crate::config::TestP2pMode;

use super::config::Configs;

pub struct AppHandles {
    /// One handle per WarpDrive vector instance
    pub wavs_handles: Vec<std::thread::JoinHandle<()>>,
    pub evm_middleware: Option<EvmMiddleware>,
    pub cosmos_middlewares: CosmosMiddlewares,
    _evm_chains: Vec<EvmInstance>,
    _cosmos_chains: Vec<CosmosInstance>,
}

pub type CosmosMiddlewares = Arc<HashMap<ChainKey, CosmosMiddleware>>;

impl AppHandles {
    pub fn start(ctx: &AppContext, configs: &mut Configs, metrics: Metrics) -> Self {
        let mut evm_chains = Vec::new();
        let mut cosmos_chains = Vec::new();

        let mut cosmos_middlewares = HashMap::new();
        {
            let chains = configs.chains.read().unwrap();
            for chain_config in chains.evm_iter() {
                let handle = EvmInstance::spawn(ctx.clone(), configs, chain_config.clone());
                evm_chains.push(handle);
            }

            for (index, chain_config) in chains.cosmos_iter().enumerate() {
                let handle =
                    CosmosInstance::spawn(ctx.clone(), configs, chain_config.clone(), index);

                let chain_key = ChainKey {
                    namespace: ChainKeyNamespace::COSMOS.parse().unwrap(),
                    id: chain_config.chain_id.clone(),
                };
                let middleware = ctx
                    .rt
                    .block_on(CosmosMiddleware::new(
                        chain_config.clone(),
                        CosmosMiddlewareKind::Mock,
                        configs.mnemonics.cosmos_middleware[index].to_string(),
                    ))
                    .unwrap();

                cosmos_middlewares.insert(chain_key, middleware);
                cosmos_chains.push(handle);
            }
        }

        // Spawn one WarpDrive instance per vector
        let mut wavs_handles = Vec::with_capacity(configs.num_vectors());

        // Check if we're using Remote P2P mode (Kademlia)

        if configs.p2p == TestP2pMode::Kademlia && configs.num_vectors() > 1 {
            // Remote mode: start vector 0 first, get bootstrap address, then start others
            wavs_handles = Self::start_warpdrive_remote_mode(ctx, configs, &metrics)
                .expect("Failed to start vectors in remote mode");
        } else {
            // Local mode or single vector: start all at once
            for (vector_index, warpdrive_config) in configs.warpdrive_configs.iter().enumerate() {
                let handle = Self::spawn_wavs_operator(ctx, warpdrive_config, &metrics, vector_index);
                wavs_handles.push(handle);
            }
        }

        let evm_middleware = if evm_chains.is_empty() {
            None
        } else {
            Some(EvmMiddleware::new(configs.evm_middleware_type).unwrap())
        };

        Self {
            wavs_handles,
            evm_middleware,
            cosmos_middlewares: Arc::new(cosmos_middlewares),
            _evm_chains: evm_chains,
            _cosmos_chains: cosmos_chains,
        }
    }

    pub fn try_join(self) -> Vec<std::thread::Result<()>> {
        let mut results = Vec::new();
        for handle in self.wavs_handles {
            results.push(handle.join());
        }
        results
    }

    /// Spawn a single WarpDrive vector
    fn spawn_wavs_operator(
        ctx: &AppContext,
        warpdrive_config: &warpdrive::config::Config,
        metrics: &Metrics,
        vector_index: usize,
    ) -> std::thread::JoinHandle<()> {
        let dispatcher = Arc::new(Dispatcher::new(warpdrive_config, metrics.warpdrive.clone()).unwrap());

        std::thread::spawn({
            let dispatcher = dispatcher.clone();
            let ctx = ctx.clone();
            let config = warpdrive_config.clone();
            let http_metrics = metrics.http.clone();

            move || {
                tracing::info!(
                    "Starting WarpDrive vector {} on port {}",
                    vector_index,
                    config.port
                );
                let health_status = warpdrive::health::SharedHealthStatus::new();
                warpdrive::run_server(
                    ctx,
                    config,
                    dispatcher,
                    http_metrics,
                    health_status,
                    warpdrive::log_buffer::LogBufferInner::new(),
                );
            }
        })
    }

    /// Start WarpDrive vectors in Remote P2P mode (Kademlia)
    /// Vector 0 starts first as bootstrap server, others connect to it
    fn start_warpdrive_remote_mode(
        ctx: &AppContext,
        configs: &Configs,
        metrics: &Metrics,
    ) -> Result<Vec<std::thread::JoinHandle<()>>, anyhow::Error> {
        let mut handles = Vec::with_capacity(configs.num_vectors());

        // Start vector 0 (bootstrap server)
        let op0_config = &configs.warpdrive_configs[0];
        tracing::info!("Starting vector 0 as bootstrap server");
        handles.push(Self::spawn_wavs_operator(ctx, op0_config, metrics, 0));

        // Wait for vector 0 to be ready and get its bootstrap address
        let op0_url = format!("http://127.0.0.1:{}", op0_config.port);
        let bootstrap_addr = ctx.rt.block_on(async {
            let client = HttpClient::new(op0_url);

            // Wait for the server to be ready
            tokio::time::sleep(Duration::from_millis(500)).await;

            let start = std::time::Instant::now();
            let timeout = Duration::from_secs(30); // Increased from 6s to 30s
            let mut delay = Duration::from_millis(100);

            loop {
                match client.get_p2p_status().await {
                    Ok(status) => {
                        // Prefer external_addresses, fall back to listen_addresses
                        let addr = status
                            .external_addresses
                            .first()
                            .or(status.listen_addresses.first())
                            .cloned();

                        if let Some(addr) = addr {
                            tracing::info!("Got bootstrap address from vector 0: {}", addr);
                            return Ok(addr);
                        }
                    }
                    Err(e) => {
                        tracing::debug!("Waiting for vector 0 P2P status: {:?}", e);
                    }
                }

                if start.elapsed() >= timeout {
                    return Err(anyhow::anyhow!(
                        "Timed out waiting for vector 0 bootstrap address after 30s"
                    ));
                }

                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(5)); // Exponential backoff, max 5s
            }
        })?;

        // Start remaining vectors with the bootstrap address
        for (vector_index, warpdrive_config) in configs.warpdrive_configs.iter().enumerate().skip(1) {
            // Clone and modify config to add bootstrap address
            let mut config = warpdrive_config.clone();
            if let P2pConfig::Remote {
                listen_port,
                bootstrap_nodes: _,
                max_retry_duration_secs,
                retry_interval_ms,
                submission_ttl_secs,
                max_catchup_submissions,
                cleanup_interval_secs,
                kademlia_discovery_interval_secs,
                max_pending_publishes,
                max_stored_submissions_per_service,
                catchup_request_timeout_secs,
                max_concurrent_catchup_requests_per_service,
            } = &config.p2p
            {
                config.p2p = P2pConfig::Remote {
                    listen_port: *listen_port,
                    bootstrap_nodes: vec![bootstrap_addr.clone()],
                    max_retry_duration_secs: *max_retry_duration_secs,
                    retry_interval_ms: *retry_interval_ms,
                    submission_ttl_secs: *submission_ttl_secs,
                    max_catchup_submissions: *max_catchup_submissions,
                    cleanup_interval_secs: *cleanup_interval_secs,
                    kademlia_discovery_interval_secs: *kademlia_discovery_interval_secs,
                    max_pending_publishes: *max_pending_publishes,
                    max_stored_submissions_per_service: *max_stored_submissions_per_service,
                    catchup_request_timeout_secs: *catchup_request_timeout_secs,
                    max_concurrent_catchup_requests_per_service:
                        *max_concurrent_catchup_requests_per_service,
                };
            }

            tracing::info!(
                "Starting vector {} with bootstrap: {}",
                vector_index,
                bootstrap_addr
            );
            handles.push(Self::spawn_wavs_operator(
                ctx,
                &config,
                metrics,
                vector_index,
            ));
        }

        Ok(handles)
    }
}
