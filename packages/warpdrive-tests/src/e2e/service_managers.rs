use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use futures::{stream::FuturesUnordered, StreamExt};
use utils::test_utils::{
    middleware::{
        cosmos::CosmosServiceManager,
        evm::{EvmMiddleware, MiddlewareServiceManagerConfig},
        stellar::{SignerScheme, StellarMiddleware, StellarServiceManager},
        vector::AvsOperator,
    },
    mock_service_manager::MockEvmServiceManager,
};
use warpdrive_cli::command::deploy_service::DeployService;
use warpdrive_types::{
    ChainKey, ChainKeyNamespace, Service, ServiceManager, ServiceStatus, SignatureAlgorithm,
    SignatureKind, SignaturePrefix, SignerResponse,
};

use crate::{
    deployment::ServiceDeployment,
    e2e::{handles::CosmosMiddlewares, helpers::wait_for_evm_trigger_streams_to_finalize},
};

use crate::e2e::{
    clients::Clients,
    components::ComponentSources,
    config::Configs,
    helpers::create_service_for_test,
    test_registry::{CosmosCodeMap, TestRegistry},
};

#[derive(Clone)]
pub struct ServiceManagers {
    configs: Arc<Configs>,
    lookup: Arc<HashMap<String, AnyServiceManagerInstance>>,
    /// One shared Stellar stack per `SignerScheme`. All Stellar tests on a
    /// given scheme route to the same `project_root`, so we only pay the
    /// ~120s testnet deploy once instead of N times. Per-test isolation
    /// lives downstream of `project_root` in the per-test `mock_submit`
    /// handler (`helpers::deploy_submit_contract`).
    stellar_stacks: Arc<HashMap<SignerScheme, SharedStellarStack>>,
}

pub enum AnyServiceManagerInstance {
    Evm {
        chain: ChainKey,
        manager: MockEvmServiceManager,
    },
    Cosmos {
        chain: ChainKey,
        manager: CosmosServiceManager,
    },
    /// Stellar tests reference a shared stack by `scheme` rather than
    /// owning a unique `StellarServiceManager`. Look up the stack on
    /// `ServiceManagers::stellar_stacks` to get `project_root`,
    /// `deploy_file_path`, the `StellarMiddleware` handle, and the
    /// per-stack URI mutex.
    Stellar {
        chain: ChainKey,
        scheme: SignerScheme,
    },
}

/// A pre-deployed Stellar middleware stack (one `project_root` plus its
/// `*_security`, `*_verification`, and `*_handler` contracts) shared by
/// every test that uses the matching `SignerScheme`. The single
/// `project_root` only holds one service URI at a time, so writes to it
/// must serialize on `uri_lock`.
#[derive(Clone)]
pub struct SharedStellarStack {
    pub chain: ChainKey,
    pub manager: StellarServiceManager,
    pub middleware: StellarMiddleware,
    pub uri_lock: Arc<tokio::sync::Mutex<()>>,
}

impl ServiceManagers {
    pub fn new(configs: Configs) -> Self {
        Self {
            lookup: Arc::new(HashMap::new()),
            stellar_stacks: Arc::new(HashMap::new()),
            configs: Arc::new(configs),
        }
    }
}

impl ServiceManagers {
    pub async fn bootstrap(
        &mut self,
        registry: &TestRegistry,
        clients: &Clients,
        evm_middleware: Option<EvmMiddleware>,
        cosmos_middlewares: CosmosMiddlewares,
        stellar_middleware: Option<StellarMiddleware>,
    ) {
        tracing::warn!("WarpDrive Concurrency: {}", self.configs.wavs_concurrency);
        tracing::warn!(
            "Middleware Concurrency: {}",
            self.configs.middleware_concurrency
        );
        tracing::warn!("Bootstrapping service managers...");
        self.deploy_service_managers(
            registry,
            clients,
            evm_middleware,
            cosmos_middlewares,
            stellar_middleware,
        )
        .await;
        tracing::warn!("Bootstrapping initial service uris...");
        self.set_initial_service_uris(registry, clients).await;
        tracing::warn!("Bootstrapping initial services...");
        self.deploy_initial_wavs_services(registry, clients).await;
        tracing::warn!("Bootstrapping register vectors...");
        self.register_operators(registry, clients).await;
    }

