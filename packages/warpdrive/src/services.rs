use std::ops::Bound;
use std::sync::RwLock;
use std::{collections::BTreeMap, sync::Arc};

use thiserror::Error;
use tracing::instrument;
use utils::{
    stellar_client::STELLAR_QUERY_KEY,
    storage::db::{DBError, WavsDb},
};
use warpdrive_client::project_root::ProjectRootClient;
use warpdrive_types::SignatureKind;
use warpdrive_types::{
    contracts::stellar::StellarServiceManagerContracts, AnyChainConfig, ChainConfigs, ChainKey,
    Service, ServiceId, ServiceManager, ServiceStatus, Workflow, WorkflowId,
};

type Result<T> = std::result::Result<T, ServicesError>;

#[derive(Clone)]
pub struct Services {
    db_storage: WavsDb,
}

impl Services {
    pub fn new(db_storage: WavsDb) -> Self {
        Self { db_storage }
    }

    #[instrument(skip(self), fields(subsys = "Services"))]
    pub fn try_get(&self, id: &ServiceId) -> Result<Option<Service>> {
        Ok(self.db_storage.services.get_cloned(id))
    }

    #[instrument(skip(self), fields(subsys = "Services"))]
    pub fn get(&self, service_id: &ServiceId) -> Result<Service> {
        match self.try_get(service_id)? {
            Some(service) => Ok(service),
            None => Err(ServicesError::UnknownService(service_id.clone())),
        }
    }

    #[instrument(skip(self), fields(subsys = "Services"))]
    pub fn get_workflow(
        &self,
        service_id: &ServiceId,
        workflow_id: &WorkflowId,
    ) -> Result<Workflow> {
        let service = self.get(service_id)?;
        service
            .workflows
            .get(workflow_id)
            .cloned()
            .ok_or_else(|| ServicesError::UnknownWorkflow {
                service_name: service.name,
                service_id: service_id.clone(),
                workflow_id: workflow_id.clone(),
            })
    }

    #[instrument(skip(self), fields(subsys = "Services"))]
    pub fn get_stellar_service_manager_contracts(
        &self,
        service_id: &ServiceId,
    ) -> Result<StellarServiceManagerContracts> {
        self.db_storage
            .stellar_service_manager_contracts
            .get_cloned(service_id)
            .ok_or_else(|| {
                ServicesError::UnknownServiceForStellarServiceManager(service_id.clone())
            })
    }

    #[instrument(skip(self), fields(subsys = "Services"))]
    pub fn exists(&self, service_id: &ServiceId) -> Result<bool> {
        Ok(self.db_storage.services.contains_key(service_id))
    }

    pub fn is_active(&self, service_id: &ServiceId) -> bool {
        self.get(service_id)
            .map(|service| match service.status {
                ServiceStatus::Active => true,
                ServiceStatus::Paused => false,
            })
            .unwrap_or(false)
    }

    #[instrument(skip(self), fields(subsys = "Services"))]
    pub fn remove(&self, service_id: &ServiceId) -> Result<()> {
        self.db_storage.services.remove(service_id);
        Ok(())
    }

    #[instrument(skip(self, service), fields(subsys = "Services"))]
    pub async fn save(
        &self,
        service: &Service,
        chain_configs: Arc<RwLock<ChainConfigs>>,
    ) -> Result<()> {
        self.db_storage
            .services
            .insert(service.id(), service.clone())
            .map_err(ServicesError::from)?;

        // In addition to populating the cache, this gives us an early sanity check that the service manager is valid
        if let ServiceManager::Stellar { chain, address } = &service.manager {
            let chain_cfg = {
                chain_configs
                    .read()
                    .unwrap()
                    .get_chain(chain)
                    .and_then(|c| match c {
                        AnyChainConfig::Stellar(cfg) => Some(cfg),
                        _ => None,
                    })
                    .ok_or_else(|| ServicesError::MissingChainConfig(chain.clone()))?
                    .clone()
            };

            // Build a fresh soroban env per call. Cheap (no network
            // handshake until a query is actually fired) and sidesteps
            // any caching invariants.
            let env = wasi_soroban_rs::Env::new(wasi_soroban_rs::EnvConfigs {
                rpc_url: chain_cfg.rpc_url.clone(),
                network_passphrase: chain_cfg.network_passphrase.clone(),
            })
            .map_err(|e| ServicesError::StellarChainQuery {
                chain: chain.clone(),
                detail: format!("soroban env: {e:?}"),
            })?;

            // We need a "source account" to build the simulation tx. The
            // signing key never gets used (simulation doesn't sign), so a
            // throwaway account is fine.
            let source_account = wasi_soroban_rs::Account::single(wasi_soroban_rs::Signer::new(
                STELLAR_QUERY_KEY.clone(),
            ));

            let project_root_client =
                ProjectRootClient::new(wasi_soroban_rs::ClientContractConfigs {
                    contract_id: *address,
                    env,
                    source_account,
                });

            let verifier = stellar_xdr::curr::ContractId(
                project_root_client
                    .verification_contract()
                    .await
                    .map_err(|e| ServicesError::StellarChainQuery {
                        chain: chain.clone(),
                        detail: format!("project_root.verification_contract: {e:?}"),
                    })?
                    .0
                    .into(),
            );

            let security = stellar_xdr::curr::ContractId(
                project_root_client
                    .security_contract()
                    .await
                    .map_err(|e| ServicesError::StellarChainQuery {
                        chain: chain.clone(),
                        detail: format!("project_root.security: {e:?}"),
                    })?
                    .0
                    .into(),
            );

            let contracts = StellarServiceManagerContracts { verifier, security };

            self.db_storage
                .stellar_service_manager_contracts
                .insert(service.id(), contracts)
                .map_err(ServicesError::from)?;
        }

        Ok(())
    }

