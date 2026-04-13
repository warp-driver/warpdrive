use warpdrive_wasi_utils::impl_u128_conversions;

use crate::bindings::{
    aggregator::world::wavs::operator::input as aggregator_operator_input,
    aggregator::world::wavs::{
        aggregator::input as aggregator_input,
        aggregator::output::{self as aggregator_output, U128},
        types::{
            chain as aggregator_chain, core as aggregator_core, events as aggregator_events,
            service as aggregator_service,
        },
    },
    operator::world::wavs::{
        operator::{input as component_input, output as component_output},
        types::{
            chain as component_chain, core as component_core, events as component_events,
            service as component_service,
        },
    },
};

impl_u128_conversions!(U128);

impl TryFrom<warpdrive_types::Trigger> for component_service::Trigger {
    type Error = anyhow::Error;

    fn try_from(src: warpdrive_types::Trigger) -> Result<Self, Self::Error> {
        Ok(match src {
            warpdrive_types::Trigger::CosmosContractEvent {
                address,
                chain,
                event_type,
            } => component_service::Trigger::CosmosContractEvent(
                component_service::TriggerCosmosContractEvent {
                    address: address.into(),
                    chain: chain.to_string(),
                    event_type,
                },
            ),
            warpdrive_types::Trigger::EvmContractEvent {
                address,
                chain,
                event_hash,
            } => component_service::Trigger::EvmContractEvent(
                component_service::TriggerEvmContractEvent {
                    address: address.into(),
                    chain: chain.to_string(),
                    event_hash: event_hash.as_slice().to_vec(),
                },
            ),
            warpdrive_types::Trigger::BlockInterval {
                chain,
                n_blocks,
                start_block,
                end_block,
            } => {
                component_service::Trigger::BlockInterval(component_service::TriggerBlockInterval {
                    chain: chain.to_string(),
                    n_blocks: n_blocks.into(),
                    start_block: start_block.map(Into::into),
                    end_block: end_block.map(Into::into),
                })
            }
            warpdrive_types::Trigger::Manual => component_service::Trigger::Manual,
            warpdrive_types::Trigger::Cron {
                schedule,
                start_time,
                end_time,
            } => component_service::Trigger::Cron(component_service::TriggerCron {
                schedule: schedule.to_string(),
                start_time: start_time.map(Into::into),
                end_time: end_time.map(Into::into),
            }),
            warpdrive_types::Trigger::AtProtoEvent {
                collection,
                repo_did,
                action,
            } => component_service::Trigger::AtprotoEvent(component_service::TriggerAtprotoEvent {
                collection,
                repo_did,
                action: action.map(|a| a.to_string()),
            }),
        })
    }
}

impl TryFrom<layer_climb::prelude::Address> for component_chain::CosmosAddress {
    type Error = anyhow::Error;

    fn try_from(address: layer_climb::prelude::Address) -> Result<Self, Self::Error> {
        match address {
            layer_climb::prelude::Address::Cosmos(addr) => Ok(Self {
                bech32_addr: addr.to_string(),
                prefix_len: addr.prefix().len() as u32,
            }),
            _ => Err(anyhow::anyhow!("Not a cosmos address")),
        }
    }
}

impl TryFrom<layer_climb::prelude::Address> for component_chain::EvmAddress {
    type Error = anyhow::Error;

    fn try_from(address: layer_climb::prelude::Address) -> Result<Self, Self::Error> {
        match address {
            layer_climb::prelude::Address::Evm(addr) => Ok(Self {
                raw_bytes: addr.as_bytes().to_vec(),
            }),
            _ => Err(anyhow::anyhow!("Not an EVM address")),
        }
    }
}

impl From<alloy_primitives::Address> for component_chain::EvmAddress {
    fn from(address: alloy_primitives::Address) -> Self {
        Self {
            raw_bytes: address.to_vec(),
        }
    }
}

impl From<layer_climb::prelude::CosmosAddr> for component_chain::CosmosAddress {
    fn from(address: layer_climb::prelude::CosmosAddr) -> Self {
        component_chain::CosmosAddress {
            bech32_addr: address.to_string(),
            prefix_len: address.prefix().len() as u32,
        }
    }
}

