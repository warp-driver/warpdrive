pub mod error;
pub mod p2p;
pub mod peer;
mod queue;
mod submit;

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use layer_climb::prelude::*;
use tracing::instrument;
use utils::{
    async_transaction::AsyncTransaction, config::EvmChainConfigExt, context::AppContext,
    evm_client::EvmSigningClient, storage::db::WavsDb, telemetry::AggregatorMetrics,
};
use warpdrive_engine::bindings::aggregator::world::AnyTxHash;
use warpdrive_types::{
    AggregatorAction, ChainKey, QuorumQueue, QuorumQueueId, Service, Submission, Submit,
    SubmitAction, TimerAction,
};

use crate::{
    config::Config,
    dispatcher::DispatcherCommand,
    services::Services,
    subsystems::{
        aggregator::{
            error::AggregatorError, p2p::P2pHandle, peer::Peer, queue::append_submission_to_queue,
            submit::AnyTransactionReceipt,
        },
        engine::AggregatorExecuteKind,
    },
};

pub struct Aggregator {
    pub metrics: AggregatorMetrics,
    storage: WavsDb,
    config: Arc<Config>,
    services: Services,
    dispatcher_to_aggregator_rx: crossbeam::channel::Receiver<AggregatorCommand>,
    aggregator_to_self_tx: crossbeam::channel::Sender<AggregatorCommand>,
    subsystem_to_dispatcher_tx: crossbeam::channel::Sender<DispatcherCommand>,
    evm_submission_clients: Arc<std::sync::RwLock<HashMap<ChainKey, EvmSigningClient>>>,
    cosmos_submission_clients:
        Arc<std::sync::RwLock<HashMap<ChainKey, layer_climb::prelude::SigningClient>>>,
    queue_transaction: AsyncTransaction<QuorumQueueId>,
    chain_transaction: AsyncTransaction<ChainKey>,
    /// Optional P2P handle for broadcasting submissions to peers
    p2p_handle: Arc<std::sync::RwLock<Option<P2pHandle>>>,
    /// Tracks whether this is the primary instance (true) or a clone for async tasks (false).
    /// Only the primary instance logs a warning when dropped.
    is_primary: Arc<AtomicBool>,
}

#[derive(Debug)]
pub enum AggregatorCommand {
    Kill,
    // From Submission Manager
    Broadcast(Submission),
    // From Peers and Self
    Receive {
        submission: Submission,
        peer: Peer,
    },
    // From Engine Manager (right before sending on-chain)
    Actions {
        submission: Submission,
        actions: Vec<AggregatorAction>,
        kind: AggregatorExecuteKind,
    },
    // Subscribe to a service's P2P topic (called when service is added)
    SubscribeService {
        service_id: warpdrive_types::ServiceId,
    },
    /// Unsubscribe from P2P topic when service is removed
    UnsubscribeService {
        service_id: warpdrive_types::ServiceId,
    },
}

impl Aggregator {
    #[allow(clippy::new_without_default)]
    #[instrument(skip(services), fields(subsys = "Aggregator"))]
    pub fn new(
        config: &Config,
        metrics: AggregatorMetrics,
        services: Services,
        dispatcher_to_aggregator_rx: crossbeam::channel::Receiver<AggregatorCommand>,
        aggregator_to_self_tx: crossbeam::channel::Sender<AggregatorCommand>,
        subsystem_to_dispatcher_tx: crossbeam::channel::Sender<DispatcherCommand>,
    ) -> Result<Self, AggregatorError> {
        // Fail-fast on credential misconfiguration. Each helper is a
        // no-op when its credential is `None` (intentional opt-out).
        // When a credential *is* set, we parse it offline (no network)
        // and bail at startup so the sysadmin sees the misconfig
        // immediately rather than at the first submission for the
        // affected chain. See #24 for the broader motivation.
        validate_evm_credential(config.aggregator_evm_credential.as_ref())?;
        validate_cosmos_credential(config.aggregator_cosmos_credential.as_ref())?;
        validate_stellar_credential(config.aggregator_stellar_credential.as_ref())?;

        Ok(Self {
            storage: WavsDb::new().map_err(AggregatorError::Db)?,
            dispatcher_to_aggregator_rx,
            aggregator_to_self_tx,
            subsystem_to_dispatcher_tx,
            metrics,
            services,
            evm_submission_clients: Arc::new(std::sync::RwLock::new(HashMap::default())),
            cosmos_submission_clients: Arc::new(std::sync::RwLock::new(HashMap::default())),
            config: Arc::new(config.clone()),
            queue_transaction: AsyncTransaction::new(false),
            chain_transaction: AsyncTransaction::new(false),
            p2p_handle: Arc::new(std::sync::RwLock::new(None)), // Initialized in start() method
            is_primary: Arc::new(AtomicBool::new(true)),
        })
    }

