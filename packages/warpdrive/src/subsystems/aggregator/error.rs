use thiserror::Error;
use utils::error::EvmClientError;
use warpdrive_types::{
    contracts::cosmwasm::service_manager::error::WavsValidateError, ChainConfigError, ChainKey,
    QuorumQueueId, ServiceManagerError, SigningError,
};

use crate::{
    dispatcher::DispatcherCommand, services::ServicesError,
    subsystems::aggregator::AggregatorCommand,
};

#[derive(Error, Debug)]
pub enum AggregatorError {
    #[error("DB: {0}")]
    Db(anyhow::Error),

    #[error("EVM client: {0}")]
    EvmClient(#[from] EvmClientError),

    #[error("Cosmos Client: {0}")]
    CosmosClient(anyhow::Error),

    #[error("Aggregator channel error: {0}")]
    SendCommandToSelf(#[from] Box<crossbeam::channel::SendError<AggregatorCommand>>),

    #[error("Aggregator channel error: {0}")]
    SendCommandToDispatcher(#[from] Box<crossbeam::channel::SendError<DispatcherCommand>>),

    #[error("Chain Config: {0}")]
    Config(#[from] ChainConfigError),

    #[error("Evm client create: {0:?}")]
    CreateEvmClient(anyhow::Error),

    #[error("Service manager validate(): {0:?}")]
    CosmosServiceManagerValidate(WavsValidateError),

    #[error("Service manager validate(): {0:?}")]
    EvmServiceManagerValidateKnown(ServiceManagerError),

    #[error("Service manager validate(): {0}")]
    EvmServiceManagerValidateAnyRevert(String),

    #[error("Service manager validate(): {0:?}")]
    EvmServiceManagerValidateUnknown(alloy_contract::Error),

    #[error("Insufficient Quorum: weight {signer_weight} < threshold {threshold_weight} (total weight: {total_weight})")]
    InsufficientQuorum {
        signer_weight: String,
        threshold_weight: String,
        total_weight: String,
    },

    #[error("Unable to look up service manager from service handler: {0:?}")]
    EvmServiceManagerLookup(alloy_contract::Error),

    #[error("Block number: {0}")]
    BlockNumber(anyhow::Error),

    #[error("Join: {0}")]
    JoinError(String),

    #[error("Services: {0}")]
    ServicesError(#[from] ServicesError),

    #[error("Queue is empty: {0:?}")]
    QueueIsEmpty(QuorumQueueId),

    #[error("Signing: {0:?}")]
    Signing(#[from] SigningError),

    #[error("Unexpected envelope difference for queue {0:?}")]
    EnvelopeDiff(QuorumQueueId),

    #[error("P2P: {0}")]
    P2p(String),

    #[error("Stellar submit: {0}")]
    Stellar(String),

    /// A signer in the submission queue isn't registered on-chain yet.
    /// Treated as transient by the dispatch loop: the queue is saved and
    /// retried when later submissions arrive (the assumption being that
    /// the missing vector finishes registering shortly). Common during
    /// multi-vector startup, especially with PoA middleware whose
    /// sequential `docker exec` calls are slow.
    #[error("Signer not registered (transient): {0}")]
    SignerNotRegistered(String),

    /// The signing credential for the target chain is missing or
    /// unparseable. Treated as transient by the dispatch loop because
    /// the typical cause is a sysadmin who hasn't finished propagating
    /// config; once the credential is in place and the node is
    /// restarted (or reloaded, if/when that exists), retries pick up
    /// from the saved queue.
    ///
    /// **Important**: prior to this variant, the lazy-credential-fetch
    /// path returned `Ok(None)` which the dispatch loop treated as
    /// "nothing to do" — silently dropping the inbound submission.
    /// Returning a typed error instead routes through the normal
    /// `save_quorum_queue` path so the submission isn't lost.
    #[error("Missing or invalid {chain_kind} credential for chain {chain}: {detail}")]
    MissingCredential {
        chain_kind: &'static str,
        chain: ChainKey,
        detail: String,
    },

    /// The chain referenced by a submission isn't in the runtime
    /// chain-config registry. Same transient framing as
    /// `MissingCredential`: the queue is saved so a sysadmin who
    /// adds the chain config (or fixes a typo) doesn't lose the
    /// pending submission.
    #[error("Chain config not found for chain {0}")]
    MissingChainConfig(ChainKey),

    /// At startup, a configured credential failed to parse. Distinct
    /// from `MissingCredential` (which is per-chain at submit time):
    /// at startup we don't have a specific chain in context — the
    /// credentials are global per kind (one EVM, one Cosmos, one
    /// Stellar). Returning this variant from `Aggregator::new` makes
    /// the node refuse to start so the sysadmin sees the misconfig
    /// immediately, not at the first submission for the affected
    /// chain.
    #[error("Invalid {chain_kind} credential at startup: {detail}")]
    InvalidStartupCredential {
        chain_kind: &'static str,
        detail: String,
    },

    // ── Receive-time validation errors ────────────────────────────
    //
    // Returned by `validate_packet_at_receive` / its per-chain
    // helpers. The receive-side caller treats *any* of these as
    // "drop the packet, increment the rejection metric, don't run
    // the wasm aggregator component". They're surfaced as variants
    // (rather than a single string) so the metric label can be a
    // stable enum.
    /// Signature couldn't be recovered into a usable signer (malformed
    /// secp256k1 sig, wrong length, bad EIP-191 hash, etc.). The
    /// signer can never be identified, so there's no way to validate
    /// against the operator set.
    #[error("Invalid packet signature: {0}")]
    InvalidPacketSignature(String),

    /// Recovered signer is not in the operator set as of the
    /// validation block. Either they were never registered, or they
    /// signed before joining (operator-set joins are not
    /// retroactive). Permanent for this packet.
    #[error(
        "Packet signer 0x{signer_pubkey_hex} not registered for chain {chain} at block {block}"
    )]
    SignerUnregisteredAtReceive {
        chain: ChainKey,
        signer_pubkey_hex: String,
        block: u64,
    },

    /// Failed to query the chain for current block height (needed to
    /// pin a fresh `reference_block` for an unseen event) or for
    /// signer weight at a specific block. Drop the packet — without
    /// the chain query we can't validate.
    #[error("Receive-validation chain query failed for chain {chain}: {detail}")]
    ReceiveValidationChainQuery { chain: ChainKey, detail: String },

    /// A chain-specific submit fn was reached with an empty queue.
    /// In normal operation the dispatch closure always
    /// `append_submission_to_queue`s before calling
    /// `handle_action_submit_*`, so the queue has at least the
    /// inbound submission — empty here would be a logic bug. Returns
    /// a typed error rather than panicking on `unwrap` so the
    /// dispatch loop's generic error-arm logging fires and the rest
    /// of the node keeps running.
    #[error("Empty submission queue for {chain_kind} submit")]
    EmptySubmissionQueue { chain_kind: &'static str },

    #[error("expected {expected} envelope, got {received}")]
    UnexpectedEnvelopeKind {
        expected: &'static str,
        received: &'static str,
    },
}