    pub fn get_service_manager(&self, test_name: &str) -> ServiceManager {
        match self.lookup.get(test_name).unwrap() {
            AnyServiceManagerInstance::Evm { chain, manager } => ServiceManager::Evm {
                chain: chain.clone(),
                address: manager.address(),
            },
            AnyServiceManagerInstance::Cosmos { chain, manager } => ServiceManager::Cosmos {
                chain: chain.clone(),
                address: manager.address.clone(),
            },
            AnyServiceManagerInstance::Stellar { chain, scheme } => {
                let stack = self.stellar_stacks.get(scheme).expect(
                    "shared Stellar stack for scheme missing — bootstrap should have deployed it",
                );
                ServiceManager::Stellar {
                    chain: chain.clone(),
                    address: stack.manager.project_root,
                }
            }
        }
    }

    /// Resolve the `SharedStellarStack` for a registered test, if it is a
    /// Stellar test. Used by the test runner / submit-contract deploy to
    /// reach the per-scheme middleware, URI lock, and contracts manifest.
    pub fn stellar_stack_for_test(&self, test_name: &str) -> Option<SharedStellarStack> {
        match self.lookup.get(test_name)? {
            AnyServiceManagerInstance::Stellar { scheme, .. } => {
                self.stellar_stacks.get(scheme).cloned()
            }
            _ => None,
        }
    }

