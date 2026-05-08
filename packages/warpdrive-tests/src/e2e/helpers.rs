use alloy_primitives::U256;
use alloy_provider::{ext::AnvilApi, Provider};
use alloy_sol_types::SolEvent;
use anyhow::{anyhow, Context, Result};
use deadpool::managed::Object;
use layer_climb::pool::SigningClientPoolManager;
use layer_climb::prelude::CosmosAddr;
use std::{collections::BTreeMap, num::NonZero, sync::Arc, time::Duration};
use utils::evm_client::AnyNonceManager;
use utils::test_utils::middleware::stellar::StellarContracts;
use utils::{
    config::WARPDRIVE_ENV_PREFIX, evm_client::EvmSigningClient, filesystem::workspace_path,
};
use uuid::Uuid;
use warpdrive_cli::clients::HttpClient;

use warpdrive_types::{
    AllowedHostPermission, ByteArray, ChainKey, Component, DevTriggerStreamSubscriptionKind,
    Permissions, Service, ServiceManager, ServiceStatus, SignatureKind, Submit, Trigger, Workflow,
};

use crate::deployment::{ServiceDeployment, WorkflowDeployment};

use crate::e2e::test_definition::CosmosSubmitDefinition;
use crate::e2e::test_registry::CosmosContractDefinition;
use crate::example_cosmos_client::SimpleCosmosSubmitClient;
use crate::{
    e2e::{
        clients::Clients,
        components::ComponentSources,
        config::BLOCK_INTERVAL,
        test_definition::{
            AggregatorDefinition, ChangeServiceDefinition, ComponentDefinition, SubmitDefinition,
            TestDefinition, TriggerDefinition,
        },
    },
    example_cosmos_client::SimpleCosmosTriggerClient,
    example_evm_client::{
        example_submit::ISimpleSubmit::SignedData, example_trigger::SimpleTrigger, LogSpamClient,
        SimpleEvmSubmitClient, TriggerId,
    },
    example_stellar_client::SimpleStellarTriggerClient,
};

use super::{
    test_definition::{
        CosmosTriggerDefinition, EvmTriggerDefinition, StellarTriggerDefinition, WorkflowDefinition,
    },
    test_registry::CosmosCodeMap,
};

/// Helper function to deploy a service for a test
pub async fn create_service_for_test(
    test: &TestDefinition,
    clients: &Clients,
    component_sources: &ComponentSources,
    service_manager: ServiceManager,
    cosmos_code_map: CosmosCodeMap,
    stellar_service_manager: Option<utils::test_utils::middleware::stellar::StellarServiceManager>,
) -> ServiceDeployment {
    tracing::info!("Deploying service for test: {}", test.name);
    tracing::info!("Service manager: {:?}", service_manager);
    tracing::info!(
        "[{}] Deploying service manager on chain {}",
        test.name,
        service_manager.chain()
    );

    // No need to load the actual service, it was a placeholder
    let mut service = Service {
        name: test.name.clone(),
        workflows: BTreeMap::new(),
        status: ServiceStatus::Active,
        manager: service_manager,
    };

    let mut submission_handlers = BTreeMap::new();

    for (workflow_id, workflow_definition) in test.workflows.iter() {
        let deployment_result = deploy_workflow(
            &test.name,
            workflow_definition,
            service.manager.clone(),
            clients,
            component_sources,
            cosmos_code_map.clone(),
            stellar_service_manager.as_ref(),
        )
        .await;

        service
            .workflows
            .insert(workflow_id.clone(), deployment_result.workflow);
        submission_handlers.insert(workflow_id.clone(), deployment_result.submission_handler);
    }

    ServiceDeployment {
        service,
        submission_handlers,
    }
}

