use alloy_provider::{DynProvider, Provider};
use alloy_rpc_types_eth::TransactionReceipt;
use layer_climb::prelude::CosmosAddr;
use serde::{Deserialize, Serialize};
use utils::evm_client::EvmSigningClient;
use warpdrive_types::{
    contracts::cosmwasm::{
        service_handler::{ServiceHandlerExecuteMessages, ServiceHandlerQueryMessages},
        service_manager::{
            error::WavsValidateError, ServiceManagerQueryMessages, WarpDriveValidateResult,
        },
    },
    CosmosSubmitAction, EvmSubmitAction,
    IWarpDriveServiceHandler::IWarpDriveServiceHandlerInstance,
    IWarpDriveServiceManager::IWarpDriveServiceManagerInstance,
    Service, ServiceManager, ServiceManagerError, StellarSubmitAction, Submission, WavsSignable,
    WavsSignature, WavsSigner,
};

use crate::subsystems::aggregator::{error::AggregatorError, Aggregator};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum AnyTransactionReceipt {
    Evm(Box<TransactionReceipt>),
    // tx hash
    Cosmos(String),
    // Soroban tx hash (hex)
    Stellar(String),
}

impl AnyTransactionReceipt {
    pub fn tx_hash(&self) -> String {
        match self {
            AnyTransactionReceipt::Evm(receipt) => format!("{}", receipt.transaction_hash),
            AnyTransactionReceipt::Cosmos(tx_hash) => tx_hash.clone(),
            AnyTransactionReceipt::Stellar(tx_hash) => tx_hash.clone(),
        }
    }
}

impl Aggregator {
    pub async fn handle_action_submit_evm(
        &self,
        client: EvmSigningClient,
        queue: &[Submission],
        action: EvmSubmitAction,
    ) -> Result<AnyTransactionReceipt, AggregatorError> {
        // Bind the canonical (oldest) and latest submissions once at
        // the top. The dispatch closure always
        // `append_submission_to_queue`s before calling us, so the
        // queue is non-empty by construction; surfacing the typed
        // error rather than `unwrap`ing keeps a logic-bug refactor
        // from panicking the spawned task.
        let first = queue
            .first()
            .ok_or(AggregatorError::EmptySubmissionQueue { chain_kind: "EVM" })?;
        // SAFETY: queue.first() returned Some, so queue.last() is also Some.
        let last = queue
            .last()
            .expect("non-empty queue invariant (first() returned Some)");

        tracing::info!("Handling submit for {}", last.label());
        let contract_address = action.address.into();

        let service_manager = self
            .evm_get_submission_service_manager(client.provider.clone(), contract_address)
            .await?;

        // TODO - query to see if we should submit at all (e.g. has it already been submitted?)

        // Use the reference_block pinned at receive time (see
        // validate.rs). All packets in this queue were validated
        // against this block, so the on-chain `validate()` call will
        // see the same operator-set membership we did. If the pin is
        // missing (shouldn't happen — receive validation pins on the
        // first valid packet), fall back to current-block-minus-one
        // and warn.
        let block_height_minus_one = match self.get_pinned_reference_block(&first.event_id) {
            Some(b) => b,
            None => {
                let current = service_manager
                    .provider()
                    .get_block_number()
                    .await
                    .map_err(|e| AggregatorError::BlockNumber(e.into()))?;
                tracing::warn!(
                    "Aggregator (EVM): no pinned reference_block for submission {}; falling back to current block {}-1. Receive-time validation should have pinned it — investigate.",
                    first.label(),
                    current,
                );
                current.saturating_sub(1)
            }
        };

        let signatures: Vec<WavsSignature> = queue
            .iter()
            .map(|queued| queued.envelope_signature.clone())
            .collect();

        let signature_data = first
            .envelope
            .signature_data(signatures, block_height_minus_one)?;

        let result = service_manager
            .validate(first.envelope.clone().into(), signature_data.clone().into())
            .call()
            .await;

        if let Err(err) = result {
            match err.as_decoded_interface_error::<ServiceManagerError>() {
                Some(err) => match err {
                    ServiceManagerError::InsufficientQuorum(info) => {
                        return Err(AggregatorError::InsufficientQuorum {
                            signer_weight: info.signerWeight.to_string(),
                            threshold_weight: info.thresholdWeight.to_string(),
                            total_weight: info.totalWeight.to_string(),
                        });
                    }
                    err => {
                        return Err(AggregatorError::EvmServiceManagerValidateKnown(err));
                    }
                },
                None => match err.as_revert_data() {
                    Some(raw) => {
                        let raw_str = raw.to_string();
                        // Detect SignerNotRegistered() error (selector 0x3dda1739)
                        //
                        // This is a transient error indicating vectors haven't completed registration
                        // on-chain yet. Common scenarios:
                        // - P2P catch-up delivers submissions before vector registration completes
                        // - PoA middleware: Sequential docker exec calls for vector registration are slow
                        // - EigenLayer middleware: Batch registration is fast but still has a small timing window
                        //
                        // The aggregator's retry mechanism will:
                        // 1. Save the queue when this error is detected
                        // 2. Retry submission when next submission arrives
                        // 3. Succeed once vectors are registered on-chain
                        if raw_str == "0x3dda1739" {
                            // 0x3dda1739 is the Solidity selector for
                            // `SignerNotRegistered()` on the service
                            // manager. Surface it as the typed variant
                            // so the dispatch loop's transient-retry
                            // matcher catches it via `matches!` rather
                            // than substring search.
                            tracing::warn!(
                                "Signer not registered yet for submission {}. Queue will be saved for retry.",
                                last.label()
                            );
                            return Err(AggregatorError::SignerNotRegistered(format!(
                                "evm {raw_str}"
                            )));
                        }
                        return Err(AggregatorError::EvmServiceManagerValidateAnyRevert(raw_str));
                    }
                    None => {
                        return Err(AggregatorError::EvmServiceManagerValidateUnknown(err));
                    }
                },
            }
        };

        let tx_receipt = client
            .send_envelope_signatures(
                first.envelope.clone(),
                signature_data,
                contract_address,
                None,
                action.gas_price,
            )
            .await?;

        Ok(AnyTransactionReceipt::Evm(Box::new(tx_receipt)))
    }