    pub async fn deploy_service_managers(
        &mut self,
        registry: &TestRegistry,
        clients: &Clients,
        evm_middleware: Option<EvmMiddleware>,
        cosmos_middlewares: CosmosMiddlewares,
        stellar_middleware: Option<StellarMiddleware>,
    ) {
        let mut lookup = HashMap::new();

        // --- Step 1: pre-deploy one shared Stellar stack per scheme used by
        // the active matrix. Each `cli.sh deploy` is a ~120s testnet round-
        // trip; doing this once per scheme rather than once per test is the
        // whole point of the shared-stack model.
        let stellar_schemes: HashSet<(SignerScheme, ChainKey)> = registry
            .list_all()
            .filter_map(|test| {
                let chain = test.service_manager_chain.as_ref()?;
                if chain.namespace.as_str() != ChainKeyNamespace::STELLAR {
                    return None;
                }
                let scheme = test
                    .stellar_scheme
                    .expect("Stellar test registered without a stellar_scheme");
                Some((scheme, chain.clone()))
            })
            .collect();

        let mut stellar_stacks: HashMap<SignerScheme, SharedStellarStack> = HashMap::new();
        for (scheme, chain) in stellar_schemes {
            if let Some(existing) = stellar_stacks.get(&scheme) {
                // We only support one chain per scheme: there's a single
                // `StellarMiddleware` container, so multiple stellar chains
                // would need a separate middleware per chain.
                assert_eq!(
                    existing.chain, chain,
                    "Stellar tests for scheme {scheme:?} use different chains \
                     ({} vs {chain}); only one chain per scheme is supported",
                    existing.chain
                );
                continue;
            }
            let middleware = stellar_middleware
                .clone()
                .expect("stellar middleware not initialized");
            tracing::info!(
                "Deploying shared stellar stack for scheme {:?} on chain {}",
                scheme,
                chain
            );
            let manager = middleware.deploy_service_manager(scheme).await.unwrap();
            tracing::info!(
                "Shared stellar stack ({:?}) project_root is {}",
                scheme,
                manager.project_root
            );
            stellar_stacks.insert(
                scheme,
                SharedStellarStack {
                    chain,
                    manager,
                    middleware,
                    uri_lock: Arc::new(tokio::sync::Mutex::new(())),
                },
            );
        }
        self.stellar_stacks = Arc::new(stellar_stacks);

        // --- Step 2: build per-test lookup entries. EVM/Cosmos still deploy a
        // unique service manager per test (anvil / wasmd are cheap); Stellar
        // tests just record their scheme and reference the shared stack.
        let mut futures = Vec::new();

        for test in registry.list_all() {
            let chain = test
                .service_manager_chain
                .clone()
                .unwrap_or_else(|| panic!("missing service manager chain for test {}", test.name));
            let stellar_scheme = test.stellar_scheme;
            futures.push({
                let evm_middleware = evm_middleware.clone();
                let cosmos_middlewares = cosmos_middlewares.clone();
                async move {
                    match chain.namespace.as_str() {
                        ChainKeyNamespace::EVM => {
                            let wallet_client = clients.get_evm_client(&chain);
                            let test_name = test.name.clone();
                            let middleware = evm_middleware.clone().unwrap();
                            tracing::info!("Deploying service manager for test {}", test_name);
                            let manager = MockEvmServiceManager::new(middleware, wallet_client)
                                .await
                                .unwrap();
                            tracing::info!(
                                "EVM Service manager for test {} is {}",
                                test_name,
                                manager.address()
                            );
                            (test_name, AnyServiceManagerInstance::Evm { manager, chain })
                        }
                        ChainKeyNamespace::COSMOS => {
                            let middleware = cosmos_middlewares.get(&chain).unwrap();
                            let manager = middleware.deploy_service_manager().await.unwrap();
                            tracing::info!(
                                "Cosmos Service manager for test {} is {}",
                                test.name,
                                manager.address
                            );
                            (
                                test.name.clone(),
                                AnyServiceManagerInstance::Cosmos { manager, chain },
                            )
                        }
                        ChainKeyNamespace::STELLAR => {
                            let scheme = stellar_scheme
                                .expect("Stellar test missing stellar_scheme at lookup time");
                            (
                                test.name.clone(),
                                AnyServiceManagerInstance::Stellar { chain, scheme },
                            )
                        }
                        other => panic!("Unsupported chain namespace: {}", other),
                    }
                }
            });
        }

        tracing::info!("Deploying {} service managers", futures.len());

        if self.configs.middleware_concurrency {
            let mut futures_unordered = FuturesUnordered::from_iter(futures);
            while let Some((test_name, value)) = futures_unordered.next().await {
                if lookup.insert(test_name.clone(), value).is_some() {
                    panic!("Service manager for test {} already exists", test_name);
                }
            }
        } else {
            for future in futures {
                let (test_name, value) = future.await;
                if lookup.insert(test_name.clone(), value).is_some() {
                    panic!("Service manager for test {} already exists", test_name);
                }
            }
        }

        self.lookup = Arc::new(lookup);
    }