fn deploy_component(
    component_sources: &ComponentSources,
    component_definition: &ComponentDefinition,
    config_vars: BTreeMap<String, String>,
    env_vars: BTreeMap<String, String>,
) -> Component {
    // Create components from test definition
    let component_source = component_sources
        .lookup
        .get(&component_definition.name)
        .unwrap()
        .clone();

    let mut component = Component::new(component_source);
    component.permissions = Permissions {
        allowed_http_hosts: AllowedHostPermission::All,
        file_system: true,
        raw_sockets: true,
        dns_resolution: true,
    };
    component.config = config_vars;
    // Set env_keys to the actual prefixed env var names that will be read by the component
    component.env_keys = env_vars
        .keys()
        .map(|k| format!("{}_{}", WARPDRIVE_ENV_PREFIX, k))
        .collect();

    for (k, v) in env_vars.iter() {
        // NOTE: we should avoid collisions here
        std::env::set_var(format!("{}_{}", WARPDRIVE_ENV_PREFIX, k), v);
    }

    component
}

async fn deploy_workflow(
    test_name: &str,
    workflow_definition: &WorkflowDefinition,
    service_manager: ServiceManager,
    clients: &Clients,
    component_sources: &ComponentSources,
    cosmos_code_map: CosmosCodeMap,
    stellar_service_manager: Option<&utils::test_utils::middleware::stellar::StellarServiceManager>,
) -> WorkflowDeployment {
    let component = deploy_component(
        component_sources,
        &workflow_definition.component,
        Default::default(),
        Default::default(),
    );

    tracing::info!("[{}] Creating submit from config", test_name);

    let submission_contract = deploy_submit_contract(
        clients,
        cosmos_code_map.clone(),
        service_manager,
        stellar_service_manager,
    )
    .await
    .unwrap();

    let submit = create_submit_from_config(
        &workflow_definition.submit,
        &submission_contract,
        Some(component_sources),
    )
    .await
    .unwrap();

    tracing::info!("[{}] Creating trigger from config", test_name);
    // Create the trigger based on test configuration
    let trigger = create_trigger_from_config(
        workflow_definition.trigger.clone(),
        clients,
        cosmos_code_map.clone(),
        Some(workflow_definition),
    )
    .await;

    // Create service workflows
    WorkflowDeployment {
        workflow: Workflow {
            trigger: trigger.clone(), // Clone for possible use in multi-trigger service
            component,
            submit: submit.clone(),
        },
        submission_handler: submission_contract,
    }
}