    pub async fn handle_action_submit_cosmos(
        &self,
        client: layer_climb::prelude::SigningClient,
        queue: &[Submission],
        action: CosmosSubmitAction,
    ) -> Result<AnyTransactionReceipt, AggregatorError> {
        // Same non-empty invariant as the EVM path — bind once, surface
        // a typed error if the invariant is ever broken.
        let first = queue.first().ok_or(AggregatorError::EmptySubmissionQueue {
            chain_kind: "Cosmos",
        })?;

        let service_manager_addr: CosmosAddr = client
            .querier
            .contract_smart(
                &action.address.clone().into(),
                &ServiceHandlerQueryMessages::WarpDriveServiceManager {},
            )
            .await
            .map_err(AggregatorError::CosmosClient)?;

        // Pinned reference_block from receive-time validation, with
        // current-1 fallback. See the EVM path above for the same
        // pattern + rationale.
        let block_height_minus_one = match self.get_pinned_reference_block(&first.event_id) {
            Some(b) => b,
            None => {
                let current = client
                    .querier
                    .block_height()
                    .await
                    .map_err(AggregatorError::BlockNumber)?;
                tracing::warn!(
                    "Aggregator (Cosmos): no pinned reference_block for submission {}; falling back to current block {}-1. Receive-time validation should have pinned it — investigate.",
                    first.label(),
                    current,
                );
                current.saturating_sub(1)
            }
        };

        let signatures: Vec<WavsSignature> = queue
            .iter()
            .map(|queued| queued.envelope_signature.clone())
            .collect();

        let signature_data = first
            .envelope
            .signature_data(signatures, block_height_minus_one)?;

        let result: WarpDriveValidateResult = client
            .querier
            .contract_smart(
                &service_manager_addr.into(),
                &ServiceManagerQueryMessages::WarpDriveValidate {
                    envelope: first.envelope.clone().into(),
                    signature_data: signature_data.clone().into(),
                },
            )
            .await
            .map_err(AggregatorError::CosmosClient)?;

        match result {
            WarpDriveValidateResult::Ok => {
                tracing::info!("Service manager validation passed for custom submit");
            }
            WarpDriveValidateResult::Err(err) => match err {
                WavsValidateError::InsufficientQuorum {
                    signer_weight,
                    threshold_weight,
                    total_weight,
                } => {
                    return Err(AggregatorError::InsufficientQuorum {
                        signer_weight: signer_weight.to_string(),
                        threshold_weight: threshold_weight.to_string(),
                        total_weight: total_weight.to_string(),
                    });
                }
                err => {
                    return Err(AggregatorError::CosmosServiceManagerValidate(err));
                }
            },
        }

        // if we reach here, validation passed and we should submit!

        let resp = client
            .contract_execute(
                &action.address.into(),
                &ServiceHandlerExecuteMessages::WarpDriveHandleSignedEnvelope {
                    envelope: first.envelope.clone().into(),
                    signature_data: signature_data.clone().into(),
                },
                vec![],
                None,
            )
            .await
            .map_err(AggregatorError::CosmosClient)?;

        tracing::info!("Custom submit transaction sent: {:?}", resp.txhash);

        Ok(AnyTransactionReceipt::Cosmos(resp.txhash))
    }