    pub async fn set_initial_service_uris(&self, registry: &TestRegistry, clients: &Clients) {
        let mut futures = Vec::new();

        // Stellar tests on a shared stack all resolve to the same on-chain
        // project_root with a single URI slot, and `ServiceId` is derived
        // purely from the manager address (`Service::id`) — so per-test
        // bootstrap rows would all collide on the WarpDrive side. We pick
        // one representative test per scheme to set an initial Paused URI;
        // its `change_service` path then carries every other test of the
        // same scheme via per-test `update_services` calls.
        let mut bootstrapped_stellar_schemes: HashSet<SignerScheme> = HashSet::new();

        for test in registry.list_all() {
            let service_manager_instance = self.lookup.get(&test.name).unwrap();

            if let AnyServiceManagerInstance::Stellar { scheme, .. } = service_manager_instance {
                if !bootstrapped_stellar_schemes.insert(*scheme) {
                    // Already bootstrapped via the representative test for this scheme.
                    continue;
                }
            }

            let service_manager = self.get_service_manager(&test.name);

            let service = Service {
                name: test.name.to_string(),
                workflows: Default::default(),
                status: ServiceStatus::Paused,
                manager: service_manager,
                signature_kind: match test.stellar_scheme {
                    Some(SignerScheme::Ed25519) => SignatureKind {
                        algorithm: SignatureAlgorithm::Ed25519,
                        prefix: Some(SignaturePrefix::Sep53),
                    },
                    _ => SignatureKind::evm_default(),
                },
            };

            // Save the service on WarpDrive endpoint (just a local test thing, real-world would be IPFS or similar)
            let service_url = DeployService::save_service(&clients.cli_ctx, &service)
                .await
                .unwrap();

            // Pre-resolve the shared Stellar stack so the future doesn't
            // need to keep `self` borrowed for the matching arm.
            let stellar_stack = match service_manager_instance {
                AnyServiceManagerInstance::Stellar { scheme, .. } => {
                    Some(self.stellar_stacks.get(scheme).unwrap().clone())
                }
                _ => None,
            };

            futures.push(async move {
                match service_manager_instance {
                    AnyServiceManagerInstance::Evm { manager, .. } => {
                        manager.set_service_uri(service_url).await.unwrap();
                    }
                    AnyServiceManagerInstance::Cosmos { manager, .. } => {
                        manager.set_service_uri(&service_url).await.unwrap();
                    }
                    AnyServiceManagerInstance::Stellar { .. } => {
                        let stack = stellar_stack
                            .as_ref()
                            .expect("Stellar branch entered without a resolved stack");
                        let _guard = stack.uri_lock.clone().lock_owned().await;
                        stack
                            .middleware
                            .set_service_uri(&stack.manager.deploy_file_path, &service_url)
                            .await
                            .unwrap();
                    }
                }
            });
        }

        if self.configs.middleware_concurrency {
            futures::future::join_all(futures).await;
        } else {
            for future in futures {
                future.await;
            }
        }
    }

    pub async fn deploy_initial_wavs_services(
        &mut self,
        registry: &TestRegistry,
        clients: &Clients,
    ) {
        let mut futures = Vec::new();

        // See `set_initial_service_uris`: only the representative test per
        // Stellar scheme actually registers the (shared) service on
        // WarpDrive at bootstrap. Subsequent stellar tests reuse the same
        // `ServiceId` and are switched in via `update_services` →
        // `change_service` when their turn comes.
        let mut registered_stellar_schemes: HashSet<SignerScheme> = HashSet::new();

        for test in registry.list_all() {
            if let Some(AnyServiceManagerInstance::Stellar { scheme, .. }) =
                self.lookup.get(&test.name)
            {
                if !registered_stellar_schemes.insert(*scheme) {
                    continue;
                }
            }

            let service_manager = self.get_service_manager(&test.name);
            let http_clients = clients.http_clients.clone();

            futures.push(async move {
                tracing::info!("Deploying service {} on all WarpDrive instances", test.name);

                // Deploy the service on ALL WarpDrive instances
                for (idx, http_client) in http_clients.iter().enumerate() {
                    tracing::info!(
                        "Deploying service {} on WarpDrive instance {}",
                        test.name,
                        idx
                    );
                    http_client
                        .create_service(service_manager.clone(), None)
                        .await
                        .unwrap();
                }
            });
        }

        if self.configs.wavs_concurrency {
            let mut futures_unordered = FuturesUnordered::from_iter(futures);
            while (futures_unordered.next().await).is_some() {}
        } else {
            for future in futures {
                future.await;
            }
        }
    }