impl From<warpdrive_types::CosmosChainConfig>
    for crate::bindings::operator::world::host::CosmosChainConfig
{
    fn from(config: warpdrive_types::CosmosChainConfig) -> Self {
        Self {
            chain_id: config.chain_id.as_str().to_string(),
            rpc_endpoint: config.rpc_endpoint,
            grpc_endpoint: config.grpc_endpoint,
            grpc_web_endpoint: None,
            gas_denom: config.gas_denom,
            gas_price: config.gas_price,
            bech32_prefix: config.bech32_prefix,
        }
    }
}

impl From<warpdrive_types::EvmChainConfig> for crate::bindings::operator::world::host::EvmChainConfig {
    fn from(config: warpdrive_types::EvmChainConfig) -> Self {
        Self {
            chain_id: config.chain_id.to_string(),
            ws_endpoints: config.ws_endpoints,
            http_endpoint: config.http_endpoint,
        }
    }
}

impl From<warpdrive_types::Timestamp> for component_core::Timestamp {
    fn from(src: warpdrive_types::Timestamp) -> Self {
        component_core::Timestamp {
            nanos: src.as_nanos(),
        }
    }
}

impl TryFrom<warpdrive_types::Service> for component_service::Service {
    type Error = anyhow::Error;

    fn try_from(src: warpdrive_types::Service) -> Result<Self, Self::Error> {
        Ok(Self {
            name: src.name,
            workflows: src
                .workflows
                .into_iter()
                .map(|(workflow_id, workflow)| {
                    workflow
                        .try_into()
                        .map(|workflow| (workflow_id.to_string(), workflow))
                })
                .collect::<anyhow::Result<Vec<(String, component_service::Workflow)>>>()?,
            status: src.status.into(),
            manager: src.manager.into(),
        })
    }
}

impl TryFrom<warpdrive_types::Workflow> for component_service::Workflow {
    type Error = anyhow::Error;

    fn try_from(src: warpdrive_types::Workflow) -> Result<Self, Self::Error> {
        Ok(Self {
            trigger: src.trigger.try_into()?,
            component: src.component.into(),
            submit: src.submit.into(),
        })
    }
}

impl From<warpdrive_types::Component> for component_service::Component {
    fn from(src: warpdrive_types::Component) -> Self {
        Self {
            source: src.source.into(),
            permissions: src.permissions.into(),
            fuel_limit: src.fuel_limit,
            time_limit_seconds: src.time_limit_seconds,
            config: src.config.into_iter().collect(),
            env_keys: src.env_keys.into_iter().collect(),
        }
    }
}

impl From<warpdrive_types::ComponentSource> for component_service::ComponentSource {
    fn from(src: warpdrive_types::ComponentSource) -> Self {
        match src {
            warpdrive_types::ComponentSource::Digest(digest) => {
                component_service::ComponentSource::Digest(digest.to_string())
            }
            warpdrive_types::ComponentSource::Download { uri, digest } => {
                component_service::ComponentSource::Download(
                    component_service::ComponentSourceDownload {
                        uri: uri.to_string(),
                        digest: digest.to_string(),
                    },
                )
            }
            warpdrive_types::ComponentSource::Registry { registry } => {
                component_service::ComponentSource::Registry(registry.into())
            }
        }
    }
}

impl From<warpdrive_types::Registry> for component_service::Registry {
    fn from(src: warpdrive_types::Registry) -> Self {
        Self {
            digest: src.digest.to_string(),
            domain: src.domain,
            version: src.version.map(|v| v.to_string()),
            pkg: src.package.to_string(),
        }
    }
}

impl From<warpdrive_types::Permissions> for component_service::Permissions {
    fn from(src: warpdrive_types::Permissions) -> Self {
        Self {
            allowed_http_hosts: src.allowed_http_hosts.into(),
            file_system: src.file_system,
            raw_sockets: src.raw_sockets,
            dns_resolution: src.dns_resolution,
        }
    }
}