    /// Get the current P2P network status
    pub async fn get_p2p_status(&self) -> warpdrive_types::P2pStatus {
        let handle = self.p2p_handle.read().unwrap().clone();
        match handle {
            Some(h) => h.get_status().await.unwrap_or_default(),
            None => warpdrive_types::P2pStatus::default(),
        }
    }

    #[instrument(skip(self, ctx), fields(subsys = "Aggregator"))]
    pub fn start(&self, ctx: AppContext) {
        let _self = self.clone();

        // Initialize P2P network if configured
        ctx.rt.block_on(async {
            // Pass signing_mnemonic to derive a persistent P2P identity
            let signing_mnemonic = self.config.signing_mnemonic.as_ref().map(|c| c.as_str());
            match P2pHandle::new(
                ctx.clone(),
                self.config.p2p.clone(),
                signing_mnemonic,
                self.aggregator_to_self_tx.clone(),
            )
            .await
            {
                Ok(Some(handle)) => {
                    tracing::info!("P2P network initialized successfully");
                    *self.p2p_handle.write().unwrap() = Some(handle);
                }
                Ok(None) => {
                    tracing::info!("P2P networking is disabled");
                }
                Err(e) => {
                    tracing::error!("Failed to initialize P2P network: {:?}", e);
                    ctx.kill();
                }
            }
        });

        // Spawn periodic cleanup task for burned queues
        {
            let _self = _self.clone();
            let mut shutdown_signal = ctx.get_kill_receiver();
            ctx.rt.spawn(async move {
                // Run cleanup every hour
                let mut cleanup_interval =
                    tokio::time::interval(tokio::time::Duration::from_secs(3600));
                cleanup_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                loop {
                    tokio::select! {
                        _ = shutdown_signal.recv() => {
                            tracing::info!("Burned queue cleanup task shutting down");
                            break;
                        }
                        _ = cleanup_interval.tick() => {
                            match _self.cleanup_old_burned_queues().await {
                                Ok(count) if count > 0 => {
                                    tracing::info!("Cleaned up {} old burned queue(s)", count);
                                }
                                Ok(_) => {
                                    tracing::debug!("No old burned queues to clean up");
                                }
                                Err(e) => {
                                    tracing::error!("Error cleaning up burned queues: {:?}", e);
                                }
                            }
                        }
                    }
                }
            });
        }

        while let Ok(command) = self.dispatcher_to_aggregator_rx.recv() {
            self.handle_dispatcher_command(&ctx, command);

            if ctx.killed() {
                break;
            }
        }
    }

