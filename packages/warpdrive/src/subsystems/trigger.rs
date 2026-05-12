pub mod error;
pub mod lookup;
pub mod schedulers;
pub mod streams;

use crate::{
    config::Config,
    dispatcher::DispatcherCommand,
    services::Services,
    subsystems::trigger::streams::{
        cosmos_stream::StreamTriggerCosmosContractEvent,
        evm_stream::client::{EvmTriggerStreams, EvmTriggerStreamsController},
        local_command_stream,
        stellar_stream::{
            channels::{StellarChannelReceivers, StellarChannelSenders, StellarChannels},
            controller::StellarStreamController,
            poller::{start_stellar_event_poller, start_stellar_ledger_poller},
            start_stellar_ledger_stream,
        },
    },
    tracing_service_info, AppContext,
};
use alloy_sol_types::SolEvent;
use anyhow::Result;
use error::TriggerError;
use futures::{stream::SelectAll, StreamExt};
use iri_string::types::UriString;
use layer_climb::prelude::*;
use lookup::{LookupId, LookupMaps};
use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU64,
    sync::Arc,
    time::Duration,
};
use streams::stellar_stream::filters::StellarRpcId;
use streams::stellar_stream::start_stellar_event_stream;
use streams::{cosmos_stream, cron_stream, evm_stream, MultiplexedStream, StreamTriggers};
use tracing::instrument;
use utils::telemetry::TriggerMetrics;
use warpdrive_types::{
    contracts::cosmwasm::service_manager::event::WavsServiceUriUpdatedEvent, AnyChainConfig,
    ByteArray, ChainConfigs, ChainKey, IWarpDriveServiceManager, ServiceId, Trigger, TriggerAction,
    TriggerConfig, TriggerData,
};

#[derive(Debug)]
pub enum TriggerCommand {
    Kill,
    StartListeningChain {
        chain: ChainKey,
    },
    StartListeningCron,
    WatchEvmBlocks {
        chain: ChainKey,
    },
    WatchEvmContractEvents {
        chain: ChainKey,
        addresses: Vec<alloy_primitives::Address>,
        event_hashes: Vec<alloy_primitives::B256>,
    },
    WatchStellarBlocks {
        chain: ChainKey,
    },
    /// Subscribe to a Stellar `project_root`'s `UpdatedSpecRepo` contract
    /// event for service-URI change detection. Fire-and-forget — the
    /// watcher waits for the chain's stellar controller to come up and
    /// then registers the filter, mirroring the EVM
    /// `WatchEvmContractEvents` async pattern. The `service_id` is
    /// recorded alongside the resulting `StellarRpcId` so a later
    /// `UnwatchStellarServiceUri` can drop the precise filter handle
    /// without touching sibling subscriptions.
    WatchStellarServiceUri {
        service_id: ServiceId,
        chain: ChainKey,
        project_root: stellar_strkey::Contract,
    },
    /// Drop the `UpdatedSpecRepo` filter previously registered for
    /// `service_id`. Sent from `TriggerManager::remove_service`; the
    /// watcher resolves the recorded `StellarRpcId` and calls
    /// `remove_filter_by_id` so other services that happen to share a
    /// project root (impossible in practice but guarded structurally)
    /// keep their subscriptions.
    UnwatchStellarServiceUri {
        service_id: ServiceId,
    },
    StartListeningAtProto,
    ManualTrigger(Box<TriggerAction>),
}