impl From<warpdrive_types::Submit> for component_service::Submit {
    fn from(src: warpdrive_types::Submit) -> Self {
        match src {
            warpdrive_types::Submit::None => component_service::Submit::None,
            warpdrive_types::Submit::Aggregator {
                component,
                signature_kind,
            } => component_service::Submit::Aggregator(component_service::AggregatorSubmit {
                component: (*component).into(),
                signature_kind: signature_kind.into(),
            }),
        }
    }
}

impl From<warpdrive_types::SignatureKind> for component_service::SignatureKind {
    fn from(src: warpdrive_types::SignatureKind) -> Self {
        Self {
            algorithm: src.algorithm.into(),
            prefix: src.prefix.map(Into::into),
        }
    }
}

impl From<warpdrive_types::SignatureAlgorithm> for component_service::SignatureAlgorithm {
    fn from(src: warpdrive_types::SignatureAlgorithm) -> Self {
        match src {
            warpdrive_types::SignatureAlgorithm::Secp256k1 => {
                component_service::SignatureAlgorithm::Secp256k1
            }
        }
    }
}

impl From<warpdrive_types::SignaturePrefix> for component_service::SignaturePrefix {
    fn from(src: warpdrive_types::SignaturePrefix) -> Self {
        match src {
            warpdrive_types::SignaturePrefix::Eip191 => component_service::SignaturePrefix::Eip191,
        }
    }
}

impl From<warpdrive_types::AllowedHostPermission> for component_service::AllowedHostPermission {
    fn from(src: warpdrive_types::AllowedHostPermission) -> Self {
        match src {
            warpdrive_types::AllowedHostPermission::All => component_service::AllowedHostPermission::All,
            warpdrive_types::AllowedHostPermission::None => {
                component_service::AllowedHostPermission::None
            }
            warpdrive_types::AllowedHostPermission::Only(hosts) => {
                component_service::AllowedHostPermission::Only(hosts)
            }
        }
    }
}

impl From<warpdrive_types::ServiceStatus> for component_service::ServiceStatus {
    fn from(src: warpdrive_types::ServiceStatus) -> Self {
        match src {
            warpdrive_types::ServiceStatus::Active => component_service::ServiceStatus::Active,
            warpdrive_types::ServiceStatus::Paused => component_service::ServiceStatus::Paused,
        }
    }
}

impl From<warpdrive_types::ServiceManager> for component_service::ServiceManager {
    fn from(src: warpdrive_types::ServiceManager) -> Self {
        match src {
            warpdrive_types::ServiceManager::Evm { chain, address } => {
                component_service::ServiceManager::Evm(component_service::EvmManager {
                    chain: chain.to_string(),
                    address: address.into(),
                })
            }
            warpdrive_types::ServiceManager::Cosmos { chain, address } => {
                component_service::ServiceManager::Cosmos(component_service::CosmosManager {
                    chain: chain.to_string(),
                    address: address.into(),
                })
            }
        }
    }
}

impl From<warpdrive_types::WasmResponse> for component_output::WasmResponse {
    fn from(src: warpdrive_types::WasmResponse) -> Self {
        Self {
            payload: src.payload,
            ordering: src.ordering,
            event_id_salt: src.event_id_salt,
        }
    }
}

impl TryFrom<warpdrive_types::TriggerAction> for component_input::TriggerAction {
    type Error = anyhow::Error;

    fn try_from(src: warpdrive_types::TriggerAction) -> Result<Self, Self::Error> {
        Ok(Self {
            config: src.config.try_into()?,
            data: src.data.try_into()?,
        })
    }
}

impl TryFrom<warpdrive_types::TriggerConfig> for component_input::TriggerConfig {
    type Error = anyhow::Error;

    fn try_from(src: warpdrive_types::TriggerConfig) -> Result<Self, Self::Error> {
        Ok(Self {
            service_id: src.service_id.to_string(),
            workflow_id: src.workflow_id.to_string(),
            trigger: src.trigger.try_into()?,
        })
    }
}

impl TryFrom<warpdrive_types::TriggerData> for component_input::TriggerData {
    type Error = anyhow::Error;

