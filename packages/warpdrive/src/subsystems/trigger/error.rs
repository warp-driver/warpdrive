use iri_string::types::CreationError;
use thiserror::Error;
use utils::error::StellarClientError;
use warpdrive_types::{ChainKey, ServiceId, WorkflowId};

use crate::{dispatcher::DispatcherCommand, subsystems::trigger::TriggerCommand};

#[derive(Error, Debug)]
pub enum TriggerError {
    #[error("climb: {0}")]
    Climb(anyhow::Error),
    #[error("EvmClient (chain {0}): missing websocket endpoint")]
    EvmMissingWebsocket(ChainKey),
    #[error("Cannot find service: {0}")]
    NoSuchService(ServiceId),
    #[error("Cannot find workflow: {0} / {1}")]
    NoSuchWorkflow(ServiceId, WorkflowId),
    #[error("Cannot find trigger data: {0}")]
    NoSuchTriggerData(usize),
    #[error("Cron scheduling error: {expression} / {reason}")]
    Cron { expression: String, reason: String },
    #[error("Interval start time cannot be after end time")]
    IntervalStartAfterEnd,
    #[error("Dispatcher command send error: {0}")]
    DispatcherCommandSendError(#[from] Box<crossbeam::channel::SendError<DispatcherCommand>>),
    #[error("Local command send error: {0}")]
    LocalCommandError(#[from] tokio::sync::mpsc::error::SendError<TriggerCommand>),
    #[error("Unable to convert event index: {0}")]
    EventIndexConversion(std::num::TryFromIntError),
    #[error("URI creation error: {0}")]
    URICreation(#[from] CreationError<String>),
    #[error("Jetstream connection error: {0}")]
    JetstreamConnection(String),
    #[error("Jetstream configuration error: {0}")]
    JetstreamConfig(String),
    #[error("Jetstream parsing error: {0}")]
    JetstreamParse(String),
    #[error("StellarClient (chain {0}): missing rpc endpoint")]
    StellarMissingRpc(ChainKey),
    #[error("StellarClient: {0}")]
    StellarClient(#[from] StellarClientError),
    #[error("Stellar client for chain {0} not found")]
    StellarMissingClient(ChainKey),
}
