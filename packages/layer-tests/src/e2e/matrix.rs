use std::collections::HashSet;

use derive_enum_all_values::AllValues;
use serde::{Deserialize, Serialize};

use super::components::{AggregatorComponent, ComponentName, VectorComponent};

#[derive(Clone, Debug, Default)]
pub struct TestMatrix {
    pub evm: HashSet<EvmService>,
    pub cosmos: HashSet<CosmosService>,
    pub cross_chain: HashSet<CrossChainService>,
}

#[derive(
    Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, AllValues,
)]
#[serde(rename_all = "snake_case")]
pub enum EvmService {
    ChainTriggerLookup,
    CosmosQuery,
    EchoData,
    AtprotoEchoData,
    // #[cfg(feature = "hypercore-tests")]
    // HypercoreEchoData,
    ChangeWorkflow,
    EchoDataSecondaryChain,
    KvStore,
    Permissions,
    Square,
    MultiWorkflow,
    MultiTrigger,
    TriggerBackpressure,
    BlockInterval,
    BlockIntervalStartStop,
    CronInterval,
    EmptyToEchoData,
    SimpleAggregator,
    TimerAggregator,
    TimerAggregatorReorg,
    GasPrice,
    MultiOperator,
}

#[derive(
    Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, AllValues,
)]
#[serde(rename_all = "snake_case")]
pub enum CosmosService {
    ChainTriggerLookup,
    CosmosQuery,
    EchoData,
    Permissions,
    Square,
    BlockInterval,
    BlockIntervalStartStop,
    CronInterval,
}

#[derive(
    Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, AllValues,
)]
#[serde(rename_all = "snake_case")]
pub enum CrossChainService {
    CosmosToEvmEchoData,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AnyService {
    Evm(EvmService),
    Cosmos(CosmosService),
    CrossChain(CrossChainService),
}

impl From<EvmService> for AnyService {
    fn from(service: EvmService) -> Self {
        AnyService::Evm(service)
    }
}

impl From<CosmosService> for AnyService {
    fn from(service: CosmosService) -> Self {
        AnyService::Cosmos(service)
    }
}

impl From<CrossChainService> for AnyService {
    fn from(service: CrossChainService) -> Self {
        AnyService::CrossChain(service)
    }
}

impl TestMatrix {
    // Returns a list of all enabled services across all chain types
    pub fn enabled_services(self) -> Vec<AnyService> {
        let mut services = Vec::new();

        // Add enabled EVM services
        for service in self.evm {
            services.push(service.into());
        }

        // Add enabled Cosmos services
        for service in self.cosmos {
            services.push(service.into());
        }

        // Add enabled cross-chain services
        for service in self.cross_chain {
            services.push(service.into());
        }

        services
    }

    pub fn evm_regular_chain_enabled(&self) -> bool {
        // since we currently only submit to EVM, it's always enabled
        // TODO - if we have `Submit::None` then this should be false if no other test is enabled
        true
    }

    pub fn evm_secondary_chain_enabled(&self) -> bool {
        self.evm.contains(&EvmService::EchoDataSecondaryChain)
    }

    pub fn cosmos_regular_chain_enabled(&self) -> bool {
        self.evm.contains(&EvmService::CosmosQuery)
            || !self.cosmos.is_empty()
            || self
                .cross_chain
                .contains(&CrossChainService::CosmosToEvmEchoData)
    }

