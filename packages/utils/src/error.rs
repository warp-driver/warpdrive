use alloy_rpc_types_eth::TransactionReceipt;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvmClientError {
    #[error("HD index must be zero when using a private key (use mnemonic instead)")]
    DerivationWithPrivateKey,

    #[error("Address is not a contract: {0}")]
    NotContract(alloy_primitives::Address),

    #[error("Could not get contract code at {0}: {1:?}")]
    FailedGetCode(alloy_primitives::Address, anyhow::Error),

    #[error("Send Transaction Error: {0}")]
    SendTransaction(anyhow::Error),

    #[error("No Transaction Receipt: {0}")]
    TransactionWithoutReceipt(anyhow::Error),

    #[error("Transaction Receipt: {0:#?}")]
    TransactionWithReceipt(Box<TransactionReceipt>),

    #[error("Unable to parse endpoint: {0}")]
    ParseEndpoint(String),

    #[error("Unable to create web socket provider: {0:#?}")]
    WebSocketProvider(anyhow::Error),

    #[error("Unsupported envelope type: {0}")]
    UnsupportedEnvelopeType(String),
}

pub type StellarClientResult<T> = std::result::Result<T, StellarClientError>;

#[derive(Debug, Error)]
pub enum StellarClientError {
    #[error("Not implemented")]
    NotImplemented,

    #[error("RPC reported unhealthy status: {0}")]
    UnhealthyStatus(String),

    #[error("RPC error: {0}")]
    Rpc(#[from] wasi_stellar_rpc_client::Error),

    #[error("Invalid ledger range, start: {start} end: {end}")]
    InvalidLedgerRange { start: u32, end: u32 },

    #[error("Invalid wildcard segment: {0}")]
    InvalidWildcard(String),

    #[error("Failed to parse xdr: {0}")]
    FilterParse(#[from] stellar_xdr::curr::Error),
}