/// Create a trigger based on test configuration
pub async fn create_trigger_from_config(
    trigger_definition: TriggerDefinition,
    clients: &Clients,
    cosmos_code_map: CosmosCodeMap,
    _workflow_definition: Option<&WorkflowDefinition>,
) -> Trigger {
    match trigger_definition {
        TriggerDefinition::NewEvmContract(evm_trigger_definition) => match evm_trigger_definition {
            EvmTriggerDefinition::SimpleContractEvent { chain } => {
                let client = clients.get_evm_client(&chain);

                // Deploy a new EVM trigger contract
                tracing::info!("Deploying EVM trigger contract on chain {}", chain);
                let contract = SimpleTrigger::deploy(client.provider.clone())
                    .await
                    .unwrap();
                let address = *contract.address();

                // Get the event hash
                let event_hash =
                    *crate::example_evm_client::example_trigger::NewTrigger::SIGNATURE_HASH;

                Trigger::EvmContractEvent {
                    chain: chain.clone(),
                    address,
                    event_hash: ByteArray::new(event_hash),
                }
            }
        },
        TriggerDefinition::NewCosmosContract(cosmos_trigger_definition) => {
            match cosmos_trigger_definition.clone() {
                CosmosTriggerDefinition::SimpleContractEvent { ref chain } => {
                    let client = clients.get_cosmos_client(chain).await;

                    // Get the code ID with better error handling
                    tracing::info!("Getting cosmos code ID for chain {}", chain);
                    let code_id = get_cosmos_code_id(
                        clients,
                        &CosmosContractDefinition::Trigger(cosmos_trigger_definition),
                        cosmos_code_map,
                    )
                    .await;

                    tracing::info!("Using cosmos code ID: {} for chain {}", code_id, chain);

                    // Deploy a new Cosmos trigger contract with better error handling
                    let contract_name = format!("simple_trigger_{}", Uuid::now_v7());
                    tracing::info!(
                        "Instantiating new contract '{}' with code ID {} on chain {}",
                        contract_name,
                        code_id,
                        chain
                    );

                    let contract =
                        SimpleCosmosTriggerClient::new_code_id(client, code_id, &contract_name)
                            .await
                            .unwrap();

                    tracing::info!(
                        "Successfully deployed cosmos contract at address: {}",
                        contract.contract_address
                    );

                    Trigger::CosmosContractEvent {
                        chain: chain.clone(),
                        address: contract.contract_address.try_into().unwrap(),
                        event_type: cw_warpdrive_trigger_api::simple::PushMessageEvent::EVENT_TYPE
                            .to_string(),
                    }
                }
            }
        }
        TriggerDefinition::NewStellarContract(stellar_trigger_definition) => {
            match stellar_trigger_definition {
                StellarTriggerDefinition::SimpleContractEvent { chain } => {
                    tracing::info!("Deploying Stellar trigger contract on chain {}", chain);
                    let client = SimpleStellarTriggerClient::new(chain.clone());
                    let contract_id = client.deploy().await.unwrap();

                    Trigger::StellarContractEvent {
                        chain,
                        contract_id,
                        topic_segments: vec![
                            warpdrive_types::StellarTopicSegment::Exact(
                                stellar_xdr::curr::ScVal::Symbol("trigger".try_into().unwrap()),
                            ),
                            warpdrive_types::StellarTopicSegment::Wildcard,
                        ],
                    }
                }
            }
        }
        TriggerDefinition::BlockInterval { chain, start_stop } => match start_stop {
            false => Trigger::BlockInterval {
                chain,
                n_blocks: BLOCK_INTERVAL,
                start_block: None,
                end_block: None,
            },
            true => {
                let current_block = if clients.evm_clients.contains_key(&chain) {
                    let client = clients.get_evm_client(&chain);
                    client.provider.get_block_number().await.unwrap()
                } else if clients.cosmos_client_pools.contains_key(&chain) {
                    let client = clients.get_cosmos_client(&chain).await;
                    client.querier.block_height().await.unwrap()
                } else {
                    panic!("Chain is not configured: {}", chain)
                };

                let current_block = NonZero::new(current_block).unwrap();

                Trigger::BlockInterval {
                    chain,
                    n_blocks: BLOCK_INTERVAL,
                    start_block: Some(current_block),
                    end_block: Some(current_block),
                }
            }
        },
        TriggerDefinition::Existing(trigger) => trigger.clone(),
    }
}

/// Create a submit based on test configuration
pub async fn create_submit_from_config(
    submit_config: &SubmitDefinition,
    submission_contract: &warpdrive_types::ChainAddress,
    component_sources: Option<&ComponentSources>,
) -> Result<Submit> {
    match submit_config {
        SubmitDefinition::Aggregator(aggregator) => match aggregator {
            AggregatorDefinition::ComponentBasedAggregator {
                component: component_def,
                ..
            } => {
                let sources = component_sources.ok_or_else(|| {
                    anyhow!("ComponentBasedAggregator requires component_sources")
                })?;

                let mut config_vars = BTreeMap::new();
                let mut env_vars = BTreeMap::new();

                for (hardcoded_key, hardcoded_value) in &component_def.configs_to_add.hardcoded {
                    config_vars.insert(hardcoded_key.clone(), hardcoded_value.clone());
                }

                for (env_key, env_value) in &component_def.env_vars_to_add {
                    env_vars.insert(env_key.clone(), env_value.clone());
                }

                if component_def.configs_to_add.service_handler {
                    // For EVM/Cosmos this is a layer-climb-shaped string; for
                    // Stellar it's the strkey "C..." form. The component just
                    // sees it as a string and uses it verbatim when wrapping
                    // its output in an envelope/encoding for the matching
                    // chain.
                    let handler_str = match submission_contract {
                        warpdrive_types::ChainAddress::Evm(addr) => addr.to_string(),
                        warpdrive_types::ChainAddress::Cosmos(addr) => addr.to_string(),
                        warpdrive_types::ChainAddress::Stellar(c) => format!("{c}"),
                    };
                    config_vars.insert("service_handler".to_string(), handler_str);
                }

                let component = deploy_component(sources, component_def, config_vars, env_vars);

                Ok(Submit::Aggregator {
                    component: Box::new(component),
                    signature_kind: SignatureKind::evm_default(),
                })
            }
        },
    }
}