impl TriggerCommand {
    pub fn map(trigger_config: &TriggerConfig, chain_configs: &ChainConfigs) -> Vec<Self> {
        match &trigger_config.trigger {
            Trigger::Cron { .. } => vec![Self::StartListeningCron],
            Trigger::EvmContractEvent {
                chain,
                address,
                event_hash,
            } => {
                vec![
                    Self::StartListeningChain {
                        chain: chain.clone(),
                    },
                    Self::WatchEvmContractEvents {
                        chain: chain.clone(),
                        addresses: vec![*address],
                        event_hashes: vec![event_hash.into_inner().into()],
                    },
                ]
            }
            Trigger::CosmosContractEvent { chain, .. } => {
                vec![Self::StartListeningChain {
                    chain: chain.clone(),
                }]
            }
            Trigger::StellarContractEvent {
                chain,
                contract_id: _,
                topic_segments: _,
            } => {
                vec![
                    Self::StartListeningChain {
                        chain: chain.clone(),
                    },
                    // There is no WatchStellarContractEvents
                    // Because we needed the rpc_id earlier on
                ]
            }
            Trigger::BlockInterval { chain, .. } => match chain_configs.get_chain(chain) {
                Some(chain_config) => match chain_config {
                    AnyChainConfig::Evm(_) => {
                        vec![
                            Self::StartListeningChain {
                                chain: chain.clone(),
                            },
                            Self::WatchEvmBlocks {
                                chain: chain.clone(),
                            },
                        ]
                    }
                    AnyChainConfig::Cosmos(_) => {
                        vec![Self::StartListeningChain {
                            chain: chain.clone(),
                        }]
                    }
                    AnyChainConfig::Stellar(_) => {
                        vec![
                            Self::StartListeningChain {
                                chain: chain.clone(),
                            },
                            Self::WatchStellarBlocks {
                                chain: chain.clone(),
                            },
                        ]
                    }
                },
                None => {
                    tracing::warn!("Block interval set for non-existant chain-config: {chain}");
                    Vec::new()
                }
            },
            Trigger::AtProtoEvent { .. } => {
                vec![Self::StartListeningAtProto]
            }
            Trigger::Manual => Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamStartState {
    Waiting,
    Connecting,
    Connected,
}

#[derive(Clone)]
pub struct TriggerManager {
    pub chain_configs: Arc<std::sync::RwLock<ChainConfigs>>,
    pub command_sender: tokio::sync::mpsc::UnboundedSender<TriggerCommand>,
    subsystem_to_dispatcher_tx: crossbeam::channel::Sender<DispatcherCommand>,
    command_receiver:
        Arc<std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<TriggerCommand>>>>,
    lookup_maps: Arc<LookupMaps>,
    metrics: TriggerMetrics,
    #[cfg(feature = "dev")]
    pub disable_networking: bool,
    pub services: Services,
    pub evm_controllers: Arc<std::sync::RwLock<HashMap<ChainKey, EvmTriggerStreamsController>>>,
    pub stellar_controllers: Arc<std::sync::RwLock<HashMap<ChainKey, StellarStreamController>>>,
    /// Tracks the `UpdatedSpecRepo` event-filter handle registered for each
    /// Stellar-managed service so `remove_service` can drop the exact filter.
    /// Keyed by `ServiceId` (one URI subscription per service); the value is
    /// the chain plus the `StellarRpcId` returned by `add_filter` at
    /// registration time.
    stellar_uri_subscriptions: Arc<std::sync::RwLock<HashMap<ServiceId, (ChainKey, StellarRpcId)>>>,
    pub config: Config,
}

impl TriggerManager {
    #[allow(clippy::new_without_default)]
    #[instrument(skip(services), fields(subsys = "TriggerManager"))]
    pub fn new(
        config: &Config,
        metrics: TriggerMetrics,
        services: Services,
        subsystem_to_dispatcher_tx: crossbeam::channel::Sender<DispatcherCommand>,
    ) -> Result<Self, TriggerError> {
        let (command_sender, command_receiver) = tokio::sync::mpsc::unbounded_channel();

        Ok(Self {
            chain_configs: config.chains.clone(),
            lookup_maps: Arc::new(LookupMaps::new(services.clone(), metrics.clone())),
            subsystem_to_dispatcher_tx,
            command_sender,
            command_receiver: Arc::new(std::sync::Mutex::new(Some(command_receiver))),
            metrics,
            #[cfg(feature = "dev")]
            disable_networking: config.disable_trigger_networking,
            services,
            evm_controllers: Arc::new(std::sync::RwLock::new(HashMap::new())),
            stellar_controllers: Arc::new(std::sync::RwLock::new(HashMap::new())),
            stellar_uri_subscriptions: Arc::new(std::sync::RwLock::new(HashMap::new())),
            config: config.clone(),
        })
    }

    #[instrument(skip(self, service), fields(subsys = "TriggerManager"))]
    pub async fn add_service(
        &self,
        service: &warpdrive_types::Service,
    ) -> Result<(), TriggerError> {
        // The mechanics of adding a trigger are that we:

        // 1. Setup all the records needed to track the trigger in various "lookup" maps.
        // 2a. If the trigger needs some kind of stream to kick it off, we need to create that stream.
        // 2b. Actual stream-creation happens by way of a "local command" so that everything is handled in `start_watcher` (helps with lifetime issues).
        //
        // It doesn't really matter what order the multiplexed streams are polled in, a trigger simply
        // will not be fired until the stream that kicks it off is polled (i.e. this definitively happens _after_ the stream is created).

        let chain_configs = self.chain_configs.read().unwrap().clone();
        let mut workflow_commands = Vec::new();
        let mut stellar_chains_to_wait_for = HashSet::new();

        for (id, workflow) in &service.workflows {
            let config = TriggerConfig {
                service_id: service.id(),
                workflow_id: id.clone(),
                trigger: workflow.trigger.clone(),
            };

            for command in TriggerCommand::map(&config, &chain_configs) {
                if let TriggerCommand::StartListeningChain { chain } = &command {
                    if matches!(
                        chain_configs.get_chain(chain),
                        Some(AnyChainConfig::Stellar(_))
                    ) {
                        stellar_chains_to_wait_for.insert(chain.clone());
                    }
                }
                workflow_commands.push(command);
            }
        }

        // Ensure the service manager's chain is being listened to for service change events
        // This is needed even if the service has no workflows, so service URI changes can be detected
        self.command_sender
            .send(TriggerCommand::StartListeningChain {
                chain: service.manager.chain().clone(),
            })?;

        for chain in &stellar_chains_to_wait_for {
            self.command_sender
                .send(TriggerCommand::StartListeningChain {
                    chain: chain.clone(),
                })?;
        }

        // Block until each Stellar chain's controller has been registered by the
        // watcher task. The healthy path resolves in a handful of poll ticks
        // (~50ms each); the 10s ceiling is a worst-case deadline for when the
        // chain runner has failed to come up at all. Any service that hits the
        // ceiling will fail to register, but at that point the whole stack is
        // already non-functional for that chain — so the long wait only delays
        // an inevitable error rather than masking a recoverable one.
        for chain in stellar_chains_to_wait_for {
            let start = std::time::Instant::now();
            while !self
                .stellar_controllers
                .read()
                .unwrap()
                .contains_key(&chain)
            {
                if start.elapsed() > Duration::from_secs(10) {
                    return Err(TriggerError::StellarMissingClient(chain));
                }

                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }

        self.lookup_maps
            .add_service(service, &self.stellar_controllers)?;

        match service.manager.clone() {
            warpdrive_types::ServiceManager::Evm { chain, address } => {
                self.command_sender
                    .send(TriggerCommand::WatchEvmContractEvents {
                        chain,
                        addresses: vec![address],
                        event_hashes: vec![
                            IWarpDriveServiceManager::ServiceURIUpdated::SIGNATURE_HASH,
                        ],
                    })?;
            }
            warpdrive_types::ServiceManager::Cosmos { .. } => {
                /* Nothing to do, Cosmos consumes all events, service URI changes will be handled */
            }
            warpdrive_types::ServiceManager::Stellar { chain, address } => {
                // Stellar analogue of the EVM `ServiceURIUpdated` log
                // subscription above. Fire-and-forget: the watcher waits
                // for the chain's stellar controller to come up and then
                // registers the filter — same async pattern as
                // `WatchEvmContractEvents`.
                self.command_sender
                    .send(TriggerCommand::WatchStellarServiceUri {
                        service_id: service.id(),
                        chain,
                        project_root: address,
                    })?;
            }
        }

        for command in workflow_commands {
            self.command_sender.send(command)?;
        }

        Ok(())
    }

    #[instrument(skip(self), fields(subsys = "TriggerManager"))]
    pub fn remove_service(&self, service_id: ServiceId) -> Result<(), TriggerError> {
        self.lookup_maps
            .remove_service(service_id.clone(), &self.stellar_controllers)?;

        // Drop any `UpdatedSpecRepo` subscription that `add_service`
        // registered for this service. Presence in the map is the canonical
        // signal that this service is Stellar-managed — no need to look up
        // the service's manager (which has already been removed from the
        // Services store by the dispatcher at this point).
        if self
            .stellar_uri_subscriptions
            .read()
            .unwrap()
            .contains_key(&service_id)
        {
            self.command_sender
                .send(TriggerCommand::UnwatchStellarServiceUri {
                    service_id: service_id.clone(),
                })?;
        }

        // TODO - consider sending commands to:
        // 1. stop listening to chains if no triggers remain for them
        // 2. remove any cron jobs if no triggers remain for them
        // 3. remove any EVM log subscriptions if no triggers remain for them
        // 4. remove any block subscriptions if no triggers remain for them

        Ok(())
    }

    #[instrument(skip(self, ctx), fields(subsys = "TriggerManager"))]
    pub fn start(&self, ctx: AppContext) {
        let kill_receiver = ctx.get_kill_receiver();
        ctx.rt.block_on(self.start_watcher(kill_receiver)).unwrap();
    }

    pub fn send_dispatcher_commands(
        &self,
        commands: impl IntoIterator<Item = DispatcherCommand>,
    ) -> Result<(), TriggerError> {
        for command in commands {
            match &command {
                DispatcherCommand::Trigger(action) => {
                    #[cfg(feature = "dev")]
                    if std::env::var("WARPDRIVE_FORCE_TRIGGER_ERROR_XXX").is_ok() {
                        self.metrics.increment_total_errors("forced trigger error");
                        continue;
                    }

                    tracing_service_info!(
                        &self.services,
                        action.config.service_id,
                        "Sending trigger action for workflow {}",
                        action.config.workflow_id,
                    );

                    self.metrics
                        .record_trigger_fired(action.data.chain(), action.data.trigger_type_str());
                }
                DispatcherCommand::ChangeServiceUri { service_id, uri } => {
                    tracing_service_info!(
                        &self.services,
                        service_id,
                        "Changing service URI to {}",
                        uri
                    );
                }
                _ => {}
            }

            let start = std::time::Instant::now();
            self.subsystem_to_dispatcher_tx
                .send(command)
                .map_err(Box::new)?;

            self.metrics
                .record_trigger_sent_dispatcher_command(start.elapsed().as_secs_f64());
        }

        Ok(())
    }

    pub fn add_trigger(&self, trigger: TriggerAction) -> Result<(), TriggerError> {
        self.command_sender
            .send(TriggerCommand::ManualTrigger(Box::new(trigger)))?;
        Ok(())
    }

    #[instrument(skip(self), fields(subsys = "TriggerManager"))]
    async fn start_watcher(
        &self,
        mut kill_receiver: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), TriggerError> {
        let mut multiplexed_stream: MultiplexedStream = SelectAll::new();

        let local_command_stream = local_command_stream::start_local_command_stream(
            self.command_receiver.lock().unwrap().take().unwrap(),
            self.metrics.clone(),
        )?;
        multiplexed_stream.push(local_command_stream);

        let mut cosmos_clients = HashMap::new();

        let mut listening_chain_states: HashMap<ChainKey, StreamStartState> = HashMap::new();
        let mut cron_stream_state = StreamStartState::Waiting;
        let mut atproto_stream_state = StreamStartState::Waiting;

        // Create a stream for cron triggers that produces a trigger for each due task

        loop {
            let res = tokio::select! {
                _ = kill_receiver.recv() => {
                    tracing::debug!("Trigger Manager watcher received shutdown");
                    break;
                }
                res = multiplexed_stream.next() => res,
            };
            let Some(res) = res else {
                break;
            };
            let res = match res {
                Err(err) => {
                    tracing::error!("{:?}", err);
                    continue;
                }
                Ok(res) => res,
            };

            tracing::debug!("Processing trigger stream event: {:?}", res);
            let mut dispatcher_commands = Vec::new();

            match res {
                StreamTriggers::LocalCommand(command) => {
                    match command {
                        TriggerCommand::Kill => {
                            tracing::info!("Received kill command, shutting down trigger manager");
                            break;
                        }
                        TriggerCommand::ManualTrigger(trigger_action) => {
                            // send it directly to dispatcher
                            dispatcher_commands.push(DispatcherCommand::Trigger(*trigger_action));
                        }
                        TriggerCommand::StartListeningCron => {
                            #[cfg(feature = "dev")]
                            if self.disable_networking {
                                tracing::warn!(
                                    "Networking is disabled, skipping cron stream start"
                                );
                                continue;
                            }

                            match cron_stream_state {
                                StreamStartState::Connected => {
                                    tracing::debug!("Cron stream already started, skipping");
                                    continue;
                                }
                                StreamStartState::Connecting => {
                                    tracing::debug!("Cron stream is already starting, skipping");
                                    continue;
                                }
                                StreamStartState::Waiting => {
                                    cron_stream_state = StreamStartState::Connecting;
                                }
                            }

                            let cron_scheduler = self.lookup_maps.cron_scheduler.clone();
                            let cron_start_result = cron_stream::start_cron_stream(
                                cron_scheduler,
                                self.metrics.clone(),
                            )
                            .await;
                            let was_connecting =
                                matches!(cron_stream_state, StreamStartState::Connecting);
                            match cron_start_result {
                                Ok(cron_stream) => {
                                    multiplexed_stream.push(cron_stream);
                                    if was_connecting {
                                        cron_stream_state = StreamStartState::Connected;
                                    }
                                }
                                Err(err) => {
                                    tracing::error!("Failed to start cron stream: {:?}", err);
                                    if was_connecting {
                                        cron_stream_state = StreamStartState::Waiting;
                                    }
                                    continue;
                                }
                            }
                        }
                        TriggerCommand::StartListeningChain { chain } => {
                            #[cfg(feature = "dev")]
                            if self.disable_networking {
                                tracing::warn!(
                                    "Networking is disabled, skipping chain stream start"
                                );
                                continue;
                            }
                            let chain_state = listening_chain_states
                                .entry(chain.clone())
                                .or_insert(StreamStartState::Waiting);
                            match chain_state {
                                StreamStartState::Connected => {
                                    tracing::debug!("Already listening to chain {chain}");
                                    continue;
                                }
                                StreamStartState::Connecting => {
                                    tracing::debug!("Chain {chain} is already starting");
                                    continue;
                                }
                                StreamStartState::Waiting => {
                                    *chain_state = StreamStartState::Connecting;
                                }
                            }

                            let chain_config =
                                match self.chain_configs.read().unwrap().get_chain(&chain) {
                                    Some(config) => config,
                                    None => {
                                        tracing::error!("No chain config found for {chain}");
                                        if let Some(chain_state) =
                                            listening_chain_states.get_mut(&chain)
                                        {
                                            *chain_state = StreamStartState::Waiting;
                                        }
                                        continue;
                                    }
                                };

                            match chain_config {
                                AnyChainConfig::Cosmos(chain_config) => {
                                    let cosmos_client = QueryClient::new(
                                        chain_config.clone().to_chain_config(),
                                        None,
                                    )
                                    .await
                                    .map_err(TriggerError::Climb)?;

                                    // Start the Cosmos event stream
                                    match cosmos_stream::start_cosmos_stream(
                                        cosmos_client.clone(),
                                        chain.clone(),
                                        self.metrics.clone(),
                                    )
                                    .await
                                    {
                                        Ok(cosmos_event_stream) => {
                                            multiplexed_stream.push(cosmos_event_stream);
                                            cosmos_clients
                                                .insert(chain.clone(), cosmos_client.clone());
                                            if let Some(chain_state) =
                                                listening_chain_states.get_mut(&chain)
                                            {
                                                *chain_state = StreamStartState::Connected;
                                            }
                                        }
                                        Err(err) => {
                                            tracing::error!(
                                                "Failed to start Cosmos event stream: {:?}",
                                                err
                                            );
                                            if let Some(chain_state) =
                                                listening_chain_states.get_mut(&chain)
                                            {
                                                *chain_state = StreamStartState::Waiting;
                                            }
                                            continue;
                                        }
                                    }
                                }
                                AnyChainConfig::Evm(chain_config) => {
                                    if chain_config.ws_endpoints.is_empty() {
                                        return Err(TriggerError::EvmMissingWebsocket(
                                            chain.clone(),
                                        ));
                                    }

                                    let chain_key: ChainKey = (&chain_config).into();
                                    let EvmTriggerStreams {
                                        controller,
                                        block_height_stream,
                                        log_stream,
                                        // ignoring this for now
                                        new_pending_transaction_stream: _,
                                    } = EvmTriggerStreams::new(
                                        chain_config.ws_endpoints,
                                        chain_key,
                                        chain_config.ws_priority_endpoint_index,
                                        self.metrics.evm_stream.clone(),
                                    );

                                    // Start the EVM event stream
                                    // however, the actual subscription for log filters is set via the controller
                                    let evm_event_stream = match evm_stream::start_evm_event_stream(
                                        chain.clone(),
                                        log_stream,
                                        self.metrics.clone(),
                                    )
                                    .await
                                    {
                                        Ok(stream) => stream,
                                        Err(err) => {
                                            tracing::error!(
                                                "Failed to start EVM event stream: {:?}",
                                                err
                                            );
                                            if let Some(chain_state) =
                                                listening_chain_states.get_mut(&chain)
                                            {
                                                *chain_state = StreamStartState::Waiting;
                                            }
                                            continue;
                                        }
                                    };

                                    // Start the EVM block stream
                                    // however, the actual subscription for blocks is gated via the controller
                                    let evm_block_stream = match evm_stream::start_evm_block_stream(
                                        chain.clone(),
                                        block_height_stream,
                                        self.metrics.clone(),
                                    )
                                    .await
                                    {
                                        Ok(stream) => stream,
                                        Err(err) => {
                                            tracing::error!(
                                                "Failed to start EVM block stream: {:?}",
                                                err
                                            );
                                            if let Some(chain_state) =
                                                listening_chain_states.get_mut(&chain)
                                            {
                                                *chain_state = StreamStartState::Waiting;
                                            }
                                            continue;
                                        }
                                    };

                                    multiplexed_stream.push(evm_event_stream);
                                    multiplexed_stream.push(evm_block_stream);

                                    self.evm_controllers
                                        .write()
                                        .unwrap()
                                        .insert(chain.clone(), controller);
                                    if let Some(chain_state) =
                                        listening_chain_states.get_mut(&chain)
                                    {
                                        *chain_state = StreamStartState::Connected;
                                    }
                                }

                                AnyChainConfig::Stellar(chain_config) => {
                                    if chain_config.rpc_url.is_empty() {
                                        return Err(TriggerError::StellarMissingRpc(chain.clone()));
                                    }

                                    let chain_key: ChainKey = (&chain_config).into();

                                    if self
                                        .stellar_controllers
                                        .read()
                                        .unwrap()
                                        .get(&chain_key)
                                        .is_none()
                                    {
                                        let controller =
                                            StellarStreamController::new(chain_config.clone())?;

                                        let channels = StellarChannels::new();

                                        let StellarChannelReceivers {
                                            event_rx,
                                            ledger_rx,
                                        } = channels.receivers;

                                        let StellarChannelSenders {
                                            ledger_tx,
                                            event_tx,
                                        } = channels.senders;

                                        // Start the event stream from the perspective of what the trigger needs
                                        // this does not directly interact with the chain - we send to it from a channel later
                                        // in other words, these streams are more like a mapping from native Stellar events/ledgers
                                        // to our internal StreamTriggers, and the pollers which we will create a bit further below
                                        // are what actually interact with the chain and send data to these streams
                                        let event_stream = start_stellar_event_stream(
                                            chain.clone(),
                                            event_rx,
                                            self.metrics.clone(),
                                        )
                                        .await;

                                        let ledger_stream = start_stellar_ledger_stream(
                                            chain.clone(),
                                            ledger_rx,
                                            self.metrics.clone(),
                                        )
                                        .await;

                                        let (event_stream, ledger_stream) =
                                            match (event_stream, ledger_stream) {
                                                (Ok(event_stream), Ok(ledger_stream)) => {
                                                    (event_stream, ledger_stream)
                                                }
                                                (Err(err), _) | (_, Err(err)) => {
                                                    tracing::error!(
                                                    "Failed to start Stellar event stream: {:?}",
                                                    err
                                                );
                                                    if let Some(chain_state) =
                                                        listening_chain_states.get_mut(&chain)
                                                    {
                                                        *chain_state = StreamStartState::Waiting;
                                                    }
                                                    continue;
                                                }
                                            };

                                        // Now create the pollers. For right now, they just run forever once kicked off
                                        // However, the event poller's filters can be adjusted on the fly via the shared client
                                        tokio::spawn({
                                            let controller = controller.clone();
                                            async move {
                                                start_stellar_event_poller(controller, event_tx)
                                                    .await
                                            }
                                        });

                                        tokio::spawn({
                                            let controller = controller.clone();
                                            async move {
                                                start_stellar_ledger_poller(controller, ledger_tx)
                                                    .await
                                            }
                                        });

                                        // Now that we'll be sending into the streams, we can multiplex them in
                                        multiplexed_stream.push(event_stream);
                                        multiplexed_stream.push(ledger_stream);

                                        // and stash our controller so we can adjust filters and polling on the fly based on trigger configs
                                        self.stellar_controllers
                                            .write()
                                            .unwrap()
                                            .insert(chain_key, controller);

                                        if let Some(chain_state) =
                                            listening_chain_states.get_mut(&chain)
                                        {
                                            *chain_state = StreamStartState::Connected;
                                        }
                                    } else {
                                        tracing::warn!(
                                            "Stellar stream for chain {chain} is already running"
                                        );
                                    }
                                }
                            }
                        }
                        TriggerCommand::WatchEvmContractEvents {
                            chain,
                            addresses,
                            event_hashes,
                        } => match self.evm_controllers.read().unwrap().get(&chain) {
                            Some(evm_controller) => {
                                evm_controller
                                    .subscriptions
                                    .enable_logs(addresses, event_hashes);
                            }
                            None => {
                                tracing::error!(
                                        "No EVM controller found for chain {chain}, cannot watch contract event"
                                    );
                                continue;
                            }
                        },
                        TriggerCommand::WatchEvmBlocks { chain } => {
                            match self.evm_controllers.read().unwrap().get(&chain) {
                                Some(evm_controller) => {
                                    evm_controller.subscriptions.toggle_block_height(true);
                                }
                                None => {
                                    tracing::error!(
                                        "No EVM controller found for chain {chain}, cannot watch blocks"
                                    );
                                    continue;
                                }
                            }
                        }
                        // There is no `WatchStellarContractEvents` command for *trigger* filters
                        // because they need the rpc_id round-tripped — they're added inline in
                        // `lookup_maps.add_service`. Service-URI subscriptions don't need that:
                        // we match incoming events by service-manager address + topic at dispatch
                        // time, so we can fire-and-forget here.
                        TriggerCommand::WatchStellarServiceUri {
                            service_id,
                            chain,
                            project_root,
                        } => {
                            use stellar_xdr::curr::{ScSymbol, ScVal, StringM};
                            use streams::stellar_stream::filters::StellarEventFilter;
                            use warpdrive_types::StellarTopicSegment;

                            // Topic name MUST match the soroban-sdk
                            // `#[contractevent]` macro's symbol-derivation rule
                            // (snake_case of the struct name) for `UpdatedSpecRepo`
                            // in `warpdrive-contracts/.../project_root.rs`. If
                            // either the macro behavior or the struct name changes,
                            // this filter silently won't match — symptom is
                            // `wait_for_service_update` timing out with no events
                            // dispatched.
                            let topic = match StringM::try_from("updated_spec_repo".as_bytes()) {
                                Ok(s) => ScVal::Symbol(ScSymbol(s)),
                                Err(err) => {
                                    tracing::error!(
                                        ?err,
                                        "Failed to encode 'updated_spec_repo' topic symbol"
                                    );
                                    continue;
                                }
                            };
                            // `format!` rather than `.to_string()`: in
                            // stellar-strkey 0.0.16 `Contract` has an
                            // inherent `to_string()` that returns
                            // `heapless::String<56>` for no_std use; the
                            // `Display` impl gives us a regular `String`.
                            let filter = match StellarEventFilter::new(
                                format!("{project_root}"),
                                vec![StellarTopicSegment::Exact(topic)],
                            ) {
                                Ok(f) => f,
                                Err(err) => {
                                    tracing::error!(
                                        ?err,
                                        chain = %chain,
                                        project_root = %project_root,
                                        "Failed to build UpdatedSpecRepo event filter"
                                    );
                                    continue;
                                }
                            };
                            match self.stellar_controllers.read().unwrap().get(&chain) {
                                Some(controller) => {
                                    match controller
                                        .client
                                        .update_event_filters(|f| f.add_filter(filter))
                                    {
                                        Ok(rpc_id) => {
                                            self.stellar_uri_subscriptions.write().unwrap().insert(
                                                service_id.clone(),
                                                (chain.clone(), rpc_id),
                                            );
                                            tracing::debug!(
                                                chain = %chain,
                                                project_root = %project_root,
                                                service_id = %service_id,
                                                "Registered UpdatedSpecRepo subscription"
                                            );
                                        }
                                        Err(err) => {
                                            tracing::error!(
                                                ?err,
                                                chain = %chain,
                                                project_root = %project_root,
                                                "Failed to register UpdatedSpecRepo filter"
                                            );
                                        }
                                    }
                                }
                                None => {
                                    tracing::error!(
                                        chain = %chain,
                                        "No Stellar controller for chain when registering UpdatedSpecRepo filter"
                                    );
                                    continue;
                                }
                            }
                        }
                        TriggerCommand::UnwatchStellarServiceUri { service_id } => {
                            let entry = self
                                .stellar_uri_subscriptions
                                .write()
                                .unwrap()
                                .remove(&service_id);
                            let (chain, rpc_id) = match entry {
                                Some(v) => v,
                                None => {
                                    tracing::debug!(
                                        service_id = %service_id,
                                        "UnwatchStellarServiceUri: no subscription tracked, nothing to do"
                                    );
                                    continue;
                                }
                            };
                            match self.stellar_controllers.read().unwrap().get(&chain) {
                                Some(controller) => {
                                    controller
                                        .client
                                        .update_event_filters(|f| f.remove_filter_by_id(rpc_id));
                                    tracing::debug!(
                                        chain = %chain,
                                        service_id = %service_id,
                                        "Removed UpdatedSpecRepo subscription"
                                    );
                                }
                                None => {
                                    // Controller is gone (e.g. shutdown) — the
                                    // filter went with it, so the bookkeeping
                                    // entry we already removed above is
                                    // sufficient.
                                    tracing::debug!(
                                        chain = %chain,
                                        service_id = %service_id,
                                        "UnwatchStellarServiceUri: no controller for chain (already gone)"
                                    );
                                }
                            }
                        }
                        TriggerCommand::WatchStellarBlocks { chain } => {
                            match self.stellar_controllers.read().unwrap().get(&chain) {
                                Some(controller) => {
                                    controller.enable_ledger_polling();
                                }
                                None => {
                                    tracing::error!(
                                        "No Stellar controller found for chain {chain}, cannot watch blocks"
                                    );
                                    continue;
                                }
                            }
                        }
                        TriggerCommand::StartListeningAtProto => {
                            #[cfg(feature = "dev")]
                            if self.disable_networking {
                                tracing::warn!(
                                    "Networking is disabled, skipping ATProto stream start"
                                );
                                continue;
                            }

                            match atproto_stream_state {
                                StreamStartState::Connected => {
                                    tracing::debug!("ATProto stream already started, skipping");
                                    continue;
                                }
                                StreamStartState::Connecting => {
                                    tracing::debug!("ATProto stream is already starting, skipping");
                                    continue;
                                }
                                StreamStartState::Waiting => {
                                    atproto_stream_state = StreamStartState::Connecting;
                                }
                            }

                            // Subscribe to all ATProto events - filtering will be done in the lookup system
                            let jetstream_config = streams::atproto_jetstream::JetstreamConfig {
                                endpoint: self.config.jetstream_endpoint.clone(),
                                wanted_collections: vec![], // Empty means subscribe to all collections
                                wanted_dids: None,          // Listen to all repos
                                cursor: None,
                                compression: false,
                                max_message_size: self.config.jetstream_max_message_size,
                                require_hello: false,
                            };

                            // Start the ATProto Jetstream stream
                            let atproto_start_result =
                                streams::atproto_jetstream::start_jetstream_stream(
                                    jetstream_config,
                                    self.metrics.clone(),
                                )
                                .await;
                            let was_connecting =
                                matches!(atproto_stream_state, StreamStartState::Connecting);
                            match atproto_start_result {
                                Ok(atproto_stream) => {
                                    multiplexed_stream.push(atproto_stream);
                                    tracing::info!("Started ATProto Jetstream stream");
                                    if was_connecting {
                                        atproto_stream_state = StreamStartState::Connected;
                                    }
                                }
                                Err(err) => {
                                    tracing::error!(
                                        "Failed to start ATProto Jetstream stream: {:?}",
                                        err
                                    );
                                    if was_connecting {
                                        atproto_stream_state = StreamStartState::Waiting;
                                    }
                                    continue;
                                }
                            }
                        }
                    }
                }
                StreamTriggers::Evm {
                    log,
                    chain,
                    block_number,
                    tx_hash,
                    log_index,
                    block_hash,
                    tx_index,
                    block_timestamp,
                } => {
                    if let Some(event_hash) = log.topic0() {
                        let contract_address = log.address();

                        if *event_hash
                            == IWarpDriveServiceManager::ServiceURIUpdated::SIGNATURE_HASH
                        {
                            // 3. Decode the event data
                            match IWarpDriveServiceManager::ServiceURIUpdated::decode_log_data(
                                log.data(),
                            ) {
                                Ok(decoded_event) => {
                                    let service_uri =
                                        UriString::try_from(decoded_event.serviceURI)?;
                                    // check if this is a service we're interested in
                                    if let Some(service_id) = self
                                        .lookup_maps
                                        .service_manager
                                        .read()
                                        .unwrap()
                                        .get_by_right(&contract_address.into())
                                    {
                                        dispatcher_commands.push(
                                            DispatcherCommand::ChangeServiceUri {
                                                service_id: service_id.clone(),
                                                uri: service_uri,
                                            },
                                        );
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "Failed to decode ServiceURIUpdated data: {}",
                                        e
                                    );
                                }
                            }
                        }

                        let triggers_by_contract_event_lock = self
                            .lookup_maps
                            .triggers_by_evm_contract_event
                            .read()
                            .unwrap();

                        if let Some(lookup_ids) = triggers_by_contract_event_lock.get(&(
                            chain.clone(),
                            contract_address,
                            ByteArray::new(**event_hash),
                        )) {
                            let trigger_data = TriggerData::EvmContractEvent {
                                contract_address,
                                chain,
                                log_data: log.data().clone(),
                                tx_hash,
                                block_number,
                                log_index,
                                block_hash,
                                block_timestamp,
                                tx_index,
                            };

                            for trigger_config in self.lookup_maps.get_trigger_configs(lookup_ids) {
                                dispatcher_commands.push(DispatcherCommand::Trigger(
                                    TriggerAction {
                                        data: trigger_data.clone(),
                                        config: trigger_config.clone(),
                                    },
                                ));
                            }
                        }
                    }
                }

                StreamTriggers::StellarEvent {
                    chain,
                    contract_id,
                    event_type,
                    ledger,
                    ledger_closed_at,
                    event_id,
                    operation_index,
                    transaction_index,
                    tx_hash,
                    topic_segments,
                    value,
                    rpc_ids,
                } => {
                    // First: is this an `UpdatedSpecRepo` event from a
                    // registered Stellar service manager? Match on
                    // (contract_id, topic[0]) directly using the existing
                    // `service_manager` BiMap — no per-service rpc_id
                    // tracking needed because we filter by topic at
                    // registration time and check it again here for safety
                    // (in case some other filter on the same project_root
                    // contract gets added later).
                    if is_updated_spec_repo_topic(&topic_segments) {
                        if let Ok(parsed) = stellar_strkey::Contract::from_string(&contract_id) {
                            let chain_addr = warpdrive_types::ChainAddress::StellarContract(parsed);
                            let service_id_opt = self
                                .lookup_maps
                                .service_manager
                                .read()
                                .unwrap()
                                .get_by_right(&chain_addr)
                                .cloned();
                            if let Some(service_id) = service_id_opt {
                                match decode_stellar_event_string_field(&value, "repo") {
                                    Some(uri_str) => match UriString::try_from(uri_str.clone()) {
                                        Ok(uri) => {
                                            tracing::info!(
                                                service_id = %service_id,
                                                contract = %contract_id,
                                                new_uri = %uri_str,
                                                "Stellar UpdatedSpecRepo event → ChangeServiceUri"
                                            );
                                            dispatcher_commands.push(
                                                DispatcherCommand::ChangeServiceUri {
                                                    service_id: service_id.clone(),
                                                    uri,
                                                },
                                            );
                                        }
                                        Err(err) => {
                                            tracing::warn!(
                                                ?err,
                                                "Stellar UpdatedSpecRepo payload was not a valid URI; skipping"
                                            );
                                        }
                                    },
                                    None => {
                                        tracing::warn!(
                                            "Stellar UpdatedSpecRepo event value did not decode to repo:String; skipping"
                                        );
                                    }
                                }
                            }
                        }
                    }

                    // Handle regular events

                    let triggers_by_contract_event_lock = self
                        .lookup_maps
                        .triggers_by_stellar_contract_event
                        .read()
                        .unwrap();

                    let trigger_data = TriggerData::StellarContractEvent {
                        chain,
                        contract_id,
                        event_type,
                        ledger,
                        ledger_closed_at,
                        event_id,
                        operation_index,
                        transaction_index,
                        tx_hash,
                        topic_segments,
                        value,
                    };

                    // BiMap forward direction (rpc id -> lookup id). The reverse
                    // direction is exercised in `LookupMaps::remove_workflow` /
                    // `remove_service` so per-workflow cleanup can target a single
                    // rpc id.
                    let lookup_ids = rpc_ids
                        .into_iter()
                        .filter_map(|rpc_id| triggers_by_contract_event_lock.get_by_left(&rpc_id));

                    for trigger_config in self.lookup_maps.get_trigger_configs(lookup_ids) {
                        dispatcher_commands.push(DispatcherCommand::Trigger(TriggerAction {
                            data: trigger_data.clone(),
                            config: trigger_config.clone(),
                        }));
                    }
                }

                StreamTriggers::Cosmos {
                    contract_events,
                    chain,
                    block_height,
                } => {
                    // extra scope in order to properly drop the locks
                    {
                        let triggers_by_contract_event_lock = self
                            .lookup_maps
                            .triggers_by_cosmos_contract_event
                            .read()
                            .unwrap();

                        for StreamTriggerCosmosContractEvent {
                            contract_address,
                            event,
                            event_index,
                        } in contract_events
                        {
                            if layer_climb::events::Event::from(&event)
                                .is_type(WavsServiceUriUpdatedEvent::EVENT_TYPE)
                            {
                                let service_uri = event.attributes.iter().find_map(|attr| {
                                    if attr.key
                                        == WavsServiceUriUpdatedEvent::EVENT_ATTR_KEY_SERVICE_URI
                                    {
                                        UriString::try_from(attr.value.clone()).ok()
                                    } else {
                                        None
                                    }
                                });

                                let service_uri = match service_uri {
                                    Some(uri) => uri,
                                    None => {
                                        tracing::error!(
                                            "ServiceURIUpdated event missing {} attribute",
                                            WavsServiceUriUpdatedEvent::EVENT_ATTR_KEY_SERVICE_URI
                                        );
                                        continue;
                                    }
                                };

                                if let Some(service_id) = self
                                    .lookup_maps
                                    .service_manager
                                    .read()
                                    .unwrap()
                                    .get_by_right(&contract_address.clone().into())
                                {
                                    dispatcher_commands.push(DispatcherCommand::ChangeServiceUri {
                                        service_id: service_id.clone(),
                                        uri: service_uri,
                                    });
                                }
                            }
                            if let Some(lookup_ids) = triggers_by_contract_event_lock.get(&(
                                chain.clone(),
                                contract_address.clone(),
                                event.ty.clone(),
                            )) {
                                let trigger_data = TriggerData::CosmosContractEvent {
                                    contract_address,
                                    chain: chain.clone(),
                                    event,
                                    event_index,
                                    block_height,
                                };
                                for trigger_config in
                                    self.lookup_maps.get_trigger_configs(lookup_ids)
                                {
                                    dispatcher_commands.push(DispatcherCommand::Trigger(
                                        TriggerAction {
                                            data: trigger_data.clone(),
                                            config: trigger_config.clone(),
                                        },
                                    ));
                                }
                            }
                        }
                    }

                    // process block-based triggers
                    dispatcher_commands.extend(self.process_blocks(chain, block_height));
                }
                StreamTriggers::EvmBlock {
                    chain,
                    block_height,
                } => {
                    dispatcher_commands.extend(self.process_blocks(chain, block_height));
                }
                StreamTriggers::StellarLedgerSequence { chain, ledger } => {
                    dispatcher_commands.extend(self.process_blocks(chain, ledger as u64));
                }
                StreamTriggers::Cron { hits } => {
                    // Process each cron hit (group of triggers at the same scheduled time)
                    for hit in hits {
                        for lookup_id in &hit.lookup_ids {
                            if let Some(trigger_config) =
                                self.lookup_maps.get_trigger_config(*lookup_id)
                            {
                                dispatcher_commands.push(DispatcherCommand::Trigger(
                                    TriggerAction {
                                        data: TriggerData::Cron {
                                            trigger_time: hit.scheduled_time,
                                        },
                                        config: trigger_config.clone(),
                                    },
                                ));
                            }
                        }
                    }
                }
                StreamTriggers::AtProto { event } => {
                    let action_enum = event.action.clone();

                    // Find matching triggers using multiple lookup strategies
                    let mut matched_lookup_ids: HashSet<LookupId> = HashSet::new();

                    // Strategy 1: Exact match (collection, repo, action)
                    {
                        let triggers_by_atproto_lock = self
                            .lookup_maps
                            .triggers_by_atproto_event_exact
                            .read()
                            .unwrap();

                        // Check exact collection/repo/action match
                        if let Some(lookup_ids) = triggers_by_atproto_lock.get(&(
                            event.collection.clone(),
                            Some(event.repo.clone()),
                            Some(action_enum.clone()),
                        )) {
                            matched_lookup_ids.extend(lookup_ids);
                        }

                        // Check collection/repo match (any action)
                        if let Some(lookup_ids) = triggers_by_atproto_lock.get(&(
                            event.collection.clone(),
                            Some(event.repo.clone()),
                            None,
                        )) {
                            matched_lookup_ids.extend(lookup_ids);
                        }

                        // Check collection/action match (any repo)
                        if let Some(lookup_ids) = triggers_by_atproto_lock.get(&(
                            event.collection.clone(),
                            None,
                            Some(action_enum.clone()),
                        )) {
                            matched_lookup_ids.extend(lookup_ids);
                        }

                        // Check collection match (any repo, any action)
                        if let Some(lookup_ids) =
                            triggers_by_atproto_lock.get(&(event.collection.clone(), None, None))
                        {
                            matched_lookup_ids.extend(lookup_ids);
                        }
                    }

                    // Strategy 2: Pattern matching for collections with wildcards
                    {
                        let triggers_by_atproto_lock = self
                            .lookup_maps
                            .triggers_by_atproto_event_pattern
                            .read()
                            .unwrap();
                        // This collection only holds wildcard patterns, so the slower path
                        // doesn't need to scan the exact-match entries above.

                        for ((collection_pattern, repo_did_filter, action_filter), lookup_ids) in
                            triggers_by_atproto_lock.iter()
                        {
                            // Check collection pattern match (supports wildcards)
                            if self
                                .matches_collection_pattern(collection_pattern, &event.collection)
                            {
                                // Check repo filter
                                let repo_matches = match repo_did_filter {
                                    Some(filter_did) => filter_did == &event.repo,
                                    None => true, // any repo
                                };

                                // Check action filter
                                let action_matches = match action_filter {
                                    Some(filter_action) => filter_action == &action_enum,
                                    None => true, // any action
                                };

                                if repo_matches && action_matches {
                                    matched_lookup_ids.extend(lookup_ids);
                                }
                            }
                        }
                    }

                    // Create trigger actions for all matched lookups
                    if !matched_lookup_ids.is_empty() {
                        let trigger_data = TriggerData::AtProtoEvent {
                            sequence: event.sequence,
                            timestamp: event.timestamp,
                            repo: event.repo.clone(),
                            collection: event.collection.clone(),
                            rkey: event.rkey.clone(),
                            action: action_enum.clone(),
                            cid: event.cid.clone(),
                            record: event.record.clone(),
                            rev: event.rev.clone(),
                            op_index: event.op_index,
                        };

                        for trigger_config in
                            self.lookup_maps.get_trigger_configs(&matched_lookup_ids)
                        {
                            dispatcher_commands.push(DispatcherCommand::Trigger(TriggerAction {
                                data: trigger_data.clone(),
                                config: trigger_config.clone(),
                            }));
                        }

                        tracing::info!(
                            "ATProto event matched {} triggers: collection={}, repo={}, action={}",
                            matched_lookup_ids.len(),
                            event.collection,
                            event.repo,
                            action_enum
                        );
                    }
                }
            }

            if !dispatcher_commands.is_empty() {
                tracing::debug!(
                    "Sending {} commands to dispatcher",
                    dispatcher_commands.len()
                );
                for (idx, command) in dispatcher_commands.iter().enumerate() {
                    if let DispatcherCommand::Trigger(action) = command {
                        tracing::debug!(
                            batch = idx + 1,
                            service_id = %action.config.service_id,
                            workflow_id = %action.config.workflow_id,
                            trigger_data = ?action.data,
                            "Trigger action (in this batch)"
                        );
                    }
                }

                self.send_dispatcher_commands(dispatcher_commands)?;
            }
        }

        tracing::debug!("Trigger Manager watcher finished");

        // just a little cleanup to avoid errors in e2e tests
        for (_, controller) in self.evm_controllers.write().unwrap().drain() {
            drop(controller);
        }

        Ok(())
    }

    /// Process blocks and return trigger actions for any triggers that should fire
    pub fn process_blocks(&self, chain: ChainKey, block_height: u64) -> Vec<DispatcherCommand> {
        let block_height = match NonZeroU64::new(block_height) {
            Some(height) => height,
            None => {
                self.metrics.increment_total_errors("block height is zero");
                return Vec::new();
            }
        };
        // Get the triggers that should fire at this block height
        let firing_lookup_ids: Vec<LookupId> =
            match self.lookup_maps.block_schedulers.get_mut(&chain) {
                Some(mut scheduler) => scheduler
                    .tick(block_height.into())
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect(),
                None => Vec::new(),
            };

        // Convert lookup_ids to TriggerActions
        if !firing_lookup_ids.is_empty() {
            self.lookup_maps
                .get_trigger_configs(&firing_lookup_ids)
                .into_iter()
                .map(|trigger_config| {
                    DispatcherCommand::Trigger(TriggerAction {
                        data: TriggerData::BlockInterval {
                            chain: chain.clone(),
                            block_height: block_height.get(),
                        },
                        config: trigger_config,
                    })
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Check if a collection pattern matches the actual collection
    /// Supports wildcard patterns with '*' (e.g., "app.bsky.feed.*")
    fn matches_collection_pattern(&self, pattern: &str, actual: &str) -> bool {
        // Exact match
        if pattern == actual {
            return true;
        }

        // Wildcard pattern matching
        if let Some(prefix) = pattern.strip_suffix(".*") {
            return actual.starts_with(prefix)
                && actual.len() > prefix.len()
                && actual[prefix.len()..].starts_with('.');
        }

        false
    }

    #[cfg(feature = "dev")]
    pub fn get_lookup_maps(&self) -> &Arc<LookupMaps> {
        &self.lookup_maps
    }
}

/// True iff `topic_segments[0]` decodes to `Symbol("updated_spec_repo")`.
/// Lets the StellarEvent dispatcher distinguish `UpdatedSpecRepo` events
/// from triggers that happen to share a contract id. Topic name MUST stay
/// in sync with `warpdrive-contracts/.../project_root.rs` `UpdatedSpecRepo`
/// (snake_case of the struct name, per soroban-sdk's `#[contractevent]`).
fn is_updated_spec_repo_topic(topic_segments: &[String]) -> bool {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use stellar_xdr::curr::{Limits, ReadXdr, ScSymbol, ScVal};
    let Some(first) = topic_segments.first() else {
        return false;
    };
    let Ok(bytes) = STANDARD.decode(first) else {
        return false;
    };
    let Ok(scval) = ScVal::from_xdr(&bytes, Limits::none()) else {
        return false;
    };
    matches!(
        scval,
        ScVal::Symbol(ScSymbol(ref s)) if s.as_slice() == b"updated_spec_repo"
    )
}

/// Decode the `value` field of a Stellar contract event (XDR-base64 of an
/// `ScVal`) into the named string field's value.
///
/// `#[contractevent]` on a struct like `UpdatedSpecRepo { repo: String }`
/// publishes the data as `ScVal::Map { Symbol(field_name) -> ScVal::String(value) }`.
/// We pull out the entry matching `field_name` and return its string. Also
/// handles the bare `ScVal::String` case for events that happen to have a
/// single-string data layout.
///
/// Returns `None` for any decode failure or shape mismatch so callers can
/// log + skip without aborting.
fn decode_stellar_event_string_field(value_xdr_base64: &str, field_name: &str) -> Option<String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use stellar_xdr::curr::{Limits, ReadXdr, ScString, ScSymbol, ScVal};
    let bytes = STANDARD.decode(value_xdr_base64).ok()?;
    let scval = ScVal::from_xdr(&bytes, Limits::none()).ok()?;
    match scval {
        ScVal::String(ScString(s)) => String::from_utf8(s.as_slice().to_vec()).ok(),
        ScVal::Map(Some(map)) => {
            for entry in map.iter() {
                if let ScVal::Symbol(ScSymbol(sym)) = &entry.key {
                    if sym.as_slice() == field_name.as_bytes() {
                        if let ScVal::String(ScString(s)) = &entry.val {
                            return String::from_utf8(s.as_slice().to_vec()).ok();
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::{config::Config, services::Services};
    use utils::{
        storage::db::WavsDb, telemetry::TriggerMetrics, test_utils::address::rand_address_evm,
    };
    use warpdrive_types::{
        Component, ComponentDigest, ComponentSource, ServiceManager, SignatureKind, Submit,
        Trigger, TriggerAction, TriggerConfig, TriggerData, Workflow, WorkflowId,
    };

    #[test]
    fn test_add_trigger() {
        let config = Config::default();

        let db_storage = WavsDb::new().unwrap();
        let services = Services::new(db_storage);

        let metrics = TriggerMetrics::new(opentelemetry::global::meter("test"));
        let (dispatcher_tx, dispatcher_rx) = crossbeam::channel::unbounded::<DispatcherCommand>();

        let service = warpdrive_types::Service {
            name: "serv1".to_string(),
            status: warpdrive_types::ServiceStatus::Active,
            manager: ServiceManager::Evm {
                chain: "evm:anvil".parse().unwrap(),
                address: rand_address_evm(),
            },
            workflows: vec![(
                "workflow-1".parse().unwrap(),
                Workflow {
                    trigger: Trigger::Manual,
                    component: Component::new(ComponentSource::Digest(ComponentDigest::hash(
                        [0; 32],
                    ))),
                    submit: Submit::Aggregator {
                        component: Box::new(Component::new(ComponentSource::Digest(
                            ComponentDigest::hash([0; 32]),
                        ))),
                        signature_kind: SignatureKind::evm_default(),
                    },
                },
            )]
            .into_iter()
            .collect(),
        };

        let ctx = utils::context::AppContext::new();

        ctx.rt
            .block_on(services.save(&service, config.chains.clone()))
            .unwrap();

        let trigger_manager =
            TriggerManager::new(&config, metrics, services, dispatcher_tx).unwrap();

        std::thread::spawn({
            let trigger_manager = trigger_manager.clone();
            let ctx = ctx.clone();
            move || {
                trigger_manager.start(ctx);
            }
        });

        // short sleep for trigger manager to kick in
        std::thread::sleep(Duration::from_millis(100));

        for i in 0..6 {
            let action = TriggerAction {
                config: TriggerConfig {
                    service_id: service.id(),
                    workflow_id: WorkflowId::new("workflow-1").unwrap(),
                    trigger: Trigger::Manual,
                },
                data: TriggerData::Raw(vec![i as u8]),
            };

            let result = trigger_manager.add_trigger(action);
            assert!(result.is_ok(), "Failed to add trigger {}: {:?}", i, result);
        }

        let mut received_count = 0;
        while let Ok(command) = dispatcher_rx.recv() {
            if let DispatcherCommand::Trigger(action) = command {
                if let TriggerData::Raw(data) = &action.data {
                    assert_eq!(
                        data,
                        &vec![received_count as u8],
                        "Trigger {} data mismatch",
                        received_count
                    );
                    received_count += 1;
                    if received_count == 6 {
                        break;
                    }
                }
            }
        }
        assert_eq!(received_count, 6, "Expected to receive 6 triggers");

        ctx.kill();
    }
}