    pub async fn register_operators(&self, registry: &TestRegistry, clients: &Clients) {
        use crate::e2e::config::MULTI_VECTOR_COUNT;

        // --- Stellar: register signers + threshold ONCE per shared stack.
        // All Stellar tests on the same scheme share the project_root +
        // security contract, so the on-chain signer set is the same for
        // every test. Doing this per-test would just re-add the same keys
        // and re-set the same threshold, paying extra testnet round-trips.
        //
        // We use the same num_vectors / threshold rule as the EVM/Cosmos
        // path: 2/3 quorum for multi-vector, otherwise 1. No Stellar test
        // currently has `multi_vector = true`, but we honor it if any test
        // on a given scheme requests it.
        for (scheme, stack) in self.stellar_stacks.iter() {
            let any_multi_vector = registry.list_all().any(|test| {
                test.multi_vector
                    && test.stellar_scheme == Some(*scheme)
                    && test
                        .service_manager_chain
                        .as_ref()
                        .map(|c| c.namespace.as_str() == ChainKeyNamespace::STELLAR)
                        .unwrap_or(false)
            });

            let num_vectors = std::cmp::min(MULTI_VECTOR_COUNT, clients.http_clients.len());

            let stack_service_manager = ServiceManager::Stellar {
                chain: stack.chain.clone(),
                address: stack.manager.project_root,
            };

            // Per-operator pubkey hex for the security contract's
            // `add_signer(key, weight)`. The key format is
            // scheme-dependent: secp256k1 takes the compressed sec1 form
            // (33 bytes), ed25519 takes the raw 32-byte public key.
            let mut operator_pubkeys_hex: Vec<String> = Vec::with_capacity(num_vectors);
            for operator_offset in 0..num_vectors {
                let http_client = &clients.http_clients[operator_offset];
                let signer = http_client
                    .get_service_signer(stack_service_manager.clone())
                    .await
                    .unwrap();
                // Unlike EVM/Cosmos, Stellar `add-signer` is signed by the
                // middleware container's admin key, not by the operator
                // itself, so per-test unique HD indexes aren't needed to
                // avoid nonce collisions. Each vector reuses the same
                // mnemonic at the WarpDrive instance's reported hd_index.
                let operator_mnemonic = &self.configs.mnemonics.vectors[operator_offset];

                // WarpDrive's `add_service_key` picks the signer's
                // algorithm by reading the service's workflow
                // `signature_kind` — but at this point in bootstrap the
                // service has only the empty-workflow placeholder
                // (`set_initial_service_uris`), so it always falls back
                // to `SignatureKind::evm_default()` (secp256k1) even on
                // ed25519 stacks. The signer is re-derived at the right
                // hd_index once the real workflows arrive via
                // `update_services`, so here we just need the hd_index
                // and we derive the on-chain key from the operator's
                // mnemonic ourselves.
                let pubkey_hex = match *scheme {
                    SignerScheme::Secp256k1 => {
                        let hd_index = signer.hd_index();
                        let signing_signer = utils::evm_client::signing::make_signer(
                            operator_mnemonic,
                            Some(hd_index),
                        )
                        .unwrap();
                        if let SignerResponse::Secp256k1 {
                            evm_address: avs_signer_address,
                            ..
                        } = &signer
                        {
                            assert_eq!(
                                signing_signer.address().to_string().to_lowercase(),
                                avs_signer_address.to_lowercase(),
                                "Derived signing address doesn't match WarpDrive signer address \
                                 for vector {operator_offset}"
                            );
                        }
                        // Stellar's `secp256k1_security` contract keys on
                        // the compressed sec1 pubkey (0x02/0x03 || x).
                        let secret_bytes = signing_signer.to_bytes();
                        let secp_key = k256::ecdsa::SigningKey::from_slice(secret_bytes.as_slice())
                            .expect("operator signing key not a valid secp256k1 key");
                        const_hex::encode(secp_key.verifying_key().to_sec1_bytes())
                    }
                    SignerScheme::Ed25519 => {
                        let hd_index = signer.hd_index();
                        // SLIP-0010 / SEP-0005 derivation, same path the
                        // WarpDrive instance uses for its signing key
                        // once it picks up an ed25519 workflow.
                        let signing_key = utils::stellar_client::make_stellar_signer(
                            operator_mnemonic,
                            Some(hd_index),
                        )
                        .unwrap();
                        // Only sanity-check the strkey match if WarpDrive
                        // already reports the matching algorithm. Until
                        // `update_services` runs with an ed25519 workflow
                        // it'll still be returning the bootstrap-default
                        // secp256k1 response, which we accept here.
                        if let SignerResponse::Ed25519 {
                            stellar_pubkey: avs_stellar_pubkey,
                            ..
                        } = &signer
                        {
                            let derived_strkey = format!(
                                "{}",
                                stellar_strkey::ed25519::PublicKey(
                                    *signing_key.verifying_key().as_bytes(),
                                )
                            );
                            assert_eq!(
                                derived_strkey, *avs_stellar_pubkey,
                                "Derived stellar pubkey doesn't match WarpDrive signer pubkey \
                                 for vector {operator_offset}"
                            );
                        }
                        // `ed25519_security` contract takes the raw
                        // 32-byte BytesN<32> public key.
                        const_hex::encode(signing_key.verifying_key().as_bytes())
                    }
                };

                operator_pubkeys_hex.push(pubkey_hex);
            }

            let required_to_pass = if any_multi_vector {
                ((num_vectors as u64) * 2).div_ceil(3)
            } else {
                1
            };
            let denominator = std::cmp::max(num_vectors, 1) as u32;

            for pubkey_hex in &operator_pubkeys_hex {
                stack
                    .middleware
                    .add_signer(
                        &stack.manager.deploy_file_path,
                        *scheme,
                        pubkey_hex,
                        AvsOperator::DEFAULT_WEIGHT as u32,
                    )
                    .await
                    .unwrap();
            }
            stack
                .middleware
                .set_threshold(
                    &stack.manager.deploy_file_path,
                    *scheme,
                    required_to_pass as u32,
                    denominator,
                )
                .await
                .unwrap();
        }

        // --- EVM / Cosmos: still per-test (anvil / wasmd round-trips are cheap).
        let mut futures = Vec::new();

        for (test_index, test) in registry.list_all().enumerate() {
            let service_manager_instance = self.lookup.get(&test.name).unwrap();
            if matches!(
                service_manager_instance,
                AnyServiceManagerInstance::Stellar { .. }
            ) {
                // Already registered once per shared stack above.
                continue;
            }

            let service_manager = self.get_service_manager(&test.name);

            // Register vectors for all running WarpDrive instances since any of them
            // might execute aggregation and submit. Cap at the number of available
            // instances (may be less than MULTI_VECTOR_COUNT for isolated tests).
            let num_vectors = std::cmp::min(MULTI_VECTOR_COUNT, clients.http_clients.len());

            // Collect all vectors for this test
            let mut avs_operators = Vec::with_capacity(num_vectors);

            for operator_offset in 0..num_vectors {
                // Reuse existing HTTP client for this WarpDrive instance
                let http_client = &clients.http_clients[operator_offset];

                let SignerResponse::Secp256k1 {
                    evm_address: avs_signer_address,
                    hd_index: wavs_signer_hd_index,
                    ..
                } = http_client
                    .get_service_signer(service_manager.clone())
                    .await
                    .unwrap()
                else {
                    panic!("e2e tests assume secp256k1 service signers");
                };

                // unique HD index per test and vector to avoid nonce collisions
                let operator_hd_index = (test_index * MULTI_VECTOR_COUNT + operator_offset) as u32;
                let operator_mnemonic = &self.configs.mnemonics.vectors[operator_offset];
                let operator_signer = utils::evm_client::signing::make_signer(
                    operator_mnemonic,
                    Some(operator_hd_index),
                )
                .unwrap();
                let vector_address = operator_signer.address();
                let operator_private_key = const_hex::encode(operator_signer.to_bytes());

                // Get the signing key that this WarpDrive instance will use
                let signing_signer = utils::evm_client::signing::make_signer(
                    operator_mnemonic,
                    Some(wavs_signer_hd_index),
                )
                .unwrap();
                let signing_address = signing_signer.address();
                let signing_private_key = const_hex::encode(signing_signer.to_bytes());

                assert_eq!(
                    signing_address.to_string().to_lowercase(),
                    avs_signer_address.to_lowercase(),
                    "Derived signing address doesn't match WarpDrive signer address for vector {}",
                    operator_offset
                );

                let avs_operator = AvsOperator::with_keys(
                    vector_address,
                    signing_address,
                    operator_private_key,
                    signing_private_key,
                );

                avs_operators.push(avs_operator);
            }

            // Calculate required signatures for quorum
            // Multi-vector: 2/3 quorum (requires multiple signatures)
            // Single-vector: quorum of 1 (any single vector can submit)
            let required_to_pass = if test.multi_vector {
                ((num_vectors as u64) * 2).div_ceil(3)
            } else {
                1
            };

            futures.push(async move {
                match service_manager_instance {
                    AnyServiceManagerInstance::Evm { manager, .. } => {
                        let config =
                            MiddlewareServiceManagerConfig::new(&avs_operators, required_to_pass);
                        manager.configure(&config).await.unwrap();
                        // Validate that vectors are properly registered before proceeding
                        manager
                            .validate_operator_registration(&config)
                            .await
                            .unwrap();
                    }
                    AnyServiceManagerInstance::Cosmos { manager, .. } => {
                        // For Cosmos, register each vector individually
                        for vector in avs_operators {
                            manager.register_operator(vector.clone()).await.unwrap();
                        }
                    }
                    AnyServiceManagerInstance::Stellar { .. } => {
                        unreachable!("Stellar handled above");
                    }
                }
            });
        }

        if self.configs.middleware_concurrency {
            let mut futures_unordered = FuturesUnordered::from_iter(futures);
            while (futures_unordered.next().await).is_some() {}
        } else {
            for future in futures {
                future.await;
            }
        }
    }

