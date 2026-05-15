//! Receive-time packet validation.
//!
//! Per the design discussed in
//! <https://github.com/warp-driver/warpdrive/issues/33#issuecomment-4387393515>
//! and <https://github.com/warp-driver/warpdrive/issues/29>, every
//! inbound submission is validated against the manager-chain operator
//! set *before* it reaches the wasm aggregator component or the
//! quorum queue. Invalid packets are dropped with a metric increment;
//! the wasm `process_input` path never sees them.
//!
//! ## Per-event reference block
//!
//! On the first valid packet for a given `(service_id, event_id)` we
//! pin the manager chain's current block height (or ledger sequence)
//! into `WavsDb::event_reference_blocks`. Every subsequent packet for
//! the same event is validated against that pinned block, so the
//! operator set is frozen for the aggregation window — a signer
//! either passes for all packets in the window or none.
//!
//! merkle-maren's clarification on #33 matters here: we pin only on
//! the first **valid** packet. A flood of invalid packets at the
//! start of an event repeatedly fetches "current block" but never
//! pins, so an attacker can't choose the reference block for us.
//!
//! ## Per-chain support today
//!
//! - **Stellar**: strict historical via
//!   `Secp256k1VerificationClient::check_one(env, sig, pubkey, Some(ref_block))`.
//! - **EVM**: strict historical via alloy's `.block(BlockId::Number(N))`
//!   on `IWarpDriveServiceManager::getLatestOperatorForSigningKey`
//!   followed by `getOperatorWeight`. No contract changes needed; the
//!   EVM RPC just reads chain state at N.
//! - **Cosmos**: current-block only — `layer_climb`'s `contract_smart`
//!   doesn't expose a height parameter, so we query
//!   `WarpDriveOperatorWeight` against the current chain state.
//!   Documented as a TODO; tracked in a follow-up issue.

use alloy_provider::Provider;
use alloy_rpc_types_eth::BlockId;
use layer_climb::prelude::EvmAddr as ClimbEvmAddr;
use std::collections::HashMap;
use std::sync::Arc;
use utils::{
    config::EvmChainConfigExt,
    evm_client::{EvmEndpoint, EvmQueryClient},
    stellar_client::STELLAR_QUERY_KEY,
};
use warpdrive_client::{
    ed25519_verification::Ed25519VerificationClient,
    secp256k1_verification::Secp256k1VerificationClient,
};
use warpdrive_types::{
    contracts::cosmwasm::service_manager::ServiceManagerQueryMessages, AnyChainConfig, ChainKey,
    EventId, IWarpDriveServiceManager::IWarpDriveServiceManagerInstance, Service, ServiceManager,
    Submission, WavsSignable, WavsSignature,
};

use crate::subsystems::aggregator::{error::AggregatorError, Aggregator};

/// Stable label values used by the `aggregator.packets_rejected_count`
/// metric. Each kept short and lowercase so they're idiomatic in
/// PromQL queries.
pub mod reject_reason {
    pub const BAD_SIG: &str = "bad_sig";
    pub const UNREGISTERED: &str = "unregistered";
    pub const CHAIN_QUERY_FAILED: &str = "chain_query_failed";
}

/// Map a receive-validation error into a stable `reason` label for
/// the rejection metric. Kept here (next to the validators) so the
/// label set evolves with the error variants.
pub fn reject_reason_for(err: &AggregatorError) -> &'static str {
    match err {
        AggregatorError::InvalidPacketSignature(_) => reject_reason::BAD_SIG,
        AggregatorError::SignerUnregisteredAtReceive { .. } => reject_reason::UNREGISTERED,
        AggregatorError::ReceiveValidationChainQuery { .. } => reject_reason::CHAIN_QUERY_FAILED,
        AggregatorError::MissingChainConfig(_) => reject_reason::CHAIN_QUERY_FAILED,
        // Anything unexpected here would be a logic bug; fall back to
        // the general "chain_query_failed" bucket so we still get a
        // metric tick rather than dropping the count silently.
        _ => reject_reason::CHAIN_QUERY_FAILED,
    }
}

impl Aggregator {
    /// Validate an inbound packet at receive time.
    ///
    /// Returns `Ok(reference_block_used)` on success — the caller pins
    /// this value into `event_reference_blocks` if it wasn't already
    /// pinned. Returns a typed `AggregatorError` on failure; the
    /// caller drops the packet, increments the rejection metric with
    /// `reject_reason_for(&err)`, and does NOT forward to the wasm
    /// component.
    pub async fn validate_packet_at_receive(
        &self,
        submission: &Submission,
        service: &Service,
    ) -> Result<u64, AggregatorError> {
        let pinned = self.get_pinned_reference_block(&submission.event_id);

        match &service.manager {
            ServiceManager::Stellar { chain, address: _ } => {
                self.validate_packet_stellar(submission, chain, pinned)
                    .await
            }
            ServiceManager::Evm { chain, address } => {
                self.validate_packet_evm(submission, *address, chain, pinned)
                    .await
            }
            ServiceManager::Cosmos { chain, address } => {
                self.validate_packet_cosmos(submission, address.clone(), chain, pinned)
                    .await
            }
        }
    }

