use alloy_rpc_types_eth::TransactionReceipt;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvmClientError {
    #[error("HD index must be zero when using a private key (use mnemonic instead)")]
    DerivationWithPrivateKey,

    #[error("Contract not deployed {0}")]
    ContractNotDeployed(alloy_primitives::Address),

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

    #[error("Unable to sign: {0:#?}")]
    Signing(anyhow::Error),

    #[error("Unable to estimate gas: {0:#?}")]
    GasEstimation(anyhow::Error),

    #[error("Unable to recover signer address: {0:#?}")]
    RecoverSignerAddress(anyhow::Error),

    #[error("Unable to parse endpoint: {0}")]
    ParseEndpoint(String),

    #[error("Unable to create web socket provider: {0:#?}")]
    WebSocketProvider(anyhow::Error),

    #[error("Unable to create http provider: {0:#?}")]
    HttpProvider(anyhow::Error),

    #[error("Unable to get block height")]
    BlockHeight,

    #[error("Unsupported envelope type: {0}")]
    UnsupportedEnvelopeType(String),
}

pub type StellarClientResult<T> = std::result::Result<T, StellarClientError>;

#[derive(Debug, Error)]
pub enum StellarClientError {
    #[error("Not implemented")]
    NotImplemented,

    #[error("RPC error: {0}")]
    Rpc(#[from] wasi_stellar_rpc_client::Error),

    #[error("Invalid ledger range, start: {start} end: {end}")]
    InvalidLedgerRange { start: u32, end: u32 },

    #[error("Too many topic segments provided (max is 4, got {0})")]
    TooManyTopicSegments(usize),

    #[error("Invalid wildcard segment: {0}")]
    InvalidWildcard(String),

    #[error("The \"**\" wildcard is only supported as the final topic segment")]
    RestWildcardMustBeLast,

    #[error("Failed to parse xdr: {0}")]
    FilterParse(#[from] stellar_xdr::curr::Error),
}
