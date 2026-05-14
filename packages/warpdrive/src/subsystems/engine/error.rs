use thiserror::Error;
use utils::storage::CAStorageError;
use warpdrive_types::{ComponentDigest, ServiceId, WorkflowId};

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("Storage: {0}")]
    Storage(#[from] CAStorageError),

    #[error{"Compile: {0}"}]
    Compile(anyhow::Error),

    #[error("Unknown Workflow {0} / {1}")]
    UnknownWorkflow(ServiceId, WorkflowId),

    #[error("No wasm found for digest {0}")]
    UnknownDigest(ComponentDigest),

    #[error{"{0}"}]
    Engine(#[from] warpdrive_engine::utils::error::EngineError),

    #[error("could not encode EventId {0:?}")]
    EncodeEventId(bincode::error::EncodeError),

    #[error("{0:?}")]
    ConvertAggregatorAction(#[from] anyhow::Error),
}