    fn try_from(src: warpdrive_types::TriggerData) -> Result<Self, Self::Error> {
        match src {
            warpdrive_types::TriggerData::EvmContractEvent {
                chain,
                contract_address,
                log_data,
                tx_hash,
                block_number,
                log_index,
                block_hash,
                block_timestamp,
                tx_index,
            } => Ok(component_input::TriggerData::EvmContractEvent(
                component_events::TriggerDataEvmContractEvent {
                    chain: chain.to_string(),
                    log: component_events::EvmEventLog {
                        address: contract_address.into(),
                        data: component_chain::EvmEventLogData {
                            topics: log_data
                                .topics()
                                .iter()
                                .map(|topic| topic.to_vec())
                                .collect(),
                            data: log_data.data.to_vec(),
                        },
                        tx_hash: tx_hash.to_vec(),
                        block_number,
                        log_index,
                        block_hash: block_hash.to_vec(),
                        block_timestamp,
                        tx_index,
                    },
                },
            )),
            warpdrive_types::TriggerData::CosmosContractEvent {
                contract_address,
                chain,
                event,
                event_index,
                block_height,
            } => Ok(component_input::TriggerData::CosmosContractEvent(
                component_events::TriggerDataCosmosContractEvent {
                    contract_address: contract_address.into(),
                    chain: chain.to_string(),
                    event: component_events::CosmosEvent {
                        ty: event.ty,
                        attributes: event
                            .attributes
                            .into_iter()
                            .map(|attr| (attr.key, attr.value))
                            .collect(),
                    },
                    event_index,
                    block_height,
                },
            )),
            warpdrive_types::TriggerData::BlockInterval {
                chain,
                block_height,
            } => Ok(component_input::TriggerData::BlockInterval(
                component_events::TriggerDataBlockInterval {
                    chain: chain.to_string(),
                    block_height,
                },
            )),
            warpdrive_types::TriggerData::Cron { trigger_time } => Ok(
                component_input::TriggerData::Cron(component_events::TriggerDataCron {
                    trigger_time: trigger_time.into(),
                }),
            ),
            warpdrive_types::TriggerData::AtProtoEvent {
                sequence,
                timestamp,
                repo,
                collection,
                rkey,
                action,
                cid,
                record,
                rev,
                op_index,
            } => {
                let record_data = record
                    .map(|value| serde_json::to_string(&value))
                    .transpose()?;

                Ok(component_input::TriggerData::AtprotoEvent(
                    component_events::TriggerDataAtprotoEvent {
                        sequence,
                        timestamp,
                        repo,
                        collection,
                        rkey,
                        action: action.to_string(),
                        cid,
                        record_data,
                        rev,
                        op_index,
                    },
                ))
            }
            warpdrive_types::TriggerData::Raw(data) => Ok(component_input::TriggerData::Raw(data)),
        }
    }
}

// aggregator

impl TryFrom<warpdrive_types::AggregatorInput> for aggregator_input::AggregatorInput {
    type Error = anyhow::Error;

    fn try_from(input: warpdrive_types::AggregatorInput) -> Result<Self, Self::Error> {
        Ok(aggregator_input::AggregatorInput {
            trigger_action: input.trigger_action.try_into()?,
            operator_response: input.operator_response.into(),
        })
    }
}

impl TryFrom<warpdrive_types::TriggerAction> for aggregator_input::TriggerAction {
    type Error = anyhow::Error;

    fn try_from(action: warpdrive_types::TriggerAction) -> Result<Self, Self::Error> {
        Ok(aggregator_input::TriggerAction {
            config: action.config.try_into()?,
            data: action.data.try_into()?,
        })
    }
}

impl From<warpdrive_types::WasmResponse> for aggregator_input::WasmResponse {
    fn from(resp: warpdrive_types::WasmResponse) -> Self {
        aggregator_input::WasmResponse {
            payload: resp.payload,
            ordering: resp.ordering,
            event_id_salt: resp.event_id_salt,
        }
    }
}