/// Deploy a per-test "mock submit" contract and return its address as a
/// chain-agnostic `ChainAddress`. The address gets stuffed into the
/// aggregator component's `service_handler` config so the component can
/// target it.
pub async fn deploy_submit_contract(
    clients: &Clients,
    cosmos_code_map: CosmosCodeMap,
    service_manager: ServiceManager,
    stellar_service_manager: Option<&utils::test_utils::middleware::stellar::StellarServiceManager>,
) -> Result<warpdrive_types::ChainAddress> {
    match service_manager {
        ServiceManager::Cosmos { chain, address } => {
            let code_id = get_cosmos_code_id(
                clients,
                &CosmosContractDefinition::Submit(CosmosSubmitDefinition::MockServiceHandler {
                    chain: chain.clone(),
                }),
                cosmos_code_map,
            )
            .await;

            let client = clients.get_cosmos_client(&chain).await;
            let contract_client =
                crate::example_cosmos_client::SimpleCosmosSubmitClient::new_code_id(
                    client,
                    code_id,
                    &address,
                    "Mock service handler",
                )
                .await?;

            Ok(warpdrive_types::ChainAddress::from(
                contract_client.contract_address,
            ))
        }
        ServiceManager::Evm { chain, address } => {
            let evm_client = clients.get_evm_client(&chain);

            tracing::info!(
                "Deploying submit contract on chain {} with service manager: {}",
                chain,
                address
            );

            let result = crate::example_evm_client::example_submit::SimpleSubmit::deploy(
                evm_client.provider.clone(),
                address,
            )
            .await
            .context("Failed to deploy submit contract")?;

            let address = *result.address();
            tracing::info!("Submit contract deployed at address: {}", address);

            Ok(warpdrive_types::ChainAddress::from(address))
        }
        ServiceManager::Stellar { chain, address: _ } => {
            // The mock submit IS a service handler — it implements
            // `verify_eth(envelope, sig_data)` directly and delegates
            // signature validation to the test stack's
            // `secp256k1_verification` contract. Per-test isolation: each
            // test gets its own handler bound to its own verification
            // contract.
            let stellar_sm = stellar_service_manager.ok_or_else(|| {
                anyhow!("StellarServiceManager required to deploy stellar submit contract")
            })?;
            match &stellar_sm.contracts {
                StellarContracts::SecpContracts(c) => {
                    let verification_contract = format!("{}", c.secp256k1_verification);
                    tracing::info!(
                        "Deploying Stellar mock submit handler on chain {} bound to verification {}",
                        chain,
                        verification_contract
                    );
                    let client =
                        crate::example_stellar_client::SimpleStellarSubmitClient::new(chain);
                    let contract_id = client.deploy(&verification_contract).await?;
                    tracing::info!("Stellar mock submit handler deployed at {}", contract_id);
                    let parsed = stellar_strkey::Contract::from_string(&contract_id)
                        .map_err(|e| anyhow!("invalid stellar contract id from deploy: {e:?}"))?;
                    Ok(warpdrive_types::ChainAddress::Stellar(parsed))
                }
                StellarContracts::EdContracts(c) => {
                    let verification_contract = format!("{}", c.ed25519_verification);
                    tracing::info!(
                        "Deploying Stellar mock submit handler on chain {} bound to verification {}",
                        chain,
                        verification_contract
                    );
                    todo!("Implement the native stellar branch");
                }
            }
        }
    }
}

/// Deploy LogSpam contract and return its address
pub async fn deploy_log_spam_contract(
    clients: &Clients,
    chain: &ChainKey,
) -> Result<alloy_primitives::Address> {
    let evm_client = clients.get_evm_client(chain);

    tracing::info!("Deploying LogSpam contract on chain {}", chain);

    let address = LogSpamClient::deploy(evm_client.provider.clone())
        .await
        .context("Failed to deploy LogSpam contract")?;

    tracing::info!("LogSpam contract deployed at address: {}", address);

    Ok(address)
}