    async fn evm_get_submission_service_manager(
        &self,
        provider: DynProvider,
        service_handler_address: alloy_primitives::Address,
    ) -> Result<IWarpDriveServiceManagerInstance<DynProvider>, AggregatorError> {
        // we need to get the service manager from the perspective of the service handler
        // which may be different than the service manager where the vector is staked
        // e.g. in the case of vector sets that are mirrored across multiple chains
        let service_handler =
            IWarpDriveServiceHandlerInstance::new(service_handler_address, provider.clone());

        let service_manager_address = service_handler
            .getServiceManager()
            .call()
            .await
            .map_err(AggregatorError::EvmServiceManagerLookup)?;

        Ok(IWarpDriveServiceManagerInstance::new(
            service_manager_address,
            provider,
        ))
    }

    /// Submit a queue of signed envelopes to a Stellar mock_submit (or any
    /// `EthereumHandler`-shaped) contract via warpdrive-client.
    ///
    /// Recovers the compressed secp256k1 public key from each
    /// `WavsSignature` (operators sign with the same secp256k1 key
    /// they use everywhere — the chain just consumes a different
    /// representation), sorts by pubkey ascending (the verification
    /// contract requires sorted input), then calls `verify_eth` on
    /// the handler contract.
    ///
    /// `verify_eth` invokes a transaction simulation before signing &
    /// submitting. The handler delegates to the secp256k1 verification
    /// contract, which panics with a `VerifyError` code on bad
    /// signatures, unregistered signers, or insufficient weight —
    /// those surface in the simulation result with no fee paid.
    ///
    /// Under the receive-time-validation design (see `validate.rs`
    /// and issue #33), every queued packet has *already* been
    /// validated against the pinned `reference_block`, so:
    /// - `#301 InvalidSignature` and `#302 SignerNotRegistered`
    ///   should not fire here in normal operation. If they do, the
    ///   error flows through `map_verify_eth_error` like any other
    ///   chain error.
    /// - `#303 InsufficientWeight` is the expected non-success: it
    ///   simply means quorum hasn't been reached yet, the queue is
    ///   saved, and the next packet retries.
    pub async fn handle_action_submit_stellar(
        &self,
        signing_key: ed25519_dalek::SigningKey,
        service: &Service,
        queue: &[Submission],
        action: StellarSubmitAction,
    ) -> Result<AnyTransactionReceipt, AggregatorError> {
        // ── Resolve chain config (RPC URL / network passphrase).
        let chain_configs = self.config.chains.read().unwrap().clone();
        let stellar_chain_config = chain_configs
            .get_chain(&action.chain)
            .and_then(|c| match c {
                warpdrive_types::AnyChainConfig::Stellar(cfg) => Some(cfg),
                _ => None,
            })
            .ok_or_else(|| {
                AggregatorError::Stellar(format!("no Stellar chain config for {}", action.chain))
            })?;

        if !matches!(&service.manager, ServiceManager::Stellar { .. }) {
            return Err(AggregatorError::Stellar(format!(
                "stellar submit on non-stellar service manager: {:?}",
                service.manager
            )));
        }

        // ── Build envelope bytes that the verification contract will
        // re-hash internally to verify each signature.
        //
        // Same non-empty invariant as the EVM/Cosmos paths — see the
        // `EmptySubmissionQueue` variant's doc comment.
        let first = queue.first().ok_or(AggregatorError::EmptySubmissionQueue {
            chain_kind: "Stellar",
        })?;
        let envelope = first.envelope.clone();
        let envelope_bytes = envelope.encode_data().map_err(|e| {
            AggregatorError::Stellar(format!("failed to abi-encode stellar envelope: {e:?}"))
        })?;

        // ── Recover compressed pubkey + sig per queue entry. The
        // shared `WavsSignature::secp256k1_compressed_pubkey` helper
        // does the EIP-191 prehash + recovery; same primitive used by
        // the receive-time validator in `validate.rs`.
        let mut signers_and_sigs: Vec<([u8; 33], [u8; 65])> = Vec::with_capacity(queue.len());
        for queued in queue {
            let sig_bytes: &[u8] = &queued.envelope_signature.data;
            if sig_bytes.len() != 65 {
                return Err(AggregatorError::Stellar(format!(
                    "stellar submit: expected 65-byte secp256k1 signature, got {}",
                    sig_bytes.len()
                )));
            }
            let pubkey = queued
                .envelope_signature
                .secp256k1_compressed_pubkey(&queued.envelope)
                .map_err(|e| {
                    AggregatorError::Stellar(format!(
                        "secp256k1 recovery failed for stellar submit: {e:?}"
                    ))
                })?;
            let mut sig_arr = [0u8; 65];
            sig_arr.copy_from_slice(sig_bytes);
            signers_and_sigs.push((pubkey, sig_arr));
        }

        // Verification contract expects signers in ascending pubkey order.
        signers_and_sigs.sort_by(|a, b| a.0.cmp(&b.0));
        let (signers, signatures): (Vec<[u8; 33]>, Vec<[u8; 65]>) =
            signers_and_sigs.into_iter().unzip();

        // ── Reference block: pinned at receive-time validation. All
        // queued packets were validated against this exact ledger
        // sequence, so the verification contract's per-signer weight
        // checks (also keyed off `reference_block`) will see the same
        // operator set we did. The fallback to current ledger
        // shouldn't normally fire — receive validation pins on the
        // first valid packet — but it keeps the submit path correct
        // if the pin is missing.
        let reference_block: u32 = match self.get_pinned_reference_block(&first.event_id) {
            Some(b) => b as u32,
            None => {
                let rpc = stellar_rpc_client::Client::new(&stellar_chain_config.rpc_url).map_err(
                    |e| {
                        AggregatorError::Stellar(format!(
                            "failed to build stellar rpc client: {e:?}"
                        ))
                    },
                )?;
                let current = rpc
                    .get_latest_ledger()
                    .await
                    .map_err(|e| AggregatorError::BlockNumber(anyhow::anyhow!("{e:?}")))?
                    .sequence;
                tracing::warn!(
                    chain = %action.chain,
                    "Stellar: no pinned reference_block for submission {}; falling back to current ledger {current}. Receive-time validation should have pinned it — investigate.",
                    first.label(),
                );

                current.saturating_sub(1)
            }
        };

        // ── Build the soroban env + source account for the handler call.
        let env = soroban_rs::Env::new(soroban_rs::EnvConfigs {
            rpc_url: stellar_chain_config.rpc_url.clone(),
            network_passphrase: stellar_chain_config.network_passphrase.clone(),
        })
        .map_err(|e| AggregatorError::Stellar(format!("soroban env: {e:?}")))?;
        let account = soroban_rs::Account::single(soroban_rs::Signer::new(signing_key));

        // ── Submit.
        //
        // `verify_eth` (via `warpdrive-client::utils::execute`)
        // performs a free read-only simulation against the Soroban
        // RPC node before signing or broadcasting:
        //   1. build tx
        //   2. simulate_transaction (free RPC call)
        //   3. if simulation reports an error → bail out, no
        //      signing, no broadcast, no fees paid
        //   4. otherwise → sign + broadcast (this consumes fees)
        //
        // Doomed submissions cost zero fees; only submissions that
        // would actually succeed make it to step 4. The expected
        // simulation failure here is `#303 InsufficientWeight`
        // (quorum not yet met), translated to
        // `AggregatorError::InsufficientQuorum` so the dispatch loop
        // saves the queue for retry.
        let contract_id = stellar_strkey::Contract(action.address);
        let handler_cfg = soroban_rs::ClientContractConfigs {
            contract_id,
            env,
            source_account: account,
        };
        let mut handler =
            warpdrive_client::ethereum_handler::EthereumHandlerClient::new(handler_cfg);
        let num_signers = signers.len();
        let sig_data = warpdrive_client::ethereum_handler::SignatureData {
            signers,
            signatures,
            reference_block,
        };

        tracing::info!(
            chain = %action.chain,
            handler = %contract_id,
            num_signers,
            reference_block,
            "Stellar: submitting via EthereumHandlerClient::verify_eth"
        );

        match handler.verify_eth(envelope_bytes, sig_data).await {
            Ok(resp) => Ok(AnyTransactionReceipt::Stellar(format!("{resp:?}"))),
            Err(err) => Err(map_verify_eth_error(err, num_signers)),
        }
    }
}