impl TryFrom<warpdrive_types::TriggerConfig> for aggregator_operator_input::TriggerConfig {
    type Error = anyhow::Error;

    fn try_from(config: warpdrive_types::TriggerConfig) -> Result<Self, Self::Error> {
        Ok(aggregator_operator_input::TriggerConfig {
            service_id: config.service_id.to_string(),
            workflow_id: config.workflow_id.to_string(),
            trigger: config.trigger.try_into()?,
        })
    }
}

impl TryFrom<warpdrive_types::TriggerData> for aggregator_operator_input::TriggerData {
    type Error = anyhow::Error;

    fn try_from(src: warpdrive_types::TriggerData) -> Result<Self, Self::Error> {
        match src {
            warpdrive_types::TriggerData::EvmContractEvent {
                chain,
                contract_address,
                log_data,
                tx_hash,
                block_number,
                log_index,
                block_hash,
                block_timestamp,
                tx_index,
            } => Ok(aggregator_operator_input::TriggerData::EvmContractEvent(
                aggregator_events::TriggerDataEvmContractEvent {
                    chain: chain.to_string(),
                    log: aggregator_events::EvmEventLog {
                        address: contract_address.into(),
                        data: aggregator_chain::EvmEventLogData {
                            topics: log_data
                                .topics()
                                .iter()
                                .map(|topic| topic.to_vec())
                                .collect(),
                            data: log_data.data.to_vec(),
                        },
                        tx_hash: tx_hash.to_vec(),
                        block_number,
                        log_index,
                        block_hash: block_hash.to_vec(),
                        block_timestamp,
                        tx_index,
                    },
                },
            )),
            warpdrive_types::TriggerData::CosmosContractEvent {
                contract_address,
                chain,
                event,
                event_index,
                block_height,
            } => Ok(aggregator_operator_input::TriggerData::CosmosContractEvent(
                aggregator_events::TriggerDataCosmosContractEvent {
                    contract_address: contract_address.into(),
                    chain: chain.to_string(),
                    event: aggregator_events::CosmosEvent {
                        ty: event.ty,
                        attributes: event
                            .attributes
                            .into_iter()
                            .map(|attr| (attr.key, attr.value))
                            .collect(),
                    },
                    event_index,
                    block_height,
                },
            )),
            warpdrive_types::TriggerData::BlockInterval {
                chain,
                block_height,
            } => Ok(aggregator_operator_input::TriggerData::BlockInterval(
                aggregator_events::TriggerDataBlockInterval {
                    chain: chain.to_string(),
                    block_height,
                },
            )),
            warpdrive_types::TriggerData::Cron { trigger_time } => Ok(
                aggregator_operator_input::TriggerData::Cron(aggregator_events::TriggerDataCron {
                    trigger_time: trigger_time.into(),
                }),
            ),
            warpdrive_types::TriggerData::AtProtoEvent {
                sequence,
                timestamp,
                repo,
                collection,
                rkey,
                action,
                cid,
                record,
                rev,
                op_index,
            } => {
                let record_data = record
                    .map(|value| serde_json::to_string(&value))
                    .transpose()?;

                Ok(aggregator_operator_input::TriggerData::AtprotoEvent(
                    aggregator_events::TriggerDataAtprotoEvent {
                        sequence,
                        timestamp,
                        repo,
                        collection,
                        rkey,
                        action: action.to_string(),
                        cid,
                        record_data,
                        rev,
                        op_index,
                    },
                ))
            }
            warpdrive_types::TriggerData::Raw(data) => {
                Ok(aggregator_operator_input::TriggerData::Raw(data))
            }
        }
    }
}

impl TryFrom<warpdrive_types::Service> for aggregator_service::Service {
    type Error = anyhow::Error;

    fn try_from(service: warpdrive_types::Service) -> Result<Self, Self::Error> {
        Ok(aggregator_service::Service {
            name: service.name,
            workflows: service
                .workflows
                .into_iter()
                .map(|(id, workflow)| (id.to_string(), workflow.try_into().unwrap()))
                .collect(),
            status: service.status.into(),
            manager: service.manager.into(),
        })
    }
}