    pub async fn create_real_wavs_services(
        &mut self,
        registry: &TestRegistry,
        clients: &Clients,
        component_sources: &ComponentSources,
        cosmos_code_map: CosmosCodeMap,
    ) -> HashMap<String, ServiceDeployment> {
        let mut futures = Vec::new();

        for test in registry.list_all() {
            let service_manager = self.get_service_manager(&test.name);
            // Pull the shared StellarServiceManager out of the per-scheme
            // stack if this is a Stellar test — `deploy_submit_contract`
            // needs the `*_verification` address from its manifest.
            let stellar_service_manager = self
                .stellar_stack_for_test(&test.name)
                .map(|stack| stack.manager.clone());

            futures.push(create_service_for_test(
                test,
                clients,
                component_sources,
                service_manager,
                cosmos_code_map.clone(),
                stellar_service_manager,
            ));
        }

        let mut services = HashMap::new();

        if self.configs.wavs_concurrency {
            let mut futures_unordered = FuturesUnordered::from_iter(futures);
            while let Some(deployment_result) = futures_unordered.next().await {
                services.insert(deployment_result.service.name.clone(), deployment_result);
            }
        } else {
            for future in futures {
                let deployment_result = future.await;
                services.insert(deployment_result.service.name.clone(), deployment_result);
            }
        }

        services
    }