    /// Persist the pinned reference block for `(service_id, event_id)`.
    /// First-write wins; subsequent calls (from later valid packets in
    /// the same aggregation window) are no-ops.
    pub fn pin_event_reference_block_if_unset(&self, event_id: &EventId, block: u64) {
        if self
            .storage
            .event_reference_blocks
            .get_cloned(event_id)
            .is_none()
        {
            // Best-effort insert. If two concurrent valid packets race
            // and both call this, the second call's `get_cloned` would
            // see `Some` and skip; if both pass the check (rare), the
            // second `insert` overwrites with what would be roughly
            // the same value (same chain block ± 1). The serializer in
            // the dispatch path doesn't lock at receive granularity,
            // so we accept the harmless drift.
            if let Err(err) = self
                .storage
                .event_reference_blocks
                .insert(event_id.clone(), block)
            {
                tracing::warn!(
                    "Aggregator: failed to pin event_reference_block: {err:?} (validation continues using the block we just queried; submit path will fall back to current chain state if the pin is missing)"
                );
            }
        }
    }

    /// Look up the pinned reference block for `(service_id, event_id)`,
    /// set by the receive-time validation gate. Returns `None` if no
    /// valid packet has been seen for this event yet — which under
    /// normal operation shouldn't happen at submit time (we only get
    /// here from an aggregator action triggered by a packet that
    /// passed receive validation), but the submit path falls back to
    /// the chain's current block with a warn log so a missing pin
    /// degrades gracefully rather than panicking.
    pub fn get_pinned_reference_block(&self, event_id: &EventId) -> Option<u64> {
        self.storage.event_reference_blocks.get_cloned(event_id)
    }

    // ── Stellar ──────────────────────────────────────────────────

