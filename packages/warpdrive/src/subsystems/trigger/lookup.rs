use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{atomic::AtomicUsize, Arc, RwLock},
};

use bimap::BiMap;
use utils::telemetry::TriggerMetrics;
use warpdrive_types::{
    AtProtoAction, ByteArray, ChainKey, ServiceId, Trigger, TriggerConfig, WorkflowId,
};

use crate::{
    services::Services,
    subsystems::trigger::{
        error::TriggerError,
        schedulers::{block_scheduler::BlockIntervalState, cron_scheduler::CronIntervalState},
        streams::stellar_stream::{
            controller::StellarStreamController,
            filters::{StellarEventFilter, StellarRpcId},
        },
    },
};

use super::schedulers::{block_scheduler::BlockSchedulers, cron_scheduler::CronScheduler};

#[allow(clippy::type_complexity)]
pub struct LookupMaps {
    /// single lookup for all triggers (in theory, can be more than just task queue addr)
    trigger_configs: Arc<RwLock<BTreeMap<LookupId, TriggerConfig>>>,
    services: Services,
    metrics: TriggerMetrics,
    /// lookup id by (chain name, contract event address, event type)
    pub triggers_by_cosmos_contract_event: Arc<
        RwLock<HashMap<(ChainKey, layer_climb::prelude::CosmosAddr, String), HashSet<LookupId>>>,
    >,
    /// lookup id by (chain id, contract event address, event hash)
    pub triggers_by_evm_contract_event: Arc<
        RwLock<HashMap<(ChainKey, alloy_primitives::Address, ByteArray<32>), HashSet<LookupId>>>,
    >,
    /// To avoid the problem of matching on wildcards we track a proprietary ID in
    /// the stream client and look up the workflow by that. The mapping is 1:1
    /// because every `add_filter` call mints a fresh `StellarRpcId` from a SlotMap,
    /// even when two workflows register an identical filter. We use a `BiMap` so
    /// removal can find the specific rpc id that belongs to a given lookup id —
    /// without it we'd have to query the stream client by filter, which returns
    /// the rpc ids of *all* workflows sharing that filter and would wipe siblings.
    pub triggers_by_stellar_contract_event: Arc<RwLock<BiMap<StellarRpcId, LookupId>>>,
    /// lookup id by (collection, optional repo_did, optional action) for exact matches
    pub triggers_by_atproto_event_exact:
        Arc<RwLock<HashMap<(String, Option<String>, Option<AtProtoAction>), HashSet<LookupId>>>>,
    /// lookup id by (collection pattern, optional repo_did, optional action) for wildcard matches
    pub triggers_by_atproto_event_pattern:
        Arc<RwLock<HashMap<(String, Option<String>, Option<AtProtoAction>), HashSet<LookupId>>>>,
    // ServiceId <-> ServiceManager address
    pub service_manager: Arc<RwLock<BiMap<ServiceId, layer_climb::prelude::Address>>>,
    /// Efficient block schedulers (one per chain) for block interval triggers
    pub block_schedulers: BlockSchedulers,
    /// lookup id by service id -> workflow id
    pub triggers_by_service_workflow:
        Arc<RwLock<BTreeMap<ServiceId, BTreeMap<WorkflowId, LookupId>>>>,
    /// latest lookup_id
    pub lookup_id: Arc<AtomicUsize>,
    /// cron scheduler
    pub cron_scheduler: CronScheduler,
}

impl LookupMaps {
    pub fn new(services: Services, metrics: TriggerMetrics) -> Self {
        Self {
            trigger_configs: Arc::new(RwLock::new(BTreeMap::new())),
            lookup_id: Arc::new(AtomicUsize::new(0)),
            triggers_by_cosmos_contract_event: Arc::new(RwLock::new(HashMap::new())),
            triggers_by_evm_contract_event: Arc::new(RwLock::new(HashMap::new())),
            triggers_by_stellar_contract_event: Arc::new(RwLock::new(BiMap::new())),
            triggers_by_atproto_event_exact: Arc::new(RwLock::new(HashMap::new())),
            triggers_by_atproto_event_pattern: Arc::new(RwLock::new(HashMap::new())),
            block_schedulers: BlockSchedulers::default(),
            triggers_by_service_workflow: Arc::new(RwLock::new(BTreeMap::new())),
            service_manager: Arc::new(RwLock::new(BiMap::new())),
            cron_scheduler: CronScheduler::default(),
            services,
            metrics,
        }
    }