/// Translate a `verify_eth` failure into a typed `AggregatorError`.
///
/// Soroban surfaces contract errors from simulation as
/// `Error(Contract, #N)`. The verification contract uses the codes
/// defined in `warpdrive-shared::interfaces::verification::VerifyError`:
///   301 InvalidSignature      304 EmptySignatures   307 ZeroRequiredWeight
///   302 SignerNotRegistered   305 LengthMismatch
///   303 InsufficientWeight    306 SignersNotOrdered
///
/// Only #302 (transient — vectors not yet registered) and #303
/// (insufficient quorum — wait for more vectors) get distinct handling
/// in the dispatch loop today; the rest collapse into `Stellar(...)`.
fn map_verify_eth_error(
    err: soroban_rs::SorobanHelperError,
    num_signers: usize,
) -> AggregatorError {
    // `SorobanHelperError::TransactionSimulationFailed(s)` carries the
    // simulation error string straight from the Soroban RPC node. For
    // contract panics that string contains `Error(Contract, #N)` where
    // N is the contract-defined error code. Substring-match is the
    // pragmatic option here — soroban-rs doesn't expose a structured
    // contract-error type today, and the format is stable on the RPC
    // wire.
    let err_str = format!("{err:?}");
    if err_str.contains("Error(Contract, #302)") {
        // Multi-vector startup race: a signer in the queue hasn't
        // registered on-chain yet. The dispatch loop matches on this
        // variant directly to recognize it as transient and save the
        // queue for retry once the remaining vectors finish
        // registering.
        return AggregatorError::SignerNotRegistered(format!("stellar #302: {err_str}"));
    }
    if err_str.contains("Error(Contract, #303)") {
        // We have <required_weight signed; expected when fewer than
        // quorum vectors have signed yet. The verification contract
        // doesn't return numeric totals in the panic, and we
        // deliberately don't issue an extra `required_weight` query
        // (the whole point of the simulation-only path is to avoid
        // extra calls). The dispatch loop only matches the variant
        // shape to decide retry, so the placeholder strings are fine.
        return AggregatorError::InsufficientQuorum {
            signer_weight: "unknown".to_string(),
            threshold_weight: "unknown".to_string(),
            total_weight: format!("{num_signers} signature(s) sent"),
        };
    }
    AggregatorError::Stellar(format!("verify_eth failed: {err_str}"))
}