    async fn validate_packet_stellar(
        &self,
        submission: &Submission,
        chain: &ChainKey,
        pinned: Option<u64>,
    ) -> Result<u64, AggregatorError> {
        let chain_cfg = self
            .config
            .chains
            .read()
            .unwrap()
            .get_chain(chain)
            .and_then(|c| match c {
                AnyChainConfig::Stellar(cfg) => Some(cfg),
                _ => None,
            })
            .ok_or_else(|| AggregatorError::MissingChainConfig(chain.clone()))?;

        // Build a fresh soroban env per call. Cheap (no network
        // handshake until a query is actually fired) and sidesteps
        // any caching invariants.
        let env = wasi_soroban_rs::Env::new(wasi_soroban_rs::EnvConfigs {
            rpc_url: chain_cfg.rpc_url.clone(),
            network_passphrase: chain_cfg.network_passphrase.clone(),
        })
        .map_err(|e| AggregatorError::ReceiveValidationChainQuery {
            chain: chain.clone(),
            detail: format!("soroban env: {e:?}"),
        })?;

        // Pick the reference block: pinned if we have one, else the
        // current ledger sequence.
        let ref_block = match pinned {
            Some(b) => b,
            None => {
                // unlike in `handle_action_submit_stellar`, we don't necessarily have a pinned reference block
                // since there may not have been a prior valid packet for this event
                // so it's not a warning, just the expected path for the first valid packet of an event
                let rpc =
                    wasi_stellar_rpc_client::Client::new(&chain_cfg.rpc_url).map_err(|e| {
                        AggregatorError::ReceiveValidationChainQuery {
                            chain: chain.clone(),
                            detail: format!("stellar rpc client: {e:?}"),
                        }
                    })?;
                let current = rpc
                    .get_latest_ledger()
                    .await
                    .map_err(|e| AggregatorError::ReceiveValidationChainQuery {
                        chain: chain.clone(),
                        detail: format!("get_latest_ledger: {e:?}"),
                    })?
                    .sequence as u64;

                current.saturating_sub(1)
            }
        };

        // We need a "source account" to build the simulation tx. The
        // signing key never gets used (simulation doesn't sign), so a
        // throwaway account is fine.
        let account = wasi_soroban_rs::Account::single(wasi_soroban_rs::Signer::new(
            STELLAR_QUERY_KEY.clone(),
        ));

        let verification_contract = self
            .services
            .get_stellar_service_manager_contracts(submission.service_id())?
            .verifier;

        // Build the canonical envelope bytes the operator signed; the
        // verification contract recomputes the same hash internally.
        let envelope_bytes = submission
            .envelope
            .encode_data()
            .map_err(|e| AggregatorError::InvalidPacketSignature(format!("envelope: {e:?}")))?;

        let (res, signer_pubkey_hex) = match submission.envelope_signature {
            WavsSignature::Secp256k1 { signature, .. } => {
                let pubkey = submission
                    .envelope_signature
                    .secp256k1_compressed_pubkey(&submission.envelope)?;

                let res =
                    Secp256k1VerificationClient::new(wasi_soroban_rs::ClientContractConfigs {
                        contract_id: stellar_strkey::Contract(verification_contract.0.into()),
                        env,
                        source_account: account,
                    })
                    .check_one(
                        envelope_bytes,
                        signature.into_inner(),
                        pubkey,
                        Some(ref_block as u32),
                    )
                    .await;

                (res, const_hex::encode(pubkey))
            }
            WavsSignature::Ed25519 { signature, pubkey } => {
                let res = Ed25519VerificationClient::new(wasi_soroban_rs::ClientContractConfigs {
                    contract_id: stellar_strkey::Contract(verification_contract.0.into()),
                    env,
                    source_account: account,
                })
                .check_one(
                    envelope_bytes,
                    signature.into_inner(),
                    pubkey.into_inner(),
                    Some(ref_block as u32),
                )
                .await;

                (res, const_hex::encode(pubkey.as_slice()))
            }
        };

        // `check_one` returns the signer's weight at `reference_block`,
        // or a contract-error string on rejection. Treat
        // `Error(Contract, #302)` (SignerNotRegistered) as the
        // unregistered-at-receive case; treat any other error as a
        // chain-query failure (transient). A return of 0 is also
        // "unregistered" — be explicit either way.
        match res {
            Ok(weight) if weight > 0 => Ok(ref_block),
            Ok(_zero_weight) => Err(AggregatorError::SignerUnregisteredAtReceive {
                chain: chain.clone(),
                signer_pubkey_hex,
                block: ref_block,
            }),
            Err(err) => {
                let err_str = format!("{err:?}");
                if err_str.contains("Error(Contract, #302)")
                    || err_str.contains("Error(Contract, #301)")
                {
                    // 301 InvalidSignature, 302 SignerNotRegistered.
                    // Both mean "this packet's signer is not in the
                    // operator set at this block, drop the packet".
                    Err(AggregatorError::SignerUnregisteredAtReceive {
                        chain: chain.clone(),
                        signer_pubkey_hex,
                        block: ref_block,
                    })
                } else {
                    Err(AggregatorError::ReceiveValidationChainQuery {
                        chain: chain.clone(),
                        detail: format!("check_one: {err_str}"),
                    })
                }
            }
        }
    }

    // ── EVM ──────────────────────────────────────────────────────