    pub async fn update_services(&self, clients: &Clients, services: Vec<Service>) {
        let mut futures = Vec::new();

        for service in services {
            // Save the service to the primary instance and get the URL for on-chain
            let service_url = DeployService::save_service(&clients.cli_ctx, &service)
                .await
                .unwrap();

            let service_manager_instance = self.lookup.get(&service.name).unwrap();
            // For Stellar tests, pre-resolve the shared stack so the future
            // body can take the URI lock. The lock covers both the on-chain
            // `set-project-spec-repo` and the per-instance
            // `wait_for_service_update` so two tests can't stomp the single
            // project_root URI between set and convergence.
            let stellar_stack = match service_manager_instance {
                AnyServiceManagerInstance::Stellar { scheme, .. } => Some(
                    self.stellar_stacks
                        .get(scheme)
                        .expect("missing shared stellar stack for scheme")
                        .clone(),
                ),
                _ => None,
            };
            let http_clients = clients.http_clients.clone();
            futures.push(async move {
                // Hold the per-scheme URI lock for the entire Stellar future.
                // For non-Stellar tests this is `None` and costs nothing.
                let _stellar_guard = if let Some(stack) = &stellar_stack {
                    Some(stack.uri_lock.clone().lock_owned().await)
                } else {
                    None
                };
                match service_manager_instance {
                    AnyServiceManagerInstance::Evm { manager, .. } => {
                        // wait for the trigger streams to be ready on all instances before we update the service uri
                        for (idx, http_client) in http_clients.iter().enumerate() {
                            tracing::info!(
                                "Waiting for trigger streams on instance {} for service {}",
                                idx,
                                service.name
                            );
                            wait_for_evm_trigger_streams_to_finalize(
                                http_client,
                                Some(service.manager.clone()),
                            )
                            .await;
                        }
                        manager.set_service_uri(service_url).await.unwrap();
                    }
                    AnyServiceManagerInstance::Cosmos { manager, .. } => {
                        manager.set_service_uri(&service_url).await.unwrap();
                    }
                    AnyServiceManagerInstance::Stellar { .. } => {
                        let stack = stellar_stack
                            .as_ref()
                            .expect("Stellar branch entered without a resolved stack");
                        stack
                            .middleware
                            .set_service_uri(&stack.manager.deploy_file_path, &service_url)
                            .await
                            .unwrap();
                    }
                }

                // No more `dev_add_service_direct` shortcut: we just set
                // the URI on chain (above) and rely on each WarpDrive
                // instance picking up the change via its native detection
                // path — EVM `ServiceURIUpdated` log, Cosmos wasm event,
                // Stellar service-URI poller. The wait below is what
                // actually blocks until propagation completes.

                // Wait for service update on all WarpDrive instances.
                // Stellar manager? Allow extra propagation time — testnet
                // ledger close + RPC indexing delay can comfortably push
                // past 30s, especially under load. EVM/Cosmos use anvil /
                // local cosmwasm so the default suffices.
                let wait_timeout = match &service.manager {
                    warpdrive_types::ServiceManager::Stellar { .. } => {
                        Some(std::time::Duration::from_secs(120))
                    }
                    _ => None,
                };
                for (idx, http_client) in http_clients.iter().enumerate() {
                    tracing::info!(
                        "Waiting for service update on instance {} for service {}",
                        idx,
                        service.name
                    );
                    http_client
                        .wait_for_service_update(&service, wait_timeout)
                        .await
                        .unwrap();
                    tracing::info!(
                        "Service update complete on instance {} for service {}",
                        idx,
                        service.name
                    );
                }

                // Debug: Log trigger streams status
                let http_client = clients
                    .http_clients
                    .first()
                    .expect("Expected at least one WarpDrive HTTP client");
                match http_client.get_trigger_streams_info().await {
                    Ok(streams) => {
                        tracing::info!(
                            "Trigger streams finalized={}, chains={:?}",
                            streams.finalized(),
                            streams.chains
                        );
                    }
                    Err(e) => {
                        tracing::warn!("Failed to get trigger streams info: {:?}", e);
                    }
                }

                // doesn't hurt to wait again for rpcs at least in case trigger contract changed
                if let AnyServiceManagerInstance::Evm { .. } = service_manager_instance {
                    for (idx, http_client) in http_clients.iter().enumerate() {
                        tracing::info!(
                            "Final trigger stream wait on instance {} for service {}",
                            idx,
                            service.name
                        );
                        wait_for_evm_trigger_streams_to_finalize(http_client, None).await;
                    }
                }
            });
        }

        if self.configs.middleware_concurrency {
            let mut futures_unordered = FuturesUnordered::from_iter(futures);
            while (futures_unordered.next().await).is_some() {}
        } else {
            for future in futures {
                future.await;
            }
        }
    }
}