    #[instrument(skip(self), fields(subsys = "Services"))]
    pub fn get_signature_kind(&self, service_id: &ServiceId) -> Result<SignatureKind> {
        self.db_storage
            .services
            .map_ref(service_id, |service| service.signature_kind())
            .ok_or_else(|| ServicesError::UnknownService(service_id.clone()))
    }

    #[instrument(skip(self), fields(subsys = "Services"))]
    pub fn list(
        &self,
        bounds_start: Bound<&ServiceId>,
        bounds_end: Bound<&ServiceId>,
    ) -> Result<Vec<Service>> {
        let mut services = BTreeMap::new();

        for entry in self.db_storage.services.iter() {
            let (key, value) = entry.pair();
            services.insert(key.clone(), value.clone());
        }

        let convert_bound = |bound: Bound<&ServiceId>| match bound {
            Bound::Unbounded => Bound::Unbounded,
            Bound::Included(id) => Bound::Included(id.clone()),
            Bound::Excluded(id) => Bound::Excluded(id.clone()),
        };

        let start = convert_bound(bounds_start);
        let end = convert_bound(bounds_end);

        let services = services
            .range((start, end))
            .map(|(_, service)| service.clone())
            .collect();

        Ok(services)
    }
}

#[derive(Error, Debug)]
pub enum ServicesError {
    #[error("Unknown Service {0}")]
    UnknownService(ServiceId),

    #[error("Unknown Workflow {workflow_id} for Service {service_name} (id: {service_id})")]
    UnknownWorkflow {
        service_name: String,
        service_id: ServiceId,
        workflow_id: WorkflowId,
    },

    #[error("Database error: {0}")]
    DBError(#[from] DBError),

    #[error("Chain config not found for chain {0}")]
    MissingChainConfig(ChainKey),

    #[error("Stellar chain query failed for chain {chain}: {detail}")]
    StellarChainQuery { chain: ChainKey, detail: String },

    #[error("Unknown Service (for stellar service manager): {0}")]
    UnknownServiceForStellarServiceManager(ServiceId),
}

#[macro_export]
macro_rules! tracing_service_info {
    ($services:expr, $service_id:expr, $($msg:tt)*) => {
        if tracing::enabled!(tracing::Level::INFO) {
            match $services.get(&$service_id).ok() {
                Some(service) => {
                    tracing::info!(service.name = %service.name, service.manager = ?service.manager, "{}", format_args!($($msg)*));
                },
                None => {
                    tracing::info!(service.id = %$service_id, "{}", format_args!($($msg)*));
                }
            }
        }
    };
}

#[macro_export]
macro_rules! tracing_service_debug {
    ($services:expr, $service_id:expr, $($msg:tt)*) => {
        if tracing::enabled!(tracing::Level::DEBUG) {
            match $services.get(&$service_id).ok() {
                Some(service) => {
                    tracing::debug!(service.name = %service.name, service.manager = ?service.manager, "{}", format_args!($($msg)*));
                },
                None => {
                    tracing::debug!(service.id = %$service_id, "{}", format_args!($($msg)*));
                }
            }
        }
    };
}

#[macro_export]
macro_rules! tracing_service_trace {
    ($services:expr, $service_id:expr, $($msg:tt)*) => {
        if tracing::enabled!(tracing::Level::TRACE) {
            match $services.get(&$service_id).ok() {
                Some(service) => {
                    tracing::trace!(service.name = %service.name, service.manager = ?service.manager, "{}", format_args!($($msg)*));
                },
                None => {
                    tracing::trace!(service.id = %$service_id, "{}", format_args!($($msg)*));
                }
            }
        }
    };
}

#[macro_export]
macro_rules! tracing_service_warn {
    ($services:expr, $service_id:expr, $($msg:tt)*) => {
        if tracing::enabled!(tracing::Level::WARN) {
            match $services.get(&$service_id).ok() {
                Some(service) => {
                    tracing::warn!(service.name = %service.name, service.manager = ?service.manager, "{}", format_args!($($msg)*));
                },
                None => {
                    tracing::warn!(service.id = %$service_id, "{}", format_args!($($msg)*));
                }
            }
        }
    };
}

#[macro_export]
macro_rules! tracing_service_error {
    ($services:expr, $service_id:expr, $($msg:tt)*) => {
        if tracing::enabled!(tracing::Level::ERROR) {
            match $services.get(&$service_id).ok() {
                Some(service) => {
                    tracing::error!(service.name = %service.name, service.manager = ?service.manager, "{}", format_args!($($msg)*));
                },
                None => {
                    tracing::error!(service.id = %$service_id, "{}", format_args!($($msg)*));
                }
            }
        }
    };
}
