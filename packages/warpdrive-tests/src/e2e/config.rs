use rand::prelude::*;
use std::{
    num::NonZeroU32,
    sync::{Arc, RwLock},
};

use alloy_signer_local::{coins_bip39::English, MnemonicBuilder};
use utils::{
    config::{ConfigBuilder, EvmChainConfigExt},
    evm_client::EvmSigningClient,
    filesystem::workspace_path,
    test_utils::middleware::evm::EvmMiddlewareType,
};
use warpdrive::subsystems::aggregator::p2p::P2pConfig;
use warpdrive_types::{
    ChainConfigs, CosmosChainConfigBuilder, Credential, EvmChainConfigBuilder,
    StellarChainConfigBuilder,
};

use crate::config::{TestConfig, TestP2pMode};

use super::matrix::TestMatrix;

pub const CRON_INTERVAL_DATA: &str = "cron-interval data";
// we can go down to 1 for small groups of tests, but it currently causes a long wait in the test runner
// might be a good candidate to use this a a benchmark for increasing throughput
pub static BLOCK_INTERVAL: NonZeroU32 = NonZeroU32::new(10).unwrap();

#[derive(Clone, Debug)]
pub struct Configs {
    pub matrix: TestMatrix,
    pub registry: bool,
    /// WarpDrive configs - one per vector node
    pub warpdrive_configs: Vec<warpdrive::config::Config>,
    pub cli: warpdrive_cli::config::Config,
    pub cli_args: warpdrive_cli::args::CliArgs,
    pub chains: Arc<RwLock<ChainConfigs>>,
    pub mnemonics: TestMnemonics,
    pub middleware_concurrency: bool,
    pub wavs_concurrency: bool,
    pub grouping: bool,
    pub evm_middleware_type: EvmMiddlewareType,
    pub p2p: TestP2pMode,
}

impl Configs {
    /// Number of vector nodes configured
    pub fn num_vectors(&self) -> usize {
        self.warpdrive_configs.len()
    }
}

#[derive(Clone, Debug)]
pub struct TestMnemonics {
    pub cli: Credential,
    pub cli_cosmos: Credential,
    /// Vector mnemonics - one per WarpDrive instance for multi-vector tests
    /// For single-vector tests, only the first one is used
    pub vectors: Vec<Credential>,
    pub aggregator_evm: Credential,
    pub aggregator_cosmos: Credential,
    pub cosmos_middleware: Vec<Credential>,
    /// Stellar deployer secret in "S..." strkey form. Set when any stellar
    /// chain is enabled. The same key is passed to the stellar middleware
    /// container as BYOK and used in-process via warpdrive-client to call
    /// admin-only contract methods.
    pub stellar_middleware: Option<Credential>,
}

impl TestMnemonics {
    pub fn new(num_vectors: usize) -> Self {
        // just some random mnemonics so they don't conflict with binaries, we'll fund it from the anvil wallet upon creation

        // Pre-defined vector mnemonics for multi-vector tests
        // Each vector needs a unique mnemonic to have a unique signing key
        let operator_mnemonics = vec![
            // Vector 0: 0x55a8F5cac28c2dA45aFA89c46e47CC4A445570AE
            "aspect mushroom fly cousin hobby body need dose blind siren shoe annual",
            // Vector 1
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            // Vector 2
            "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
        ];

        let vectors: Vec<Credential> = operator_mnemonics
            .into_iter()
            .take(num_vectors)
            .map(|m| Credential::new(m.to_string()))
            .collect();

        Self {
            // 0x63A513A1c878283BC1fF829d6938f45D714E22A1
            cli: Credential::new(
                "replace course few short practice end crawl element rather strong text fit"
                    .to_string(),
            ),
            cli_cosmos: Credential::new(
                "arch forward congress comfort shove palace staff flat concert such double tooth brown buffalo cycle school change exhaust episode ball embody various enroll tenant"
                    .to_string(),
            ),
            vectors,
            // 0xB1Ebb71428FF42F529708B5Afd2BA6Ad3432f38d
            aggregator_evm: Credential::new(
                "brain medal write network foam renew muscle mirror rather daring bike uniform"
                    .to_string(),
            ),
            aggregator_cosmos: Credential::new(
                "body total lion ritual invest cup destroy kidney fame symptom gasp snake spy between wire style elegant walk furnace total verify clay swamp flavor"
                    .to_string(),
            ),
            cosmos_middleware: vec![],
            stellar_middleware: None,
        }
    }