/// Deploy submit contract and create a Submit from it
pub async fn get_cosmos_code_id(
    clients: &Clients,
    cosmos_contract_definition: &CosmosContractDefinition,
    cosmos_code_map: CosmosCodeMap,
) -> u64 {
    // Get or insert the entry
    let entry = cosmos_code_map
        .entry(cosmos_contract_definition.clone())
        .or_insert_with(|| Arc::new(tokio::sync::RwLock::new(None)))
        .clone();

    // try to read (non-blocking for other readers)
    {
        let read_guard = entry.read().await;
        if let Some(code_id) = *read_guard {
            return code_id;
        }
    }

    // cache miss, acquire write lock for upload
    let mut write_guard = entry.write().await;

    // check cache after acquiring write lock, if another thread already uploaded
    if let Some(code_id) = *write_guard {
        return code_id;
    }

    // Upload since not cached
    let (chain, cosmos_bytecode) = match cosmos_contract_definition {
        CosmosContractDefinition::Trigger(CosmosTriggerDefinition::SimpleContractEvent {
            chain,
        }) => {
            let wasm_path = workspace_path()
                .join("examples")
                .join("build")
                .join("contracts")
                .join("cw_warpdrive_trigger_simple.wasm");

            if !wasm_path.exists() {
                panic!(
                    "Cosmos contract WASM file not found at: {}",
                    wasm_path.display()
                );
            }

            (chain, tokio::fs::read(&wasm_path).await.unwrap())
        }
        CosmosContractDefinition::Submit(CosmosSubmitDefinition::MockServiceHandler { chain }) => {
            let wasm_path = workspace_path()
                .join("examples")
                .join("build")
                .join("contracts")
                .join("cw_warpdrive_mock_service_handler.wasm");

            if !wasm_path.exists() {
                panic!(
                    "Cosmos contract WASM file not found at: {}",
                    wasm_path.display()
                );
            }

            (chain, tokio::fs::read(&wasm_path).await.unwrap())
        }
    };

    tracing::info!(
        "Uploading cosmos wasm byte code ({} bytes) to chain {}",
        cosmos_bytecode.len(),
        chain
    );

    let client = clients.get_cosmos_client(chain).await;

    let (code_id, _) = client
        .contract_upload_file(cosmos_bytecode, None)
        .await
        .unwrap();

    tracing::info!(
        "Successfully uploaded WASM bytecode to chain {}, code_id: {}",
        chain,
        code_id
    );

    // Cache result and return
    *write_guard = Some(code_id);
    code_id
}

/// Simulate a re-org by reverting to a previous block and mining new blocks
pub async fn simulate_anvil_reorg(
    evm_client: &EvmSigningClient,
    reorg_snapshot: U256,
) -> Result<()> {
    // Revert to the specified block using Anvil's revert RPC
    evm_client.provider.anvil_revert(reorg_snapshot).await?;

    // Update nonce
    if let AnyNonceManager::Fast(fast_nonce_manager) = &evm_client.nonce_manager {
        fast_nonce_manager
            .set_current_nonce(&evm_client.provider)
            .await
            .unwrap();
    }

    // Mine new blocks to simulate chain reorganization
    evm_client.provider.evm_mine(None).await?;
    Ok(())
}