#[cfg(test)]
mod map_verify_eth_error_tests {
    use super::*;
    use soroban_rs::SorobanHelperError;

    // The simulation error string from soroban-rpc embeds the contract
    // panic as `Error(Contract, #N)`. We feed a representative wrapper
    // string here; the helper's contract is "match the substring on
    // the debug-formatted error", which is exactly what these inputs
    // exercise.
    fn sim_err(inner: &str) -> SorobanHelperError {
        SorobanHelperError::TransactionSimulationFailed(inner.to_string())
    }

    #[test]
    fn maps_302_to_signer_not_registered() {
        let err = sim_err("HostError: Error(Contract, #302) ...");
        match map_verify_eth_error(err, 1) {
            AggregatorError::SignerNotRegistered(s) => {
                assert!(s.contains("#302"), "detail should preserve raw error: {s}");
            }
            other => panic!("expected SignerNotRegistered, got {other:?}"),
        }
    }

    #[test]
    fn maps_303_to_insufficient_quorum() {
        let err = sim_err("HostError: Error(Contract, #303) ...");
        match map_verify_eth_error(err, 2) {
            AggregatorError::InsufficientQuorum { total_weight, .. } => {
                assert!(
                    total_weight.contains('2'),
                    "expected num_signers (2) preserved in total_weight, got {total_weight}"
                );
            }
            other => panic!("expected InsufficientQuorum, got {other:?}"),
        }
    }

    #[test]
    fn unknown_contract_code_falls_through_to_stellar() {
        let err = sim_err("HostError: Error(Contract, #999) ...");
        assert!(
            matches!(map_verify_eth_error(err, 0), AggregatorError::Stellar(_)),
            "unknown contract code should map to Stellar(_)",
        );
    }

    #[test]
    fn non_simulation_error_falls_through_to_stellar() {
        // Anything that's not the simulation-failure shape (e.g.
        // network errors, encoding errors) also collapses to Stellar.
        let err = SorobanHelperError::NotSupported("placeholder".to_string());
        assert!(
            matches!(map_verify_eth_error(err, 0), AggregatorError::Stellar(_)),
            "non-simulation error should map to Stellar(_)",
        );
    }

    #[test]
    fn matches_302_anywhere_in_string() {
        // The match is a substring scan, so the marker can appear
        // surrounded by other diagnostic noise from soroban-rpc.
        let err = sim_err(
            "transaction simulation failed: HostError: Error(Contract, #302) trace [foo bar baz]",
        );
        assert!(matches!(
            map_verify_eth_error(err, 1),
            AggregatorError::SignerNotRegistered(_)
        ));
    }
}