impl TryFrom<warpdrive_types::Workflow> for aggregator_service::Workflow {
    type Error = anyhow::Error;

    fn try_from(workflow: warpdrive_types::Workflow) -> Result<Self, Self::Error> {
        Ok(aggregator_service::Workflow {
            trigger: workflow.trigger.try_into()?,
            component: workflow.component.into(),
            submit: workflow.submit.into(),
        })
    }
}

impl From<warpdrive_types::ServiceStatus> for aggregator_service::ServiceStatus {
    fn from(status: warpdrive_types::ServiceStatus) -> Self {
        match status {
            warpdrive_types::ServiceStatus::Active => aggregator_service::ServiceStatus::Active,
            warpdrive_types::ServiceStatus::Paused => aggregator_service::ServiceStatus::Paused,
        }
    }
}

impl From<warpdrive_types::ServiceManager> for aggregator_service::ServiceManager {
    fn from(manager: warpdrive_types::ServiceManager) -> Self {
        match manager {
            warpdrive_types::ServiceManager::Evm { chain, address } => {
                aggregator_service::ServiceManager::Evm(aggregator_service::EvmManager {
                    chain: chain.to_string(),
                    address: aggregator_chain::EvmAddress {
                        raw_bytes: address.to_vec(),
                    },
                })
            }
            warpdrive_types::ServiceManager::Cosmos { chain, address } => {
                aggregator_service::ServiceManager::Cosmos(aggregator_service::CosmosManager {
                    chain: chain.to_string(),
                    address: aggregator_chain::CosmosAddress {
                        bech32_addr: address.to_string(),
                        prefix_len: address.prefix().len() as u32,
                    },
                })
            }
        }
    }
}

impl From<alloy_primitives::Address> for aggregator_chain::EvmAddress {
    fn from(address: alloy_primitives::Address) -> Self {
        aggregator_chain::EvmAddress {
            raw_bytes: address.to_vec(),
        }
    }
}

impl From<layer_climb::prelude::EvmAddr> for aggregator_chain::EvmAddress {
    fn from(address: layer_climb::prelude::EvmAddr) -> Self {
        aggregator_chain::EvmAddress {
            raw_bytes: address.as_bytes().to_vec(),
        }
    }
}

impl From<layer_climb::prelude::CosmosAddr> for aggregator_chain::CosmosAddress {
    fn from(address: layer_climb::prelude::CosmosAddr) -> Self {
        aggregator_chain::CosmosAddress {
            bech32_addr: address.to_string(),
            prefix_len: address.prefix().len() as u32,
        }
    }
}

impl From<warpdrive_types::Component> for aggregator_service::Component {
    fn from(component: warpdrive_types::Component) -> Self {
        aggregator_service::Component {
            source: component.source.into(),
            permissions: component.permissions.into(),
            fuel_limit: component.fuel_limit,
            time_limit_seconds: component.time_limit_seconds,
            config: component.config.into_iter().collect(),
            env_keys: component.env_keys.into_iter().collect(),
        }
    }
}

impl From<warpdrive_types::ComponentSource> for aggregator_service::ComponentSource {
    fn from(source: warpdrive_types::ComponentSource) -> Self {
        match source {
            warpdrive_types::ComponentSource::Digest(digest) => {
                aggregator_service::ComponentSource::Digest(digest.to_string())
            }
            warpdrive_types::ComponentSource::Download { uri, digest } => {
                aggregator_service::ComponentSource::Download(
                    aggregator_service::ComponentSourceDownload {
                        uri: uri.to_string(),
                        digest: digest.to_string(),
                    },
                )
            }
            warpdrive_types::ComponentSource::Registry { registry } => {
                aggregator_service::ComponentSource::Registry(registry.into())
            }
        }
    }
}

impl From<warpdrive_types::Registry> for aggregator_service::Registry {
    fn from(registry: warpdrive_types::Registry) -> Self {
        aggregator_service::Registry {
            digest: registry.digest.to_string(),
            domain: registry.domain,
            version: registry.version.map(|v| v.to_string()),
            pkg: registry.package.to_string(),
        }
    }
}