    async fn validate_packet_evm(
        &self,
        submission: &Submission,
        manager_address: alloy_primitives::Address,
        chain: &ChainKey,
        pinned: Option<u64>,
    ) -> Result<u64, AggregatorError> {
        // Recover the operator address. Offline; bad sigs fail here
        // before any RPC.
        let signer_addr = submission
            .envelope_signature
            .signer_address(&submission.envelope)
            .map_err(|e| AggregatorError::InvalidPacketSignature(format!("{e:?}")))?
            .try_as_evm()
            .ok_or_else(|| {
                AggregatorError::InvalidPacketSignature(
                    "EVM service-manager validation requires a secp256k1 signature".to_string(),
                )
            })?;

        let query_client = self
            .get_or_create_evm_query_client(chain)
            .await
            .map_err(|e| AggregatorError::ReceiveValidationChainQuery {
                chain: chain.clone(),
                detail: format!("evm query client: {e:?}"),
            })?;

        let ref_block = match pinned {
            Some(b) => b,
            None => {
                let current = query_client
                    .provider
                    .get_block_number()
                    .await
                    .map_err(|e| AggregatorError::ReceiveValidationChainQuery {
                        chain: chain.clone(),
                        detail: format!("eth_blockNumber: {e:?}"),
                    })?;

                current.saturating_sub(1)
            }
        };

        // The one-liner historical-state win: alloy's
        // `.block(BlockId::Number(N))` makes `eth_call` read state at
        // block N. We do the operator-set lookup as a two-step at
        // block N: first map the recovered signing-key address to its
        // operator, then read that operator's weight. Production
        // service managers (e.g. POAStakeRegistry) key
        // `operatorWeights` by operator address — not by signing-key
        // address — so calling `getOperatorWeight(signer_addr)`
        // directly always returns zero. The local mock
        // `SimpleServiceManager` papers over this by returning the
        // input from `getLatestOperatorForSigningKey`, so this
        // two-step works for both.
        let service_manager =
            IWarpDriveServiceManagerInstance::new(manager_address, query_client.provider.clone());

        let operator_addr = service_manager
            .getLatestOperatorForSigningKey(signer_addr)
            .block(BlockId::Number(ref_block.into()))
            .call()
            .await
            .map_err(|e| AggregatorError::ReceiveValidationChainQuery {
                chain: chain.clone(),
                detail: format!(
                    "getLatestOperatorForSigningKey at block {ref_block}: {e:?}"
                ),
            })?;

        if operator_addr == alloy_primitives::Address::ZERO {
            return Err(AggregatorError::SignerUnregisteredAtReceive {
                chain: chain.clone(),
                signer_pubkey_hex: format!("{signer_addr:?}"),
                block: ref_block,
            });
        }

        let weight = service_manager
            .getOperatorWeight(operator_addr)
            .block(BlockId::Number(ref_block.into()))
            .call()
            .await
            .map_err(|e| AggregatorError::ReceiveValidationChainQuery {
                chain: chain.clone(),
                detail: format!("getOperatorWeight at block {ref_block}: {e:?}"),
            })?;

        if weight.is_zero() {
            return Err(AggregatorError::SignerUnregisteredAtReceive {
                chain: chain.clone(),
                signer_pubkey_hex: format!("{signer_addr:?}"),
                block: ref_block,
            });
        }

        Ok(ref_block)
    }

    // ── Cosmos ───────────────────────────────────────────────────

    async fn validate_packet_cosmos(
        &self,
        submission: &Submission,
        manager_address: layer_climb::prelude::CosmosAddr,
        chain: &ChainKey,
        pinned: Option<u64>,
    ) -> Result<u64, AggregatorError> {
        // Recover the EVM-shaped operator address. Cosmos service
        // managers index operators by their EVM/secp256k1 address
        // (see `WarpDriveOperatorWeight { vector_address: EvmAddr }`)
        // because the same secp256k1 keypair signs across all chains.
        let signer_addr = submission
            .envelope_signature
            .signer_address(&submission.envelope)
            .map_err(|e| AggregatorError::InvalidPacketSignature(format!("{e:?}")))?
            .try_as_evm()
            .ok_or_else(|| {
                AggregatorError::InvalidPacketSignature(
                    "Cosmos service-manager validation requires a secp256k1 signature".to_string(),
                )
            })?;
        // Re-encode into layer-climb's `EvmAddr` shape (it has a
        // `From<alloy_primitives::Address>` impl).
        let climb_signer_addr: ClimbEvmAddr = signer_addr.into();

        let query_client = self
            .get_or_create_cosmos_query_client(chain)
            .await
            .map_err(|e| AggregatorError::ReceiveValidationChainQuery {
                chain: chain.clone(),
                detail: format!("cosmos query client: {e:?}"),
            })?;

        // TODO(warp-driver/warpdrive#TODO_COSMOS_HISTORICAL): query
        // operator weight at `pinned` if it's set, not at current
        // block. layer_climb's `contract_smart` doesn't expose a
        // height parameter today — both paths below read current
        // chain state. The pinned ref_block from prior packets is
        // still respected for the *value we record*, just not for
        // the operator-weight check itself, so there's a small race
        // window where a signer who was registered at ref_block but
        // has since deregistered would pass receive validation here.
        // The Stellar/EVM paths above don't have this race.
        let ref_block = match pinned {
            Some(b) => b,
            None => {
                let current = query_client.block_height().await.map_err(|e| {
                    AggregatorError::ReceiveValidationChainQuery {
                        chain: chain.clone(),
                        detail: format!("block_height: {e:?}"),
                    }
                })?;

                current.saturating_sub(1)
            }
        };

        let weight: cosmwasm_std::Uint256 = query_client
            .contract_smart(
                &manager_address.into(),
                &ServiceManagerQueryMessages::WarpDriveOperatorWeight {
                    vector_address: climb_signer_addr,
                },
            )
            .await
            .map_err(|e| AggregatorError::ReceiveValidationChainQuery {
                chain: chain.clone(),
                detail: format!("WarpDriveOperatorWeight: {e:?}"),
            })?;

        if weight.is_zero() {
            return Err(AggregatorError::SignerUnregisteredAtReceive {
                chain: chain.clone(),
                signer_pubkey_hex: format!("{signer_addr:?}"),
                block: ref_block,
            });
        }

        Ok(ref_block)
    }

