use thiserror::Error;
use warpdrive_types::ServiceId;

use crate::{dispatcher::DispatcherCommand, services::ServicesError};

#[derive(Error, Debug)]
pub enum SubmissionError {
    #[error("missing signing mnemonic")]
    MissingSigningMnemonic,
    #[error("missing key for service {service_id}")]
    MissingServiceKey { service_id: ServiceId },
    #[error("aggregator: {0}")]
    Aggregator(String),
    #[error("failed to sign envelope: {0:?}")]
    FailedToSignEnvelope(anyhow::Error),
    #[error("missing EVM signer for service {0}")]
    MissingEvmSigner(ServiceId),
    #[error("failed to create EVM signer for service {0}: {1:?}")]
    FailedToCreateEvmSigner(ServiceId, anyhow::Error),
    #[error("failed to create Stellar signer for service {0}: {1:?}")]
    FailedToCreateStellarSigner(ServiceId, anyhow::Error),
    #[error("services {0:?}")]
    Services(#[from] ServicesError),
    #[error("Submission channel error: {0}")]
    SendCommand(#[from] Box<crossbeam::channel::SendError<DispatcherCommand>>),
    #[error("could not encode EventId {0:?}")]
    EncodeEventId(bincode::error::EncodeError),
    #[error("HD index overflow")]
    HdIndexOverflow,
    #[error("Error with soroban-rs client: {0}")]
    SorobanClient(wasi_soroban_rs::SorobanHelperError),
}

impl From<wasi_soroban_rs::SorobanHelperError> for SubmissionError {
    fn from(value: wasi_soroban_rs::SorobanHelperError) -> Self {
        Self::SorobanClient(value)
    }
}