impl From<warpdrive_types::Permissions> for aggregator_service::Permissions {
    fn from(permissions: warpdrive_types::Permissions) -> Self {
        aggregator_service::Permissions {
            allowed_http_hosts: permissions.allowed_http_hosts.into(),
            file_system: permissions.file_system,
            raw_sockets: permissions.raw_sockets,
            dns_resolution: permissions.dns_resolution,
        }
    }
}

impl From<warpdrive_types::AllowedHostPermission> for aggregator_service::AllowedHostPermission {
    fn from(permission: warpdrive_types::AllowedHostPermission) -> Self {
        match permission {
            warpdrive_types::AllowedHostPermission::All => {
                aggregator_service::AllowedHostPermission::All
            }
            warpdrive_types::AllowedHostPermission::None => {
                aggregator_service::AllowedHostPermission::None
            }
            warpdrive_types::AllowedHostPermission::Only(hosts) => {
                aggregator_service::AllowedHostPermission::Only(hosts)
            }
        }
    }
}

impl From<warpdrive_types::Submit> for aggregator_service::Submit {
    fn from(submit: warpdrive_types::Submit) -> Self {
        match submit {
            warpdrive_types::Submit::None => aggregator_service::Submit::None,
            warpdrive_types::Submit::Aggregator {
                component,
                signature_kind,
            } => aggregator_service::Submit::Aggregator(aggregator_service::AggregatorSubmit {
                component: (*component).into(),
                signature_kind: signature_kind.into(),
            }),
        }
    }
}

impl From<warpdrive_types::SignatureKind> for aggregator_service::SignatureKind {
    fn from(src: warpdrive_types::SignatureKind) -> Self {
        Self {
            algorithm: src.algorithm.into(),
            prefix: src.prefix.map(Into::into),
        }
    }
}

impl From<warpdrive_types::SignatureAlgorithm> for aggregator_service::SignatureAlgorithm {
    fn from(src: warpdrive_types::SignatureAlgorithm) -> Self {
        match src {
            warpdrive_types::SignatureAlgorithm::Secp256k1 => {
                aggregator_service::SignatureAlgorithm::Secp256k1
            }
        }
    }
}

impl From<warpdrive_types::SignaturePrefix> for aggregator_service::SignaturePrefix {
    fn from(src: warpdrive_types::SignaturePrefix) -> Self {
        match src {
            warpdrive_types::SignaturePrefix::Eip191 => aggregator_service::SignaturePrefix::Eip191,
        }
    }
}

impl TryFrom<warpdrive_types::Trigger> for aggregator_service::Trigger {
    type Error = anyhow::Error;

    fn try_from(trigger: warpdrive_types::Trigger) -> Result<Self, Self::Error> {
        Ok(match trigger {
            warpdrive_types::Trigger::Manual => aggregator_service::Trigger::Manual,
            warpdrive_types::Trigger::EvmContractEvent {
                address,
                chain,
                event_hash,
            } => aggregator_service::Trigger::EvmContractEvent(
                aggregator_service::TriggerEvmContractEvent {
                    address: address.into(),
                    chain: chain.to_string(),
                    event_hash: event_hash.as_slice().to_vec(),
                },
            ),
            warpdrive_types::Trigger::CosmosContractEvent {
                address,
                chain,
                event_type,
            } => aggregator_service::Trigger::CosmosContractEvent(
                aggregator_service::TriggerCosmosContractEvent {
                    address: address.into(),
                    chain: chain.to_string(),
                    event_type,
                },
            ),
            warpdrive_types::Trigger::BlockInterval {
                chain,
                n_blocks,
                start_block,
                end_block,
            } => aggregator_service::Trigger::BlockInterval(
                aggregator_service::TriggerBlockInterval {
                    chain: chain.to_string(),
                    n_blocks: n_blocks.into(),
                    start_block: start_block.map(Into::into),
                    end_block: end_block.map(Into::into),
                },
            ),
            warpdrive_types::Trigger::Cron {
                schedule,
                start_time,
                end_time,
            } => aggregator_service::Trigger::Cron(aggregator_service::TriggerCron {
                schedule: schedule.to_string(),
                start_time: start_time.map(Into::into),
                end_time: end_time.map(Into::into),
            }),
            warpdrive_types::Trigger::AtProtoEvent {
                collection,
                repo_did,
                action,
            } => {
                aggregator_service::Trigger::AtprotoEvent(aggregator_service::TriggerAtprotoEvent {
                    collection,
                    repo_did,
                    action: action.map(|a| a.to_string()),
                })
            }
        })
    }
}