    pub async fn fund(&self, chain_configs: &ChainConfigs) {
        // Friendbot-fund the stellar deployer if a stellar chain is enabled
        // and a deployer key was generated.
        if let Some(deployer_secret) = &self.stellar_middleware {
            tracing::info!("Stellar deployer secret configured, funding via friendbot");
            for chain_config in chain_configs.stellar_iter() {
                let Some(friendbot_url) = chain_config.friendbot_url.as_ref() else {
                    continue;
                };
                let secret =
                    stellar_strkey::ed25519::PrivateKey::from_string(deployer_secret.as_str())
                        .expect("stellar deployer secret is not a valid S... strkey");
                let signing = ed25519_dalek::SigningKey::from_bytes(&secret.0);
                let public = stellar_strkey::ed25519::PublicKey(signing.verifying_key().to_bytes())
                    .to_string();
                let url = format!("{}?addr={}", friendbot_url.trim_end_matches('/'), public);
                tracing::info!("Friendbot-funding stellar deployer {public} via {url}");
                let resp = reqwest::get(&url).await.unwrap();
                let status = resp.status();
                if !status.is_success() && status.as_u16() != 400 {
                    // 400 means "account already exists" — fine on reruns.
                    let body = resp.text().await.unwrap_or_default();
                    panic!("friendbot funding failed ({status}): {body}");
                }
            }
            tracing::info!("Successfully funded via friendbot");
        } else {
            tracing::warn!("No stellar deployer secret configured, skipping friendbot funding");
        }

        for chain_config in chain_configs.evm_iter() {
            let anvil_mnemonic =
                "test test test test test test test test test test test junk".to_string();
            let anvil_config = chain_config
                .signing_client_config(Credential::new(anvil_mnemonic))
                .unwrap();
            let anvil_client = EvmSigningClient::new(anvil_config).await.unwrap();

            // Collect all mnemonics to fund
            let mut mnemonics_to_fund: Vec<&Credential> =
                vec![&self.cli, &self.aggregator_evm, &self.aggregator_cosmos];

            // Add all vector mnemonics
            for vector in &self.vectors {
                mnemonics_to_fund.push(vector);
            }

            for mnemonic in mnemonics_to_fund {
                let dest_addr = MnemonicBuilder::<English>::default()
                    .phrase(mnemonic.as_str())
                    .build()
                    .unwrap()
                    .address();

                anvil_client.transfer_funds(dest_addr, "100").await.unwrap();
            }
        }
    }

    pub fn push_cosmos_middleware(&mut self) {
        let mut rng = rand::rng();

        let entropy: [u8; 32] = rng.random();
        let mnemonic = bip39::Mnemonic::from_entropy(&entropy).unwrap().to_string();
        self.cosmos_middleware.push(Credential::new(mnemonic));
    }

    /// Generate a fresh ed25519 deployer key for the stellar middleware.
    /// Called once when any stellar chain is enabled.
    pub fn ensure_stellar_middleware(&mut self) {
        if self.stellar_middleware.is_some() {
            return;
        }
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
        // `format!` instead of `.to_string()`: stellar-strkey 0.0.16
        // added an inherent `to_string()` returning `heapless::String`
        // for no_std consumers, which shadows the `ToString` trait impl.
        let secret = format!("{}", stellar_strkey::ed25519::PrivateKey(bytes));
        self.stellar_middleware = Some(Credential::new(secret));
    }
}