pub async fn evm_wait_for_task_to_land(
    evm_submit_client: EvmSigningClient,
    address: alloy_primitives::Address,
    trigger_id: TriggerId,
    submit_start_block: u64,
    timeout: Duration,
) -> Result<SignedData> {
    let submit_client = SimpleEvmSubmitClient::new(evm_submit_client, address);

    tokio::time::timeout(timeout, async move {
        loop {
            let current_block = submit_client
                .evm_client
                .provider
                .get_block_number()
                .await
                .map_err(|e| anyhow!("Failed to get block number: {e}"))?;

            if current_block <= submit_start_block {
                submit_client.evm_client.provider.evm_mine(None).await?;
            }

            if submit_client.trigger_validated(trigger_id).await {
                return submit_client
                    .signed_data(trigger_id)
                    .await
                    .map_err(|e| anyhow!("Failed to get signed data: {e}"));
            }

            tracing::debug!("Waiting for task response on trigger {}", trigger_id);
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("Timeout when waiting for task to land"))?
}

pub async fn stellar_wait_for_task_to_land(
    chain: warpdrive_types::ChainKey,
    contract_id: stellar_strkey::Contract,
    trigger_id: TriggerId,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let submit_client = crate::example_stellar_client::SimpleStellarSubmitClient::new(chain);
    let contract_id_str = format!("{contract_id}");

    tokio::time::timeout(timeout, async move {
        loop {
            if submit_client
                .is_valid_trigger_id(&contract_id_str, trigger_id)
                .await
                .unwrap_or(false)
            {
                return submit_client
                    .get_data(&contract_id_str, trigger_id)
                    .await
                    .map_err(|e| anyhow!("Failed to get stellar submit data: {e}"));
            }

            tracing::debug!(
                "Waiting for stellar task response on trigger {}",
                trigger_id
            );
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("Timeout when waiting for stellar task to land"))?
}

pub async fn cosmos_wait_for_task_to_land(
    cosmos_submit_client: Object<SigningClientPoolManager>,
    address: CosmosAddr,
    trigger_id: TriggerId,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let submit_client = SimpleCosmosSubmitClient::new(cosmos_submit_client, address.into());

    let trigger_id = trigger_id.u64();
    tokio::time::timeout(timeout, async move {
        loop {
            if submit_client
                .trigger_validated(trigger_id)
                .await
                .unwrap_or(false)
            {
                return submit_client
                    .trigger_message(trigger_id)
                    .await
                    .map_err(|e| anyhow!("Failed to get signed data: {e}"));
            }

            tracing::debug!("Waiting for task response on trigger {}", trigger_id);
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("Timeout when waiting for task to land"))?
}

/// Helper function to deploy a service for a test
pub async fn change_service_for_test(
    service: &mut Service,
    change_service: ChangeServiceDefinition,
    clients: &Clients,
    component_sources: &ComponentSources,
    cosmos_code_map: CosmosCodeMap,
    stellar_service_manager: Option<&utils::test_utils::middleware::stellar::StellarServiceManager>,
) {
    match change_service {
        ChangeServiceDefinition::Component {
            workflow_id,
            component: component_definition,
        } => {
            let component = deploy_component(
                component_sources,
                &component_definition,
                Default::default(),
                Default::default(),
            );
            let workflow = service
                .workflows
                .get_mut(&workflow_id)
                .expect("Workflow not found in service");

            workflow.component = component;
        }
        ChangeServiceDefinition::AddWorkflow {
            workflow_id,
            workflow,
        } => {
            let deployed_workflow = deploy_workflow(
                &workflow_id,
                &workflow,
                service.manager.clone(),
                clients,
                component_sources,
                cosmos_code_map,
                stellar_service_manager,
            )
            .await;

            service
                .workflows
                .insert(workflow_id.clone(), deployed_workflow.workflow);
        }
    }
}

pub async fn wait_for_evm_trigger_streams_to_finalize(
    client: &HttpClient,
    service_manager: Option<ServiceManager>,
) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            tracing::info!("Getting trigger stream info...");
            let info = client.get_trigger_streams_info().await.unwrap();

            if info.finalized() {
                if let Some(service_manager) = &service_manager {
                    match service_manager {
                        ServiceManager::Evm { chain, address } => {
                            let address = ByteArray::new(address.into_array());
                            if info.chains.iter().any(|(key, value)| {
                                key == chain
                                    && value.active_subscriptions.values().any(|kind| match kind {
                                        DevTriggerStreamSubscriptionKind::Logs {
                                            addresses,
                                            ..
                                        } => addresses.contains(&address),
                                        _ => false,
                                    })
                            }) {
                                break;
                            }
                        }
                        ServiceManager::Cosmos { .. } => {
                            unreachable!("This is only meant for EVM");
                        }
                        ServiceManager::Stellar { .. } => {
                            unreachable!("This is only meant for EVM");
                        }
                    }
                } else if info.any_active_subscriptions() {
                    break;
                }
            } else {
                tracing::warn!("Still waiting for trigger streams to finalize");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
}