    fn handle_dispatcher_command(&self, ctx: &AppContext, command: AggregatorCommand) {
        let label = match &command {
            AggregatorCommand::Kill => "Kill".to_string(),
            AggregatorCommand::Broadcast(submission) => {
                format!("{} Broadcast", submission.label())
            }
            AggregatorCommand::Receive { submission, peer } => {
                format!("{} Receive from peer: {:?}", submission.label(), peer)
            }
            AggregatorCommand::Actions {
                submission,
                actions,
                kind,
            } => format!(
                "{} {} Actions (count: {})",
                submission.label(),
                match kind {
                    AggregatorExecuteKind::Standard => "Standard",
                    AggregatorExecuteKind::SubmitCallback { .. } => "SubmitCallback",
                    AggregatorExecuteKind::TimerCallback => "TimerCallback",
                },
                actions.len()
            ),
            AggregatorCommand::SubscribeService { service_id } => {
                format!("SubscribeService({})", service_id)
            }
            AggregatorCommand::UnsubscribeService { service_id } => {
                format!("UnsubscribeService({})", service_id)
            }
        };

        tracing::info!("Aggregator received command: {}", label);

        match command {
            AggregatorCommand::Kill => {
                tracing::info!("Aggregator received Kill command, shutting down");
            }
            AggregatorCommand::Broadcast(submission) => {
                let service =
                    match self.extract_service_from_submission(&submission, &Peer::Me, &label) {
                        Some(s) => s,
                        None => {
                            return;
                        }
                    };

                ctx.rt.spawn({
                    let _self = self.clone();
                    async move {
                        match _self.handle_broadcast(&submission).await {
                            Ok(_) => {
                                _self
                                    .metrics
                                    .increment_broadcast_count(&service, submission.workflow_id());
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Aggregator: Error broadcasting submission: {:?}",
                                    e
                                );
                            }
                        }
                    }
                });
            }
            AggregatorCommand::Receive { submission, peer } => {
                let service = match self.extract_service_from_submission(&submission, &peer, &label)
                {
                    Some(s) => s,
                    None => {
                        return;
                    }
                };
                ctx.rt.spawn({
                    let _self = self.clone();
                    async move {
                        let workflow_id = submission.workflow_id().clone();

                        match _self.handle_receive(submission, service.clone()).await {
                            Ok(_) => {
                                _self
                                    .metrics
                                    .increment_receive_count(&service, &workflow_id);
                            }
                            Err(e) => {
                                tracing::error!("Aggregator: Error receiving submission: {:?}", e);
                            }
                        }
                    }
                });
            }

            AggregatorCommand::Actions {
                submission,
                actions,
                kind: _,
            } => {
                let service =
                    match self.extract_service_from_submission(&submission, &Peer::Me, &label) {
                        Some(s) => s,
                        None => {
                            return;
                        }
                    };

                for action in actions {
                    ctx.rt.spawn({
                        let _self = self.clone();
                        let service = service.clone();
                        let submission = submission.clone();
                        async move {
                            match action {
                                AggregatorAction::Submit(action) => {
                                    let workflow_id = submission.workflow_id().clone();
                                    let queue_id = QuorumQueueId {
                                        event_id: submission.event_id.clone(),
                                        action: action.clone(),
                                    };
                                    // Lock this queue_id to prevent concurrent modifications from other submissions
                                    // Other queue IDs can still run concurrently
                                    _self
                                        .queue_transaction
                                        .run(queue_id.clone(), {
                                            let _self = _self.clone();
                                            move || async move {
                                                // Load existing queue or start fresh
                                                // Queue lifecycle:
                                                // 1. Active: Contains submissions waiting for quorum
                                                // 2. Burned: Successfully submitted on-chain, no longer valid
                                                let mut queue = match _self.get_quorum_queue(&queue_id).await {
                                                    Ok(queue) => {
                                                        match queue {
                                                            QuorumQueue::Burned(timestamp) => {
                                                                tracing::warn!("Tried to access burned quorum queue: {:?} (burned at {})", queue_id, timestamp);
                                                                return;
                                                            }
                                                            QuorumQueue::Active(submissions) => submissions,
                                                        }
                                                    }
                                                    Err(err) => {
                                                        tracing::error!(
                                                            "Aggregator: Error getting quorum queue {:?}: {:?}",
                                                            queue_id,
                                                            err
                                                        );
                                                        return;
                                                    },
                                                };

                                                // CRITICAL: Append current submission to the queue
                                                // This submission is now IN the queue, so we must save it if submission fails
                                                // Otherwise this submission will be lost and never retried
                                                if let Err(err) = append_submission_to_queue(&queue_id, &mut queue,submission.clone()) {
                                                    tracing::error!("{}", err);
                                                    return;
                                                }


                                                tracing::info!("Queue count for {:?}: {}", queue_id, queue.len());

                                                match _self
                                                    .handle_submit_action(
                                                        &submission,
                                                        &service,
                                                        queue_id,
                                                        queue,
                                                        action,
                                                    )
                                                    .await
                                                {
                                                    Ok(_) => {
                                                        _self.metrics.increment_action_count(
                                                            &service,
                                                            &workflow_id,
                                                        );
                                                    }
                                                    Err(e) => {
                                                        tracing::error!(
                                                            "Aggregator: Error handling actions: {:?}",
                                                            e
                                                        );
                                                    }
                                                }
                                            }
                                        })
                                        .await;
                                },
                                AggregatorAction::Timer(TimerAction { delay }) => {
                                    // this is fine, we're in our own spawned task
                                    tokio::time::sleep(delay.into()).await;
                                    tracing::info!(
                                        "Timer expired after {} seconds, executing callback for event {}",
                                        delay.secs,
                                        submission.event_id
                                    );

                                    if let Err(e) = _self.subsystem_to_dispatcher_tx
                                        .send(DispatcherCommand::AggregatorExecute {
                                            submission: submission.clone(),
                                            service: service.clone(),
                                            kind: AggregatorExecuteKind::TimerCallback,
                                        }) {
                                            tracing::error!(
                                                "Aggregator: Error sending Timer callback to Dispatcher: {:?}",
                                                e
                                            );
                                        }

                                }
                            }
                        }
                    });
                }
            }
            AggregatorCommand::SubscribeService { service_id } => {
                if let Some(handle) = self.p2p_handle.read().unwrap().clone() {
                    if let Err(e) = handle.subscribe(&service_id) {
                        tracing::warn!(
                            "Failed to subscribe to P2P topic for service {}: {:?}",
                            service_id,
                            e
                        );
                    } else {
                        tracing::info!("Subscribed to P2P topic for service {}", service_id);
                    }
                }
            }
            AggregatorCommand::UnsubscribeService { service_id } => {
                if let Some(handle) = self.p2p_handle.read().unwrap().clone() {
                    if let Err(e) = handle.unsubscribe(&service_id) {
                        tracing::warn!(
                            "Failed to unsubscribe from P2P topic for service {}: {:?}",
                            service_id,
                            e
                        );
                    } else {
                        tracing::info!("Unsubscribed from P2P topic for service {}", service_id);
                    }
                }
            }
        }
    }

    fn extract_service_from_submission(
        &self,
        submission: &Submission,
        peer: &Peer,
        label: &str,
    ) -> Option<Service> {
        match self.services.get(submission.service_id()) {
            Ok(service) => {
                let workflow = match service.workflows.get(submission.workflow_id()) {
                    Some(w) => w,
                    None => {
                        tracing::error!("{label}: Workflow not found",);
                        return None;
                    }
                };

                if !matches!(workflow.submit, Submit::Aggregator { .. }) {
                    tracing::info!(
                        "{label}: Received submission for workflow not using Aggregator submission",
                    );
                    return None;
                }

                Some(service)
            }
            Err(_) => {
                match &peer {
                    Peer::Me => {
                        // this IS an error, we should never receive submissions from ourself for services we don't host
                        tracing::error!("{label}: Service not found for receive from myself",);
                    }
                    Peer::Other(_) => {
                        // this is NOT an error, peers can broadcast submissions for services we don't host
                        tracing::info!("{label}: Service not found for receive from peer",);
                    }
                }
                None
            }
        }
    }

    async fn handle_broadcast(&self, submission: &Submission) -> Result<(), AggregatorError> {
        // 1. Send to self for local processing
        self.aggregator_to_self_tx
            .send(AggregatorCommand::Receive {
                submission: submission.clone(),
                peer: Peer::Me,
            })
            .map_err(Box::new)?;

        // 2. Broadcast to P2P network if enabled
        let p2p_handle = self.p2p_handle.read().unwrap().clone();
        if let Some(handle) = p2p_handle {
            if let Err(e) = handle.publish(submission) {
                tracing::warn!("Failed to publish submission to P2P network: {:?}", e);
                // Don't fail the broadcast if P2P fails - local processing should continue
            } else {
                tracing::debug!(
                    "Published submission to P2P network: {}",
                    submission.label()
                );
            }
        }

        Ok(())
    }

    async fn handle_receive(
        &self,
        submission: Submission,
        service: Service,
    ) -> Result<(), AggregatorError> {
        // Note: Broadcasting to peers happens in handle_broadcast when we create our own submission.
        // When we receive from peers via P2P, it comes here with Peer::Other and gets processed locally.

        self.subsystem_to_dispatcher_tx
            .send(DispatcherCommand::AggregatorExecute {
                submission,
                service,
                kind: AggregatorExecuteKind::Standard,
            })
            .map_err(Box::new)?;

        Ok(())
    }

    /// Handle submission to blockchain with automatic retry mechanism
    ///
    /// This function implements the core retry logic for on-chain submissions:
    ///
    /// ## Queue Lifecycle
    /// - **Active**: Queue contains submissions waiting for quorum or retrying after errors
    /// - **Burned**: Successfully submitted on-chain, queue is marked completed
    ///
    /// ## Retry Mechanism
    /// When submission fails (insufficient quorum, transient errors, etc.):
    /// 1. Queue is saved with all accumulated submissions
    /// 2. Next submission (from P2P or local vector) loads the saved queue
    /// 3. Appends its own submission to the queue
    /// 4. Retries with all signatures combined
    ///
    /// ## P2P Interaction
    /// - P2P catch-up may deliver submissions before vectors are fully registered
    /// - SignerNotRegistered errors are treated as transient and trigger retry
    /// - Common scenario: PoA middleware does sequential vector registration,
    ///   causing a timing gap between P2P submission delivery and on-chain registration
    ///
    /// ## Important
    /// The queue passed to this function already has the current submission appended,
    /// so we must always save it on error to avoid losing submissions.
    async fn handle_submit_action(
        &self,
        submission: &Submission,
        service: &Service,
        queue_id: QuorumQueueId,
        queue: Vec<Submission>,
        action: SubmitAction,
    ) -> Result<(), AggregatorError> {
        // Running in a transaction keyed by chain to avoid nonce errors.
        //
        // The closure returns a flat `Result` (no `Option`): missing
        // credentials and missing chain config used to return
        // `Ok(None)` here, which the outer loop treated as
        // "skip without saving" — silently dropping the inbound
        // submission. They now produce typed `MissingCredential` /
        // `MissingChainConfig` errors which fall through to the
        // shared error arm below and call `save_quorum_queue`, so
        // submissions aren't lost while a sysadmin propagates config.
        let result: Result<AnyTransactionReceipt, AggregatorError> = self
            .chain_transaction
            .run(action.chain().clone(), {
                let _self = self.clone();
                let queue = queue.clone();
                let service_clone = service.clone();
                move || async move {
                    match action {
                        SubmitAction::Evm(action) => {
                            let client = _self.get_evm_client(&action.chain).await?;
                            _self.handle_action_submit_evm(client, &queue, action).await
                        }
                        SubmitAction::Cosmos(action) => {
                            let client = _self.get_cosmos_client(&action.chain).await?;
                            _self
                                .handle_action_submit_cosmos(client, &queue, action)
                                .await
                        }
                        SubmitAction::Stellar(action) => {
                            let signing_key = _self.get_stellar_signing_key(&action.chain)?;
                            _self
                                .handle_action_submit_stellar(
                                    signing_key,
                                    &service_clone,
                                    &queue,
                                    action,
                                )
                                .await
                        }
                    }
                }
            })
            .await;

        // Process the submission result and manage queue state
        // Three outcomes determine queue lifecycle:
        // 1. InsufficientQuorum: Save queue for retry when more vectors sign
        // 2. Success: Burn queue to prevent duplicate submissions
        // 3. Other errors: Save queue because we appended the submission above
        match &result {
            Err(AggregatorError::InsufficientQuorum {
                signer_weight,
                threshold_weight,
                total_weight,
            }) => {
                tracing::warn!(
                        "Aggregator: Insufficient quorum for {}: signer weight: {}, threshold weight: {}, total weight: {}",
                        submission.label(),
                        signer_weight,
                        threshold_weight,
                        total_weight
                    );

                // Save queue: Next submission from another vector will retry with accumulated signatures
                self.save_quorum_queue(queue_id, queue).await?;
            }
            Ok(tx_resp) => {
                tracing::info!(
                    "Aggregator: Successfully submitted on-chain for {}: tx hash: {}",
                    submission.label(),
                    tx_resp.tx_hash()
                );
                // Burn queue: Mark as completed to prevent duplicate on-chain submissions
                self.burn_quorum_queue(queue_id).await?;
            }

            Err(err) => {
                // Three categories collapse into "warn & save for
                // retry"; everything else is logged at error level
                // for investigation. The retry-on-save behavior is
                // identical for all three — only the log level
                // differs, since these three are expected/transient
                // and the rest are not.
                match err {
                    // SignerNotRegistered: vectors are still being
                    // registered on-chain. Common during startup,
                    // especially with PoA middleware whose sequential
                    // docker-exec calls are slow. Both EVM and
                    // Stellar paths produce this variant — EVM
                    // detects the Solidity selector 0x3dda1739,
                    // Stellar detects contract error code #302.
                    AggregatorError::SignerNotRegistered(_) => {
                        tracing::warn!(
                            "Aggregator: Signer not registered yet for submission {}. Will retry when vectors complete registration.",
                            submission.label()
                        );
                    }
                    // MissingCredential: sysadmin hasn't finished
                    // propagating config. Save the queue so the
                    // submission isn't lost while they fix it.
                    AggregatorError::MissingCredential {
                        chain_kind, chain, ..
                    } => {
                        tracing::warn!(
                            "Aggregator: Missing {} credential for chain {} for submission {}. Saving queue; will retry once credential is configured (typically requires a node restart).",
                            chain_kind,
                            chain,
                            submission.label()
                        );
                    }
                    // MissingChainConfig: chain referenced by the
                    // submission isn't in the chain-config registry.
                    // Same transient framing — save the queue so a
                    // sysadmin who adds the chain doesn't lose the
                    // pending submission.
                    AggregatorError::MissingChainConfig(chain) => {
                        tracing::warn!(
                            "Aggregator: Chain config not found for chain {} for submission {}. Saving queue; will retry once the chain is configured.",
                            chain,
                            submission.label()
                        );
                    }
                    _ => {
                        tracing::error!(
                            "Aggregator: Error submitting on-chain for submission {}: {:?}",
                            submission.label(),
                            err
                        );
                    }
                }
                // IMPORTANT: Always save the queue on error
                // We appended the current submission above, so failing to save it would lose this submission
                // When the next submission arrives (from P2P or vector), it will:
                // 1. Load this saved queue with accumulated submissions
                // 2. Append its own submission
                // 3. Retry submission with all signatures
                // This implements automatic retry for transient errors like SignerNotRegistered
                self.save_quorum_queue(queue_id, queue).await?;
            }
        }

        self.subsystem_to_dispatcher_tx
            .send(DispatcherCommand::AggregatorExecute {
                submission: submission.clone(),
                service: service.clone(),
                kind: AggregatorExecuteKind::SubmitCallback {
                    result: result
                        .map(|tx_resp| match tx_resp {
                            AnyTransactionReceipt::Evm(transaction_receipt) => {
                                AnyTxHash::Evm(transaction_receipt.transaction_hash.to_vec())
                            }
                            AnyTransactionReceipt::Cosmos(tx_hash) => AnyTxHash::Cosmos(tx_hash),
                            AnyTransactionReceipt::Stellar(tx_hash) => AnyTxHash::Stellar(tx_hash),
                        })
                        .map_err(|err| err.to_string()),
                },
            })
            .map_err(Box::new)?;

        Ok(())
    }

    /// Resolve (and cache) the EVM signing client for `chain`.
    ///
    /// Returns a typed error rather than `Option` so the dispatch loop
    /// routes credential / chain-config misses through the normal
    /// `save_quorum_queue` path. Prior to that change, missing config
    /// returned `Ok(None)` and the dispatch loop silently dropped the
    /// inbound submission.
    async fn get_evm_client(&self, chain: &ChainKey) -> Result<EvmSigningClient, AggregatorError> {
        {
            let client = self
                .evm_submission_clients
                .read()
                .unwrap()
                .get(chain)
                .cloned();

            if let Some(client) = client {
                return Ok(client);
            }
        };

        let credential = self
            .config
            .aggregator_evm_credential
            .as_ref()
            .ok_or_else(|| AggregatorError::MissingCredential {
                chain_kind: "EVM",
                chain: chain.clone(),
                detail: "config.aggregator_evm_credential is unset".to_string(),
            })?;

        let chain_config = self
            .config
            .chains
            .read()
            .unwrap()
            .get_chain(chain)
            .ok_or_else(|| AggregatorError::MissingChainConfig(chain.clone()))?
            .to_evm_config()?;

        let client_config = chain_config.signing_client_config(credential.clone())?;

        let client = EvmSigningClient::new(client_config)
            .await
            .map_err(AggregatorError::CreateEvmClient)?;

        {
            let clients = &mut self.evm_submission_clients.write().unwrap();
            clients.insert(chain.clone(), client.clone());
        }

        Ok(client)
    }

    /// Parse `aggregator_stellar_credential` (an ed25519 secret in `S...`
    /// strkey form) into a signing key. Lightweight enough that we don't
    /// bother caching: callers hit this once per submission.
    ///
    /// Returns a typed error (rather than `Option`) for the same reason
    /// as `get_evm_client` — missing/unparseable credentials should
    /// route through the dispatch loop's save-and-retry path, not the
    /// silent-drop path.
    fn get_stellar_signing_key(
        &self,
        chain: &ChainKey,
    ) -> Result<ed25519_dalek::SigningKey, AggregatorError> {
        let credential = self
            .config
            .aggregator_stellar_credential
            .as_ref()
            .ok_or_else(|| AggregatorError::MissingCredential {
                chain_kind: "Stellar",
                chain: chain.clone(),
                detail: "config.aggregator_stellar_credential is unset".to_string(),
            })?;
        let secret = stellar_strkey::ed25519::PrivateKey::from_string(credential.as_str())
            .map_err(|e| AggregatorError::MissingCredential {
                chain_kind: "Stellar",
                chain: chain.clone(),
                detail: format!("strkey parse failed: {e:?}"),
            })?;
        Ok(ed25519_dalek::SigningKey::from_bytes(&secret.0))
    }

    /// Resolve (and cache) the Cosmos signing client for `chain`.
    ///
    /// Returns a typed error rather than `Option` for the same reason
    /// as `get_evm_client`.
    async fn get_cosmos_client(
        &self,
        chain: &ChainKey,
    ) -> Result<layer_climb::prelude::SigningClient, AggregatorError> {
        {
            let client = self
                .cosmos_submission_clients
                .read()
                .unwrap()
                .get(chain)
                .cloned();

            if let Some(client) = client {
                return Ok(client);
            }
        };

        let credential = self
            .config
            .aggregator_cosmos_credential
            .as_ref()
            .ok_or_else(|| AggregatorError::MissingCredential {
                chain_kind: "Cosmos",
                chain: chain.clone(),
                detail: "config.aggregator_cosmos_credential is unset".to_string(),
            })?;

        let chain_config = self
            .config
            .chains
            .read()
            .unwrap()
            .get_chain(chain)
            .ok_or_else(|| AggregatorError::MissingChainConfig(chain.clone()))?
            .to_cosmos_config()?;

        let key_signer =
            KeySigner::new_mnemonic_str(credential, None).map_err(AggregatorError::CosmosClient)?;

        let client = SigningClient::new(chain_config.into(), key_signer, None)
            .await
            .map_err(AggregatorError::CosmosClient)?;

        {
            let clients = &mut self.cosmos_submission_clients.write().unwrap();
            clients.insert(chain.clone(), client.clone());
        }

        Ok(client)
    }
}