    pub fn get_trigger_config(&self, lookup_id: LookupId) -> Option<TriggerConfig> {
        let trigger_configs = self.trigger_configs.read().unwrap();
        let trigger_config = match trigger_configs.get(&lookup_id).cloned() {
            Some(config) => config,
            None => {
                self.metrics
                    .increment_total_errors("trigger config not found");
                tracing::error!("Trigger config not found for lookup_id {}", lookup_id);
                return None;
            }
        };

        match self.services.is_active(&trigger_config.service_id) {
            true => Some(trigger_config),
            _ => None,
        }
    }

    pub fn get_trigger_configs<'a>(
        &self,
        lookup_ids: impl IntoIterator<Item = &'a LookupId>,
    ) -> Vec<TriggerConfig> {
        let trigger_configs = self.trigger_configs.read().unwrap();
        lookup_ids
            .into_iter()
            .filter_map(|id| match trigger_configs.get(id) {
                Some(config) => Some(config.clone()),
                None => {
                    self.metrics
                        .increment_total_errors("trigger config not found");
                    tracing::error!("Trigger config not found for lookup_id {}", id);
                    None
                }
            })
            .filter(|config| self.services.is_active(&config.service_id))
            .collect()
    }

    pub fn add_service(
        &self,
        service: &warpdrive_types::Service,
        stellar_controllers: &Arc<std::sync::RwLock<HashMap<ChainKey, StellarStreamController>>>,
    ) -> Result<(), TriggerError> {
        let manager_address: layer_climb::prelude::Address = service.manager.address();

        self.service_manager
            .write()
            .unwrap()
            .insert(service.id(), manager_address);

        for (id, workflow) in &service.workflows {
            let trigger = TriggerConfig {
                service_id: service.id(),
                workflow_id: id.clone(),
                trigger: workflow.trigger.clone(),
            };
            self.add_trigger(trigger, stellar_controllers)?;
        }

        Ok(())
    }

    pub fn add_trigger(
        &self,
        config: TriggerConfig,
        stellar_controllers: &Arc<std::sync::RwLock<HashMap<ChainKey, StellarStreamController>>>,
    ) -> Result<(), TriggerError> {
        // get the next lookup id
        let lookup_id = self
            .lookup_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        match config.trigger.clone() {
            Trigger::EvmContractEvent {
                address,
                chain,
                event_hash,
            } => {
                let key = (chain.clone(), address, event_hash);
                self.triggers_by_evm_contract_event
                    .write()
                    .unwrap()
                    .entry(key)
                    .or_default()
                    .insert(lookup_id);
            }
            Trigger::CosmosContractEvent {
                address,
                chain,
                event_type,
            } => {
                let key = (chain.clone(), address.clone(), event_type.clone());
                self.triggers_by_cosmos_contract_event
                    .write()
                    .unwrap()
                    .entry(key)
                    .or_default()
                    .insert(lookup_id);
            }
            Trigger::StellarContractEvent {
                chain,
                contract_id,
                topic_segments,
            } => {
                let filter =
                    StellarEventFilter::new(contract_id.to_string(), topic_segments.clone())?;

                let controller = stellar_controllers.read().unwrap();
                let client = &controller
                    .get(&chain)
                    .as_ref()
                    .ok_or_else(|| TriggerError::StellarMissingClient(chain.clone()))?
                    .client;

                let rpc_id =
                    client.update_event_filters(|filters| filters.add_filter(filter.clone()))?;

                // BiMap::insert silently overwrites on either-side collision, but
                // both keys are freshly minted here (rpc_id from a SlotMap, lookup_id
                // from an AtomicUsize) so a collision is structurally impossible.
                self.triggers_by_stellar_contract_event
                    .write()
                    .unwrap()
                    .insert(rpc_id, lookup_id);
            }
            Trigger::BlockInterval {
                chain,
                n_blocks,
                start_block,
                end_block,
            } => {
                self.block_schedulers
                    .entry(chain.clone())
                    .or_default()
                    .add_trigger(BlockIntervalState::new(
                        lookup_id,
                        n_blocks,
                        start_block.map(Into::into),
                        end_block.map(Into::into),
                    ))?;
            }
            Trigger::Cron {
                schedule,
                start_time,
                end_time,
            } => {
                // Add directly to the cron scheduler
                self.cron_scheduler
                    .lock()
                    .unwrap()
                    .add_trigger(CronIntervalState::new(
                        lookup_id, &schedule, start_time, end_time,
                    )?)?;
            }
            Trigger::AtProtoEvent {
                collection,
                repo_did,
                action,
            } => {
                let key = (collection.clone(), repo_did.clone(), action.clone());
                // Use separate collections so the pattern-matching path only iterates over patterns
                if collection.contains('*') {
                    self.triggers_by_atproto_event_pattern
                        .write()
                        .unwrap()
                        .entry(key)
                        .or_default()
                        .insert(lookup_id);
                } else {
                    self.triggers_by_atproto_event_exact
                        .write()
                        .unwrap()
                        .entry(key)
                        .or_default()
                        .insert(lookup_id);
                }
            }
            Trigger::Manual => {}
        }

        // adding it to our lookups is the same, regardless of type
        self.triggers_by_service_workflow
            .write()
            .unwrap()
            .entry(config.service_id.clone())
            .or_default()
            .insert(config.workflow_id.clone(), lookup_id);

        self.trigger_configs
            .write()
            .unwrap()
            .insert(lookup_id, config);

        Ok(())
    }

    pub fn remove_workflow(
        &self,
        service_id: ServiceId,
        workflow_id: WorkflowId,
        stellar_controllers: &Arc<std::sync::RwLock<HashMap<ChainKey, StellarStreamController>>>,
    ) -> Result<(), TriggerError> {
        let mut service_lock = self.triggers_by_service_workflow.write().unwrap();

        let workflow_map = service_lock
            .get_mut(&service_id)
            .ok_or_else(|| TriggerError::NoSuchService(service_id.clone()))?;

        // first remove it from services
        let lookup_id = workflow_map
            .remove(&workflow_id)
            .ok_or(TriggerError::NoSuchWorkflow(service_id, workflow_id))?;

        // Get the trigger type to know which scheduler to remove from
        let trigger_type = {
            let trigger_configs = self.trigger_configs.read().unwrap();
            trigger_configs
                .get(&lookup_id)
                .map(|config| config.trigger.clone())
        };

        // Remove from the appropriate collection based on trigger type
        if let Some(trigger) = trigger_type {
            match trigger {
                Trigger::EvmContractEvent {
                    address,
                    chain,
                    event_hash,
                } => {
                    let mut lock = self.triggers_by_evm_contract_event.write().unwrap();
                    if let Some(set) = lock.get_mut(&(chain.clone(), address, event_hash)) {
                        set.remove(&lookup_id);
                        if set.is_empty() {
                            lock.remove(&(chain, address, event_hash));
                        }
                    }
                }
                Trigger::CosmosContractEvent {
                    address,
                    chain,
                    event_type,
                } => {
                    let mut lock = self.triggers_by_cosmos_contract_event.write().unwrap();
                    if let Some(set) =
                        lock.get_mut(&(chain.clone(), address.clone(), event_type.clone()))
                    {
                        set.remove(&lookup_id);
                        if set.is_empty() {
                            lock.remove(&(chain, address, event_type));
                        }
                    }
                }
                Trigger::StellarContractEvent { chain, .. } => {
                    // Resolve this workflow's specific rpc id via the BiMap rather
                    // than rebuilding the filter and asking the client — the client
                    // dedupes by filter and would return rpc ids for sibling
                    // workflows too, which we must not touch.
                    let mut lock = self.triggers_by_stellar_contract_event.write().unwrap();
                    if let Some((rpc_id, _)) = lock.remove_by_right(&lookup_id) {
                        // Drop the filter from the stream client as well so its
                        // poller stops requesting events for a workflow we just
                        // removed. The client's SlotMap-backed dedupe list keeps
                        // the underlying filter alive while any sibling rpc id
                        // still references it.
                        let controller = stellar_controllers.read().unwrap();
                        let client = &controller
                            .get(&chain)
                            .as_ref()
                            .ok_or_else(|| TriggerError::StellarMissingClient(chain.clone()))?
                            .client;
                        client.update_event_filters(|filters| filters.remove_filter_by_id(rpc_id));
                    }
                }
                Trigger::BlockInterval { chain, .. } => {
                    // Remove from block scheduler
                    if let Some(mut scheduler) = self.block_schedulers.get_mut(&chain) {
                        scheduler.remove_trigger(lookup_id);
                    }
                }
                Trigger::Cron { .. } => {
                    // Remove from cron scheduler
                    self.cron_scheduler
                        .lock()
                        .unwrap()
                        .remove_trigger(lookup_id);
                }
                Trigger::Manual => {}
                Trigger::AtProtoEvent {
                    collection,
                    repo_did,
                    action,
                } => {
                    let key = (collection.clone(), repo_did.clone(), action.clone());
                    if collection.contains('*') {
                        let mut lock = self.triggers_by_atproto_event_pattern.write().unwrap();
                        if let Some(set) = lock.get_mut(&key) {
                            set.remove(&lookup_id);
                            if set.is_empty() {
                                lock.remove(&key);
                            }
                        }
                    } else {
                        let mut lock = self.triggers_by_atproto_event_exact.write().unwrap();
                        if let Some(set) = lock.get_mut(&key) {
                            set.remove(&lookup_id);
                            if set.is_empty() {
                                lock.remove(&key);
                            }
                        }
                    }
                }
            }
        }

        // Remove from trigger_configs
        self.trigger_configs.write().unwrap().remove(&lookup_id);

        Ok(())
    }

    pub fn remove_service(
        &self,
        service_id: warpdrive_types::ServiceId,
        stellar_controllers: &Arc<std::sync::RwLock<HashMap<ChainKey, StellarStreamController>>>,
    ) -> Result<(), TriggerError> {
        let mut trigger_configs = self.trigger_configs.write().unwrap();
        let mut triggers_by_evm_contract_event =
            self.triggers_by_evm_contract_event.write().unwrap();
        let mut triggers_by_cosmos_contract_event =
            self.triggers_by_cosmos_contract_event.write().unwrap();
        let mut triggers_by_stellar_contract_event =
            self.triggers_by_stellar_contract_event.write().unwrap();
        let mut triggers_by_atproto_event_exact =
            self.triggers_by_atproto_event_exact.write().unwrap();
        let mut triggers_by_atproto_event_pattern =
            self.triggers_by_atproto_event_pattern.write().unwrap();
        let mut triggers_by_service_workflow_lock =
            self.triggers_by_service_workflow.write().unwrap();

        // Remove the service manager
        self.service_manager
            .write()
            .unwrap()
            .remove_by_left(&service_id);

        if let Some(workflow_map) = triggers_by_service_workflow_lock.get(&service_id) {
            // Collect all lookup IDs to be removed
            let lookup_ids: Vec<LookupId> = workflow_map.values().copied().collect();

            // Remove triggers from all collections
            for lookup_id in &lookup_ids {
                if let Some(config) = trigger_configs.get(lookup_id) {
                    match &config.trigger {
                        Trigger::EvmContractEvent {
                            address,
                            chain,
                            event_hash,
                        } => {
                            if let Some(set) = triggers_by_evm_contract_event.get_mut(&(
                                chain.clone(),
                                *address,
                                *event_hash,
                            )) {
                                set.remove(lookup_id);
                                if set.is_empty() {
                                    triggers_by_evm_contract_event.remove(&(
                                        chain.clone(),
                                        *address,
                                        *event_hash,
                                    ));
                                }
                            }
                        }
                        Trigger::CosmosContractEvent {
                            address,
                            chain,
                            event_type,
                        } => {
                            if let Some(set) = triggers_by_cosmos_contract_event.get_mut(&(
                                chain.clone(),
                                address.clone(),
                                event_type.clone(),
                            )) {
                                set.remove(lookup_id);
                                if set.is_empty() {
                                    triggers_by_cosmos_contract_event.remove(&(
                                        chain.clone(),
                                        address.clone(),
                                        event_type.clone(),
                                    ));
                                }
                            }
                        }
                        Trigger::StellarContractEvent { chain, .. } => {
                            // Per-workflow removal: see the matching arm in
                            // `remove_workflow` for why we look up by lookup id
                            // instead of by filter. Each iteration here removes
                            // exactly one rpc id from the stream client, so a
                            // shared filter only disappears once the last
                            // workflow referencing it is gone.
                            if let Some((rpc_id, _)) =
                                triggers_by_stellar_contract_event.remove_by_right(lookup_id)
                            {
                                let controller = stellar_controllers.read().unwrap();
                                let client = &controller
                                    .get(chain)
                                    .as_ref()
                                    .ok_or_else(|| {
                                        TriggerError::StellarMissingClient(chain.clone())
                                    })?
                                    .client;
                                client.update_event_filters(|filters| {
                                    filters.remove_filter_by_id(rpc_id)
                                });
                            }
                        }
                        Trigger::BlockInterval { chain, .. } => {
                            // Remove from block scheduler
                            if let Some(mut scheduler) = self.block_schedulers.get_mut(chain) {
                                scheduler.remove_trigger(*lookup_id);
                            }
                        }
                        Trigger::Cron { .. } => {
                            self.cron_scheduler
                                .lock()
                                .unwrap()
                                .remove_trigger(*lookup_id);
                        }
                        Trigger::Manual => {}
                        Trigger::AtProtoEvent {
                            collection,
                            repo_did,
                            action,
                        } => {
                            let key = (collection.clone(), repo_did.clone(), action.clone());
                            if collection.contains('*') {
                                if let Some(set) = triggers_by_atproto_event_pattern.get_mut(&key) {
                                    set.remove(lookup_id);
                                    if set.is_empty() {
                                        triggers_by_atproto_event_pattern.remove(&key);
                                    }
                                }
                            } else if let Some(set) = triggers_by_atproto_event_exact.get_mut(&key)
                            {
                                set.remove(lookup_id);
                                if set.is_empty() {
                                    triggers_by_atproto_event_exact.remove(&key);
                                }
                            }
                        }
                    }
                }
            }

            // Remove all trigger configs
            for lookup_id in &lookup_ids {
                trigger_configs.remove(lookup_id);
            }

            // Remove from service_workflow_lookup_map
            triggers_by_service_workflow_lock.remove(&service_id);
        }

        Ok(())
    }

    pub fn configs_for_service(
        &self,
        service_id: ServiceId,
    ) -> Result<Vec<TriggerConfig>, TriggerError> {
        let mut triggers = Vec::new();

        let triggers_by_service_workflow_lock = self.triggers_by_service_workflow.read().unwrap();
        let trigger_configs = self.trigger_configs.read().unwrap();

        let workflow_map = triggers_by_service_workflow_lock
            .get(&service_id)
            .ok_or(TriggerError::NoSuchService(service_id))?;

        for lookup_id in workflow_map.values() {
            let trigger_config = trigger_configs
                .get(lookup_id)
                .ok_or(TriggerError::NoSuchTriggerData(*lookup_id))?;
            triggers.push(trigger_config.clone());
        }

        Ok(triggers)
    }
}