    // ── Read-only chain client cache ─────────────────────────────
    //
    // Receive-time validation is read-only and may target a chain
    // the aggregator doesn't sign on (e.g. manager on EVM, submit on
    // Stellar). Build query-only clients on demand so we don't
    // require credentials for the validation chain.

    async fn get_or_create_evm_query_client(
        &self,
        chain: &ChainKey,
    ) -> Result<EvmQueryClient, anyhow::Error> {
        if let Some(client) = self.evm_query_clients.read().unwrap().get(chain).cloned() {
            return Ok(client);
        }

        let chain_config = self
            .config
            .chains
            .read()
            .unwrap()
            .get_chain(chain)
            .and_then(|c| match c {
                AnyChainConfig::Evm(cfg) => Some(cfg),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("EVM chain config not found for {chain}"))?;

        let endpoints: Vec<EvmEndpoint> = chain_config.query_client_endpoints()?;
        let endpoint = endpoints
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no EVM query endpoints for {chain}"))?;
        let client = EvmQueryClient::new(endpoint).await?;
        self.evm_query_clients
            .write()
            .unwrap()
            .insert(chain.clone(), client.clone());
        Ok(client)
    }

    async fn get_or_create_cosmos_query_client(
        &self,
        chain: &ChainKey,
    ) -> Result<layer_climb::prelude::QueryClient, anyhow::Error> {
        if let Some(client) = self
            .cosmos_query_clients
            .read()
            .unwrap()
            .get(chain)
            .cloned()
        {
            return Ok(client);
        }

        let chain_config = self
            .config
            .chains
            .read()
            .unwrap()
            .get_chain(chain)
            .and_then(|c| match c {
                AnyChainConfig::Cosmos(cfg) => Some(cfg),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("Cosmos chain config not found for {chain}"))?;

        let client = layer_climb::prelude::QueryClient::new(chain_config.into(), None).await?;
        self.cosmos_query_clients
            .write()
            .unwrap()
            .insert(chain.clone(), client.clone());
        Ok(client)
    }
}

/// Type alias kept short for the per-chain query-client caches. The
/// indirection makes the `Aggregator` struct fields readable.
pub type EvmQueryClientMap = Arc<std::sync::RwLock<HashMap<ChainKey, EvmQueryClient>>>;
pub type CosmosQueryClientMap =
    Arc<std::sync::RwLock<HashMap<ChainKey, layer_climb::prelude::QueryClient>>>;

#[cfg(test)]
mod reject_reason_tests {
    use super::*;
    use warpdrive_types::ChainKeyId;

    fn dummy_chain() -> ChainKey {
        ChainKey {
            namespace: "evm".parse().unwrap(),
            id: ChainKeyId::new("test-chain").unwrap(),
        }
    }

    #[test]
    fn invalid_packet_signature_maps_to_bad_sig() {
        let err = AggregatorError::InvalidPacketSignature("malformed".to_string());
        assert_eq!(reject_reason_for(&err), reject_reason::BAD_SIG);
    }

    #[test]
    fn signer_unregistered_maps_to_unregistered() {
        let err = AggregatorError::SignerUnregisteredAtReceive {
            chain: dummy_chain(),
            signer_pubkey_hex: "deadbeef".to_string(),
            block: 42,
        };
        assert_eq!(reject_reason_for(&err), reject_reason::UNREGISTERED);
    }

    #[test]
    fn chain_query_failure_maps_to_chain_query_failed() {
        let err = AggregatorError::ReceiveValidationChainQuery {
            chain: dummy_chain(),
            detail: "rpc timeout".to_string(),
        };
        assert_eq!(reject_reason_for(&err), reject_reason::CHAIN_QUERY_FAILED);
    }

    #[test]
    fn missing_chain_config_maps_to_chain_query_failed() {
        // Missing chain config also surfaces from the validation
        // path; the operator can't validate without it. Same metric
        // bucket as a transient query failure.
        let err = AggregatorError::MissingChainConfig(dummy_chain());
        assert_eq!(reject_reason_for(&err), reject_reason::CHAIN_QUERY_FAILED);
    }

    #[test]
    fn unrelated_variant_falls_back_to_chain_query_failed() {
        // A logic-bug case: receive validation should never produce
        // these variants. We still want a metric tick rather than a
        // silent drop, so the fallback bucket catches them.
        let err = AggregatorError::Stellar("would never come from validate.rs".to_string());
        assert_eq!(reject_reason_for(&err), reject_reason::CHAIN_QUERY_FAILED);
    }
}