impl TryFrom<layer_climb::prelude::Address> for aggregator_chain::CosmosAddress {
    type Error = anyhow::Error;

    fn try_from(address: layer_climb::prelude::Address) -> Result<Self, Self::Error> {
        match address {
            layer_climb::prelude::Address::Cosmos(addr) => Ok(Self {
                bech32_addr: addr.to_string(),
                prefix_len: addr.prefix().len() as u32,
            }),
            _ => Err(anyhow::anyhow!("Not a cosmos address")),
        }
    }
}

impl From<warpdrive_types::Timestamp> for aggregator_core::Timestamp {
    fn from(timestamp: warpdrive_types::Timestamp) -> Self {
        aggregator_core::Timestamp {
            nanos: timestamp.as_nanos(),
        }
    }
}

impl From<warpdrive_types::Duration> for aggregator_core::Duration {
    fn from(duration: warpdrive_types::Duration) -> Self {
        aggregator_core::Duration {
            secs: duration.secs,
        }
    }
}

impl From<aggregator_core::Duration> for warpdrive_types::Duration {
    fn from(duration: aggregator_core::Duration) -> Self {
        warpdrive_types::Duration {
            secs: duration.secs,
        }
    }
}

impl From<warpdrive_types::AggregatorAction> for aggregator_output::AggregatorAction {
    fn from(action: warpdrive_types::AggregatorAction) -> Self {
        match action {
            warpdrive_types::AggregatorAction::Submit(action) => {
                aggregator_output::AggregatorAction::Submit(action.into())
            }
            warpdrive_types::AggregatorAction::Timer(timer) => {
                aggregator_output::AggregatorAction::Timer(aggregator_output::TimerAction {
                    delay: timer.delay.into(),
                })
            }
        }
    }
}

impl From<warpdrive_types::SubmitAction> for aggregator_output::SubmitAction {
    fn from(action: warpdrive_types::SubmitAction) -> Self {
        match action {
            warpdrive_types::SubmitAction::Evm(action) => {
                aggregator_output::SubmitAction::Evm(aggregator_output::EvmSubmitAction {
                    chain: action.chain.to_string(),
                    address: action.address.into(),
                    gas_price: action.gas_price.map(|x| x.into()),
                })
            }
            warpdrive_types::SubmitAction::Cosmos(action) => {
                aggregator_output::SubmitAction::Cosmos(aggregator_output::CosmosSubmitAction {
                    chain: action.chain.to_string(),
                    address: action.address.into(),
                    gas_price: action.gas_price.map(|x| x.into()),
                })
            }
        }
    }
}

impl From<warpdrive_types::CosmosChainConfig> for aggregator_chain::CosmosChainConfig {
    fn from(config: warpdrive_types::CosmosChainConfig) -> Self {
        Self {
            chain_id: config.chain_id.as_str().to_string(),
            rpc_endpoint: config.rpc_endpoint,
            grpc_endpoint: config.grpc_endpoint,
            grpc_web_endpoint: None,
            gas_denom: config.gas_denom,
            gas_price: config.gas_price,
            bech32_prefix: config.bech32_prefix,
        }
    }
}

impl From<warpdrive_types::EvmChainConfig> for aggregator_chain::EvmChainConfig {
    fn from(config: warpdrive_types::EvmChainConfig) -> Self {
        Self {
            chain_id: config.chain_id.to_string(),
            ws_endpoints: config.ws_endpoints,
            http_endpoint: config.http_endpoint,
        }
    }
}