pub type LookupId = usize;

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::ScVal;
    use utils::{storage::db::WavsDb, telemetry::TriggerMetrics};
    use warpdrive_types::{StellarChainConfig, StellarTopicSegment};

    fn make_lookup_maps() -> LookupMaps {
        let db = WavsDb::new().unwrap();
        let services = crate::services::Services::new(db);
        let metrics = TriggerMetrics::new(opentelemetry::global::meter("test"));
        LookupMaps::new(services, metrics)
    }

    fn make_controllers(
        chain: &ChainKey,
    ) -> Arc<RwLock<HashMap<ChainKey, StellarStreamController>>> {
        // The rpc_url is never dialed because the controller's poller is not
        // started in this test — only `update_event_filters` is exercised.
        let config = StellarChainConfig {
            chain_id: chain.id.clone(),
            chain_poll_interval_ms: 1000,
            rpc_url: "http://localhost:0".to_string(),
            network_passphrase: "Standalone Network ; February 2017".to_string(),
            friendbot_url: None,
        };
        let controller = StellarStreamController::new(config).unwrap();
        let mut map = HashMap::new();
        map.insert(chain.clone(), controller);
        Arc::new(RwLock::new(map))
    }

    fn shared_filter_trigger(chain: &ChainKey) -> Trigger {
        Trigger::StellarContractEvent {
            chain: chain.clone(),
            contract_id: "CABCD".to_string(),
            topic_segments: vec![StellarTopicSegment::Exact(ScVal::Symbol(
                "transfer".try_into().unwrap(),
            ))],
        }
    }

    /// Regression: removing one workflow used to wipe every workflow that shared
    /// its Stellar filter, because removal queried the stream client by filter
    /// (`get_rpc_ids_for_filter`) and dumped every rpc id it returned.
    #[test]
    fn remove_workflow_leaves_siblings_with_shared_stellar_filter() {
        let chain: ChainKey = "stellar:testnet".parse().unwrap();
        let lookup_maps = make_lookup_maps();
        let controllers = make_controllers(&chain);
        let trigger = shared_filter_trigger(&chain);

        let service_a = ServiceId::hash(b"service_a");
        let service_b = ServiceId::hash(b"service_b");
        let workflow_a: WorkflowId = "workflow-a".parse().unwrap();
        let workflow_b: WorkflowId = "workflow-b".parse().unwrap();

        for (service_id, workflow_id) in [
            (service_a.clone(), workflow_a.clone()),
            (service_b.clone(), workflow_b.clone()),
        ] {
            lookup_maps
                .add_trigger(
                    TriggerConfig {
                        service_id,
                        workflow_id,
                        trigger: trigger.clone(),
                    },
                    &controllers,
                )
                .unwrap();
        }
        assert_eq!(
            lookup_maps
                .triggers_by_stellar_contract_event
                .read()
                .unwrap()
                .len(),
            2
        );

        lookup_maps
            .remove_workflow(service_a.clone(), workflow_a.clone(), &controllers)
            .unwrap();

        // Lookup map still has B and only B.
        let lookup_lock = lookup_maps
            .triggers_by_stellar_contract_event
            .read()
            .unwrap();
        assert_eq!(lookup_lock.len(), 1, "removing A must not touch B");
        let surviving_lookup_id = *lookup_lock.right_values().next().unwrap();
        let by_service = lookup_maps.triggers_by_service_workflow.read().unwrap();
        assert!(
            by_service.get(&service_a).is_none_or(|m| m.is_empty()),
            "A should be cleared from triggers_by_service_workflow"
        );
        assert_eq!(
            by_service
                .get(&service_b)
                .unwrap()
                .get(&workflow_b)
                .copied(),
            Some(surviving_lookup_id),
        );

        // Stream client also keeps exactly one rpc id alive — A's filter handle
        // was removed, but B still has one.
        let controllers_lock = controllers.read().unwrap();
        let filters = controllers_lock
            .get(&chain)
            .unwrap()
            .client
            .clone_event_filters();
        let total_rpc_ids: usize = filters
            .get_all_filters_with_ids()
            .values()
            .map(|ids| ids.len())
            .sum();
        assert_eq!(
            total_rpc_ids, 1,
            "stream client should keep exactly one rpc id (B's) after A is removed"
        );
    }

    /// Same bug, same fix, but for `remove_service` (the bulk removal path).
    #[test]
    fn remove_service_leaves_siblings_with_shared_stellar_filter() {
        let chain: ChainKey = "stellar:testnet".parse().unwrap();
        let lookup_maps = make_lookup_maps();
        let controllers = make_controllers(&chain);
        let trigger = shared_filter_trigger(&chain);

        let service_a = ServiceId::hash(b"service_a");
        let service_b = ServiceId::hash(b"service_b");

        for service_id in [service_a.clone(), service_b.clone()] {
            lookup_maps
                .add_trigger(
                    TriggerConfig {
                        service_id,
                        workflow_id: "workflow-1".parse().unwrap(),
                        trigger: trigger.clone(),
                    },
                    &controllers,
                )
                .unwrap();
        }

        lookup_maps
            .remove_service(service_a.clone(), &controllers)
            .unwrap();

        let lookup_lock = lookup_maps
            .triggers_by_stellar_contract_event
            .read()
            .unwrap();
        assert_eq!(lookup_lock.len(), 1, "removing service A must not touch B");
        let by_service = lookup_maps.triggers_by_service_workflow.read().unwrap();
        assert!(by_service.get(&service_a).is_none());
        assert!(by_service.get(&service_b).is_some());
    }
}