impl Clone for Aggregator {
    fn clone(&self) -> Self {
        Self {
            metrics: self.metrics.clone(),
            storage: self.storage.clone(),
            config: self.config.clone(),
            services: self.services.clone(),
            dispatcher_to_aggregator_rx: self.dispatcher_to_aggregator_rx.clone(),
            aggregator_to_self_tx: self.aggregator_to_self_tx.clone(),
            subsystem_to_dispatcher_tx: self.subsystem_to_dispatcher_tx.clone(),
            evm_submission_clients: self.evm_submission_clients.clone(),
            cosmos_submission_clients: self.cosmos_submission_clients.clone(),
            queue_transaction: self.queue_transaction.clone(),
            chain_transaction: self.chain_transaction.clone(),
            p2p_handle: self.p2p_handle.clone(),
            // Clones are not primary - only the original instance is
            is_primary: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Drop for Aggregator {
    fn drop(&mut self) {
        // Only log when the primary instance is dropped (actual subsystem shutdown),
        // not when clones used for async tasks are dropped
        if self.is_primary.load(Ordering::Relaxed) {
            tracing::warn!("Dropping Aggregator subsystem");
        }
    }
}

// ── Startup credential validation ────────────────────────────────
//
// Each `validate_*_credential` helper is `Option<&Credential>` →
// `Result<(), AggregatorError>`:
//   - `None` is a no-op (intentional opt-out — the operator hasn't
//     configured submission for this chain kind).
//   - `Some(cred)` is parsed offline (no network calls) using the
//     same parsing primitives the lazy submission paths use, so a
//     successful startup validation guarantees the lazy path will
//     also succeed (modulo network).
//
// Splitting them out (vs. one big function) keeps each individually
// unit-testable without constructing a full `Aggregator`.

fn validate_evm_credential(
    credential: Option<&warpdrive_types::Credential>,
) -> Result<(), AggregatorError> {
    let Some(cred) = credential else {
        return Ok(());
    };
    // `make_signer` accepts either a `0x`-prefixed private key or a
    // BIP-39 mnemonic. Either form is parsed entirely offline.
    utils::evm_client::signing::make_signer(cred, None).map_err(|e| {
        AggregatorError::InvalidStartupCredential {
            chain_kind: "EVM",
            detail: format!("{e:?}"),
        }
    })?;
    Ok(())
}

fn validate_cosmos_credential(
    credential: Option<&warpdrive_types::Credential>,
) -> Result<(), AggregatorError> {
    let Some(cred) = credential else {
        return Ok(());
    };
    KeySigner::new_mnemonic_str(cred, None).map_err(|e| {
        AggregatorError::InvalidStartupCredential {
            chain_kind: "Cosmos",
            detail: format!("{e:?}"),
        }
    })?;
    Ok(())
}

fn validate_stellar_credential(
    credential: Option<&warpdrive_types::Credential>,
) -> Result<(), AggregatorError> {
    let Some(cred) = credential else {
        return Ok(());
    };
    stellar_strkey::ed25519::PrivateKey::from_string(cred.as_str()).map_err(|e| {
        AggregatorError::InvalidStartupCredential {
            chain_kind: "Stellar",
            detail: format!("strkey parse failed: {e:?}"),
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod startup_credential_validation_tests {
    use super::*;
    use warpdrive_types::Credential;

    // ── EVM ──────────────────────────────────────────────────────

    #[test]
    fn evm_none_is_ok() {
        assert!(validate_evm_credential(None).is_ok());
    }

    #[test]
    fn evm_valid_hex_private_key_is_ok() {
        // 32-byte secp256k1 key, 0x-prefixed hex.
        let cred = Credential::new(
            "0x1111111111111111111111111111111111111111111111111111111111111111".to_string(),
        );
        assert!(validate_evm_credential(Some(&cred)).is_ok());
    }

    #[test]
    fn evm_valid_mnemonic_is_ok() {
        // BIP-39 test vector — well-formed, never use in production.
        let cred = Credential::new(
            "test test test test test test test test test test test junk".to_string(),
        );
        assert!(validate_evm_credential(Some(&cred)).is_ok());
    }

    #[test]
    fn evm_garbage_bails_at_startup() {
        let cred = Credential::new("not-a-key-or-mnemonic".to_string());
        let err = validate_evm_credential(Some(&cred)).unwrap_err();
        assert!(
            matches!(
                err,
                AggregatorError::InvalidStartupCredential {
                    chain_kind: "EVM",
                    ..
                }
            ),
            "expected InvalidStartupCredential{{kind=EVM}}, got {err:?}",
        );
    }

    // ── Cosmos ───────────────────────────────────────────────────

    #[test]
    fn cosmos_none_is_ok() {
        assert!(validate_cosmos_credential(None).is_ok());
    }

    #[test]
    fn cosmos_valid_mnemonic_is_ok() {
        let cred = Credential::new(
            "test test test test test test test test test test test junk".to_string(),
        );
        assert!(validate_cosmos_credential(Some(&cred)).is_ok());
    }

    #[test]
    fn cosmos_garbage_bails_at_startup() {
        let cred = Credential::new("not a mnemonic".to_string());
        let err = validate_cosmos_credential(Some(&cred)).unwrap_err();
        assert!(
            matches!(
                err,
                AggregatorError::InvalidStartupCredential {
                    chain_kind: "Cosmos",
                    ..
                }
            ),
            "expected InvalidStartupCredential{{kind=Cosmos}}, got {err:?}",
        );
    }

    // ── Stellar ──────────────────────────────────────────────────

    #[test]
    fn stellar_none_is_ok() {
        assert!(validate_stellar_credential(None).is_ok());
    }

    #[test]
    fn stellar_valid_strkey_is_ok() {
        // Generate a fresh strkey on the fly so we don't depend on a
        // brittle hardcoded test vector. SEP-0023 ed25519 secret seed
        // strkeys start with 'S'; constructing one and round-tripping
        // through the parser is what we want validate_stellar_credential
        // to accept.
        let secret = stellar_strkey::ed25519::PrivateKey([7u8; 32]);
        // `format!` instead of `.to_string()`: stellar-strkey 0.0.16
        // added an inherent `to_string()` returning `heapless::String`
        // for no_std consumers, which shadows the `ToString` trait impl.
        let cred = Credential::new(format!("{secret}"));
        assert!(
            validate_stellar_credential(Some(&cred)).is_ok(),
            "round-tripped strkey {} failed validation",
            secret
        );
    }

    #[test]
    fn stellar_garbage_bails_at_startup() {
        let cred = Credential::new("not a stellar strkey".to_string());
        let err = validate_stellar_credential(Some(&cred)).unwrap_err();
        assert!(
            matches!(
                err,
                AggregatorError::InvalidStartupCredential {
                    chain_kind: "Stellar",
                    ..
                }
            ),
            "expected InvalidStartupCredential{{kind=Stellar}}, got {err:?}",
        );
    }
}
