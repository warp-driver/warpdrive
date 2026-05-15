use std::collections::HashSet;

use derive_enum_all_values::AllValues;
use serde::{Deserialize, Serialize};
use utils::test_utils::middleware::stellar::SignerScheme;

use super::components::{AggregatorComponent, ComponentName, VectorComponent};

#[derive(Clone, Debug, Default)]
pub struct TestMatrix {
    pub evm: HashSet<EvmService>,
    pub cosmos: HashSet<CosmosService>,
    pub stellar: HashSet<StellarService>,
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

#[derive(
    Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, AllValues,
)]
#[serde(rename_all = "snake_case")]
pub enum StellarService {
    EchoData,
    /// First ed25519 (`--variant stellar`) Stellar test. Drives the
    /// `echo-data-xlm` component, which mirrors `echo-data` but emits
    /// its `WasmResponse` through `stellar_encode_trigger_output` —
    /// raw bytes, no `DataWithId` wrapper. The aggregator wraps that
    /// payload in an XDR-encoded `XlmEnvelope`; `mock_submit_xlm`
    /// decodes the envelope and stores `envelope.payload` raw, keyed
    /// by 20-byte `event_id`. The read path polls the contract's
    /// `Verified` event for the event_id, then calls `payload(event_id)`
    /// to retrieve the bytes.
    EchoDataXlm,
    BlockInterval,
    BlockIntervalStartStop,
    /// Drives the `stellar-query` component. The component logs the
    /// chain config returned by `host::get_stellar_chain_config` and
    /// then `unimplemented!()`s — the test is expected to fail
    /// locally until issue #5's follow-up wires up Soroban-from-
    /// component RPC. CI configs deliberately don't include this so
    /// CI stays green.
    StellarQuery,
}

impl StellarService {
    /// Which Stellar signing scheme this test runs against.
    /// `EchoDataXlm` runs on the ed25519 (`--variant stellar`) stack;
    /// every other Stellar test runs on the secp256k1
    /// (`--variant ethereum`) stack. The shared-stack bootstrap deploys
    /// one stack per distinct scheme that appears in the test matrix.
    pub fn scheme(self) -> SignerScheme {
        match self {
            StellarService::EchoData
            | StellarService::BlockInterval
            | StellarService::BlockIntervalStartStop
            | StellarService::StellarQuery => SignerScheme::Secp256k1,
            StellarService::EchoDataXlm => SignerScheme::Ed25519,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AnyService {
    Evm(EvmService),
    Cosmos(CosmosService),
    Stellar(StellarService),
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

impl From<StellarService> for AnyService {
    fn from(service: StellarService) -> Self {
        AnyService::Stellar(service)
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

        // Add enabled Stellar services
        for service in self.stellar {
            services.push(service.into());
        }

        // Add enabled cross-chain services
        for service in self.cross_chain {
            services.push(service.into());
        }

        services
    }

    pub fn evm_regular_chain_enabled(&self) -> bool {
        !self.evm.is_empty()
            || self
                .cross_chain
                .contains(&CrossChainService::CosmosToEvmEchoData)
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
            EvmService::ChainTriggerLookup => {
                vec![ComponentName::Vector(VectorComponent::ChainTriggerLookup)]
            }
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
            EvmService::BlockInterval => {
                vec![ComponentName::Vector(VectorComponent::EchoBlockInterval)]
            }
            EvmService::BlockIntervalStartStop => {
                vec![ComponentName::Vector(VectorComponent::EchoBlockInterval)]
            }
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
            CosmosService::ChainTriggerLookup => {
                vec![ComponentName::Vector(VectorComponent::ChainTriggerLookup)]
            }
            CosmosService::CosmosQuery => {
                vec![ComponentName::Vector(VectorComponent::CosmosQuery)]
            }
            CosmosService::EchoData => vec![ComponentName::Vector(VectorComponent::EchoData)],
            CosmosService::Permissions => {
                vec![ComponentName::Vector(VectorComponent::Permissions)]
            }
            CosmosService::Square => vec![ComponentName::Vector(VectorComponent::Square)],
            CosmosService::BlockInterval => {
                vec![ComponentName::Vector(VectorComponent::EchoBlockInterval)]
            }
            CosmosService::BlockIntervalStartStop => {
                vec![ComponentName::Vector(VectorComponent::EchoBlockInterval)]
            }
            CosmosService::CronInterval => {
                vec![ComponentName::Vector(VectorComponent::EchoCronInterval)]
            }
        }
    }
}

impl From<StellarService> for Vec<ComponentName> {
    fn from(service: StellarService) -> Self {
        match service {
            StellarService::EchoData => vec![ComponentName::Vector(VectorComponent::EchoData)],
            StellarService::EchoDataXlm => {
                vec![ComponentName::Vector(VectorComponent::EchoData)]
            }
            StellarService::BlockInterval => {
                vec![ComponentName::Vector(VectorComponent::EchoBlockInterval)]
            }
            StellarService::BlockIntervalStartStop => {
                vec![ComponentName::Vector(VectorComponent::EchoBlockInterval)]
            }
            StellarService::StellarQuery => {
                vec![ComponentName::Vector(VectorComponent::StellarQuery)]
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
            AnyService::Stellar(service) => service.into(),
            AnyService::CrossChain(service) => service.into(),
        }
    }
}