/// Number of vectors for multi-vector tests
pub const MULTI_VECTOR_COUNT: usize = 3;
/// Default base port for WarpDrive HTTP servers
pub const DEFAULT_WARPDRIVE_BASE_PORT: u32 = 8000;
/// Default base port for WarpDrive P2P servers
pub const DEFAULT_P2P_BASE_PORT: u16 = 9000;

impl From<TestConfig> for Configs {
    fn from(test_config: TestConfig) -> Self {
        let matrix: TestMatrix = test_config.mode.into();

        // Determine number of vectors based on test matrix
        let num_vectors = if matrix.multi_vector_enabled() {
            MULTI_VECTOR_COUNT
        } else {
            1
        };

        let mut mnemonics = TestMnemonics::new(num_vectors);

        let chain_configs = Arc::new(RwLock::new(ChainConfigs::default()));

        let mut evm_port = 8545;
        let mut evm_chain_id = 31337;

        let mut push_evm_chain = || {
            let http_endpoint = format!("http://127.0.0.1:{}", evm_port);
            let ws_endpoint = format!("ws://127.0.0.1:{}", evm_port);

            let chain_config = EvmChainConfigBuilder {
                http_endpoint: Some(http_endpoint),
                ws_endpoints: vec![ws_endpoint],
                faucet_endpoint: None,
                ws_priority_endpoint_index: None,
            };

            chain_configs
                .write()
                .unwrap()
                .evm
                .insert(evm_chain_id.to_string().parse().unwrap(), chain_config);

            evm_port += 1;
            evm_chain_id += 1;
        };

        let mut cosmos_port = 9545;
        let mut cosmos_chain_id = 1;

        let mut push_cosmos_chain = || {
            let rpc_endpoint = format!("http://127.0.0.1:{}", cosmos_port);

            let chain_config = CosmosChainConfigBuilder {
                rpc_endpoint: Some(rpc_endpoint),
                grpc_endpoint: None,
                gas_price: 0.025,
                gas_denom: "ucosm".to_string(),
                bech32_prefix: "wasm".to_string(),
                faucet_endpoint: None,
            };

            chain_configs.write().unwrap().cosmos.insert(
                format!("wasmd-{}", cosmos_chain_id).parse().unwrap(),
                chain_config,
            );

            mnemonics.push_cosmos_middleware();

            cosmos_port += 1;
            cosmos_chain_id += 1;
        };

        if matrix.evm_regular_chain_enabled() {
            push_evm_chain();
        }

        if matrix.evm_secondary_chain_enabled() {
            push_evm_chain();
        }

        if matrix.cosmos_regular_chain_enabled() {
            push_cosmos_chain();
        }

        if !matrix.stellar.is_empty() {
            match test_config.stellar_quickstart_port {
                Some(port) => {
                    chain_configs.write().unwrap().stellar.insert(
                        "testnet".parse().unwrap(),
                        StellarChainConfigBuilder {
                            chain_poll_interval_ms: 1_000,
                            rpc_url: format!("http://127.0.0.1:{}", port),
                            network_passphrase: "Test SDF Network ; September 2015".to_string(),
                            friendbot_url: Some(format!("http://127.0.0.1:{}/friendbot", port)),
                        },
                    );
                }
                None => {
                    chain_configs.write().unwrap().stellar.insert(
                        "testnet".parse().unwrap(),
                        StellarChainConfigBuilder {
                            chain_poll_interval_ms: 1_000,
                            rpc_url: "https://soroban-testnet.stellar.org".to_string(),
                            network_passphrase: "Test SDF Network ; September 2015".to_string(),
                            friendbot_url: Some(
                                "https://friendbot-testnet.stellar.org/".to_string(),
                            ),
                        },
                    );
                }
            }
            mnemonics.ensure_stellar_middleware();
        }

        // Create WarpDrive configs for each vector
        let mut warpdrive_configs = Vec::with_capacity(num_vectors);
        for vector_index in 0..num_vectors {
            let mut warpdrive_config: warpdrive::config::Config =
                ConfigBuilder::new(warpdrive::args::CliArgs {
                    data: Some(tempfile::tempdir().unwrap().path().to_path_buf()),
                    home: Some(workspace_path()),
                    // deliberately point to a non-existing file
                    dotenv: Some(tempfile::NamedTempFile::new().unwrap().path().to_path_buf()),
                    ..Default::default()
                })
                .build()
                .unwrap();

            warpdrive_config.chains = chain_configs.clone();
            // Each vector gets its own signing mnemonic for unique signing keys
            warpdrive_config.signing_mnemonic = Some(mnemonics.vectors[vector_index].clone());
            warpdrive_config.aggregator_cosmos_credential =
                Some(mnemonics.aggregator_cosmos.clone());
            warpdrive_config.aggregator_evm_credential = Some(mnemonics.aggregator_evm.clone());
            warpdrive_config.aggregator_stellar_credential = mnemonics.stellar_middleware.clone();
            warpdrive_config.dev_endpoints_enabled = true;
            warpdrive_config.port = DEFAULT_WARPDRIVE_BASE_PORT + vector_index as u32;

            // Enable P2P for multi-vector tests
            if num_vectors > 1 {
                match test_config.p2p {
                    TestP2pMode::Kademlia => {
                        // Remote mode: Kademlia DHT discovery
                        // Vector 0 is the bootstrap server (empty bootstrap_nodes)
                        // Vectors 1+ will have bootstrap_nodes set at runtime after vector 0 starts
                        warpdrive_config.p2p = P2pConfig::Remote {
                            listen_port: DEFAULT_P2P_BASE_PORT + vector_index as u16,
                            bootstrap_nodes: vec![], // Set at runtime for vectors 1+
                            max_retry_duration_secs: None,
                            retry_interval_ms: None,
                            submission_ttl_secs: None,
                            max_catchup_submissions: None,
                            cleanup_interval_secs: None,
                            kademlia_discovery_interval_secs: Some(2),
                            catchup_request_timeout_secs: None,
                            max_concurrent_catchup_requests_per_service: None,
                            max_pending_publishes: None,
                            max_stored_submissions_per_service: None,
                        };
                    }
                    TestP2pMode::Mdns => {
                        // Local mode: mDNS discovery
                        warpdrive_config.p2p = P2pConfig::Local {
                            listen_port: DEFAULT_P2P_BASE_PORT + vector_index as u16,
                            max_retry_duration_secs: None,
                            retry_interval_ms: None,
                            submission_ttl_secs: None,
                            max_catchup_submissions: None,
                            cleanup_interval_secs: None,
                            catchup_request_timeout_secs: None,
                            max_concurrent_catchup_requests_per_service: None,
                            max_pending_publishes: None,
                            max_stored_submissions_per_service: None,
                        };
                    }
                }
            }

            warpdrive_configs.push(warpdrive_config);
        }

        let cli_args = warpdrive_cli::args::CliArgs {
            data: Some(tempfile::tempdir().unwrap().path().to_path_buf()),
            home: Some(workspace_path()),
            // deliberately point to a non-existing file
            dotenv: Some(tempfile::NamedTempFile::new().unwrap().path().to_path_buf()),
            ..Default::default()
        };

        let mut cli_config: warpdrive_cli::config::Config =
            ConfigBuilder::new(cli_args.clone()).build().unwrap();

        cli_config.chains = chain_configs.clone();
        // some random mnemonic
        cli_config.evm_credential = Some(mnemonics.cli.clone());
        cli_config.cosmos_mnemonic = Some(mnemonics.cli_cosmos.clone());

        Self {
            matrix,
            registry: test_config.registry.unwrap_or(false),
            cli: cli_config,
            cli_args,
            warpdrive_configs,
            chains: chain_configs,
            mnemonics,
            middleware_concurrency: test_config.middleware_concurrency,
            wavs_concurrency: test_config.wavs_concurrency,
            grouping: test_config.grouping,
            evm_middleware_type: test_config.evm_middleware_type,
            p2p: test_config.p2p,
        }
    }
}