    pub fn multi_vector_enabled(&self) -> bool {
        self.evm.contains(&EvmService::MultiOperator)
    }
}

impl From<EvmService> for Vec<ComponentName> {
    fn from(service: EvmService) -> Self {
        match service {
            EvmService::ChainTriggerLookup => vec![ComponentName::Vector(
                VectorComponent::ChainTriggerLookup,
            )],
            EvmService::CosmosQuery => {
                vec![ComponentName::Vector(VectorComponent::CosmosQuery)]
            }
            EvmService::EchoData => vec![ComponentName::Vector(VectorComponent::EchoData)],
            EvmService::AtprotoEchoData => {
                vec![ComponentName::Vector(VectorComponent::EchoData)]
            }
            // #[cfg(feature = "hypercore-tests")]
            // EvmService::HypercoreEchoData => {
            //     vec![ComponentName::Vector(VectorComponent::EchoData)]
            // }
            EvmService::ChangeWorkflow => vec![
                ComponentName::Vector(VectorComponent::Square),
                ComponentName::Vector(VectorComponent::EchoData),
            ],
            EvmService::EchoDataSecondaryChain => {
                vec![ComponentName::Vector(VectorComponent::EchoData)]
            }
            EvmService::KvStore => vec![ComponentName::Vector(VectorComponent::KvStore)],
            EvmService::Permissions => {
                vec![ComponentName::Vector(VectorComponent::Permissions)]
            }
            EvmService::Square => vec![ComponentName::Vector(VectorComponent::Square)],
            EvmService::MultiWorkflow => vec![
                ComponentName::Vector(VectorComponent::Square),
                ComponentName::Vector(VectorComponent::EchoData),
            ],
            EvmService::MultiTrigger => vec![ComponentName::Vector(VectorComponent::EchoData)],
            EvmService::TriggerBackpressure => {
                vec![ComponentName::Vector(VectorComponent::EchoData)]
            }
            EvmService::BlockInterval => vec![ComponentName::Vector(
                VectorComponent::EchoBlockInterval,
            )],
            EvmService::BlockIntervalStartStop => vec![ComponentName::Vector(
                VectorComponent::EchoBlockInterval,
            )],
            EvmService::CronInterval => {
                vec![ComponentName::Vector(VectorComponent::EchoCronInterval)]
            }
            EvmService::EmptyToEchoData => {
                vec![ComponentName::Vector(VectorComponent::EchoData)]
            }
            EvmService::SimpleAggregator => {
                vec![ComponentName::Vector(VectorComponent::EchoData)]
            }
            EvmService::TimerAggregator => {
                vec![ComponentName::Vector(VectorComponent::EchoData)]
            }
            EvmService::TimerAggregatorReorg => {
                vec![ComponentName::Vector(VectorComponent::EchoData)]
            }
            EvmService::GasPrice => {
                vec![
                    ComponentName::Vector(VectorComponent::EchoData),
                    ComponentName::Aggregator(AggregatorComponent::SimpleAggregator),
                ]
            }
            EvmService::MultiOperator => {
                vec![ComponentName::Vector(VectorComponent::EchoData)]
            }
        }
    }
}

impl From<CosmosService> for Vec<ComponentName> {
    fn from(service: CosmosService) -> Self {
        match service {
            CosmosService::ChainTriggerLookup => vec![ComponentName::Vector(
                VectorComponent::ChainTriggerLookup,
            )],
            CosmosService::CosmosQuery => {
                vec![ComponentName::Vector(VectorComponent::CosmosQuery)]
            }
            CosmosService::EchoData => vec![ComponentName::Vector(VectorComponent::EchoData)],
            CosmosService::Permissions => {
                vec![ComponentName::Vector(VectorComponent::Permissions)]
            }
            CosmosService::Square => vec![ComponentName::Vector(VectorComponent::Square)],
            CosmosService::BlockInterval => vec![ComponentName::Vector(
                VectorComponent::EchoBlockInterval,
            )],
            CosmosService::BlockIntervalStartStop => vec![ComponentName::Vector(
                VectorComponent::EchoBlockInterval,
            )],
            CosmosService::CronInterval => {
                vec![ComponentName::Vector(VectorComponent::EchoCronInterval)]
            }
        }
    }
}

impl From<CrossChainService> for Vec<ComponentName> {
    fn from(service: CrossChainService) -> Self {
        match service {
            CrossChainService::CosmosToEvmEchoData => {
                vec![ComponentName::Vector(VectorComponent::EchoData)]
            }
        }
    }
}

impl From<AnyService> for Vec<ComponentName> {
    fn from(service: AnyService) -> Self {
        match service {
            AnyService::Evm(service) => service.into(),
            AnyService::Cosmos(service) => service.into(),
            AnyService::CrossChain(service) => service.into(),
        }
    }
}
