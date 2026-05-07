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
        tracing::info!("Handling submit for {}", queue.last().unwrap().label());
        let contract_address = action.address.into();

        let service_manager = self
            .evm_get_submission_service_manager(client.provider.clone(), contract_address)
            .await?;

        // TODO - query to see if we should submit at all (e.g. has it already been submitted?)

        let block_height_minus_one = service_manager
            .provider()
            .get_block_number()
            .await
            .map_err(|e| AggregatorError::BlockNumber(e.into()))?
            - 1;

        let signatures: Vec<WavsSignature> = queue
            .iter()
            .map(|queued| queued.envelope_signature.clone())
            .collect();

        // safe - we pushed the latest submission into the (temporary) queue
        let signature_data = queue
            .first()
            .unwrap()
            .envelope
            .signature_data(signatures, block_height_minus_one)?;

        let result = service_manager
            .validate(
                queue.first().unwrap().envelope.clone().into(),
                signature_data.clone().into(),
            )
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
                                queue.last().unwrap().label()
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
                queue.first().unwrap().envelope.clone(),
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
        let service_manager_addr: CosmosAddr = client
            .querier
            .contract_smart(
                &action.address.clone().into(),
                &ServiceHandlerQueryMessages::WarpDriveServiceManager {},
            )
            .await
            .map_err(AggregatorError::CosmosClient)?;

        let block_height_minus_one = client
            .querier
            .block_height()
            .await
            .map_err(AggregatorError::BlockNumber)?
            - 1;

        let signatures: Vec<WavsSignature> = queue
            .iter()
            .map(|queued| queued.envelope_signature.clone())
            .collect();

        // safe - we pushed the latest submission into the (temporary) queue
        let signature_data = queue
            .first()
            .unwrap()
            .envelope
            .signature_data(signatures, block_height_minus_one)?;

        let result: WarpDriveValidateResult = client
            .querier
            .contract_smart(
                &service_manager_addr.into(),
                &ServiceManagerQueryMessages::WarpDriveValidate {
                    envelope: queue.first().unwrap().envelope.clone().into(),
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
                    envelope: queue.first().unwrap().envelope.clone().into(),
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
    /// `WavsSignature` (operators sign with the same secp256k1 key they
    /// use everywhere — the chain just consumes a different
    /// representation), sorts by pubkey ascending (the verification
    /// contract requires sorted input), then calls `verify_eth` on the
    /// handler contract.
    ///
    /// `verify_eth` invokes a transaction simulation before signing &
    /// submitting. The handler delegates to the secp256k1 verification
    /// contract, which panics with a `VerifyError` code on bad
    /// signatures, unregistered signers, or insufficient weight — those
    /// surface in the simulation result with no fee paid. We translate
    /// the relevant codes to typed `AggregatorError` variants:
    ///   - `#302 SignerNotRegistered` → `SignerNotRegistered(...)` —
    ///     dispatch loop recognizes this variant as transient and
    ///     retries when the next vector signs.
    ///   - `#303 InsufficientWeight` → `InsufficientQuorum` — queue
    ///     saved, retried when the next vector signs.
    ///   - other codes → `Stellar(...)`.
    pub async fn handle_action_submit_stellar(
        &self,
        signing_key: ed25519_dalek::SigningKey,
        service: &Service,
        queue: &[Submission],
        action: StellarSubmitAction,
    ) -> Result<AnyTransactionReceipt, AggregatorError> {
        use k256::ecdsa::{RecoveryId, Signature as K256Sig, VerifyingKey};

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

        // ── Build envelope bytes + the EIP-191 prehash that operators signed.
        let envelope = queue
            .first()
            .ok_or_else(|| {
                AggregatorError::Stellar("empty submission queue for stellar submit".to_string())
            })?
            .envelope
            .clone();
        let envelope_bytes = envelope.encode_data().map_err(|e| {
            AggregatorError::Stellar(format!("failed to abi-encode stellar envelope: {e:?}"))
        })?;
        let prefixed_hash: alloy_primitives::FixedBytes<32> =
            envelope.prefix_eip191_hash().map_err(|e| {
                AggregatorError::Stellar(format!(
                    "failed to compute eip-191 hash for stellar envelope: {e:?}"
                ))
            })?;

        // ── Recover (compressed_pubkey, signature) per queue entry.
        let mut signers_and_sigs: Vec<([u8; 33], [u8; 65])> = Vec::with_capacity(queue.len());
        for queued in queue {
            let sig_bytes: &[u8] = &queued.envelope_signature.data;
            if sig_bytes.len() != 65 {
                return Err(AggregatorError::Stellar(format!(
                    "stellar submit: expected 65-byte secp256k1 signature, got {}",
                    sig_bytes.len()
                )));
            }
            let r_s: [u8; 64] = sig_bytes[..64]
                .try_into()
                .expect("65-byte slice gives 64-byte head");
            let v = sig_bytes[64];
            let k_sig = K256Sig::from_slice(&r_s).map_err(|e| {
                AggregatorError::Stellar(format!("invalid secp256k1 signature: {e:?}"))
            })?;
            // Operator signatures use Ethereum-style v (27/28); reduce to
            // the 0/1 recovery id k256 expects.
            let recid_byte = if v >= 27 { v - 27 } else { v };
            let recid = RecoveryId::try_from(recid_byte).map_err(|e| {
                AggregatorError::Stellar(format!("invalid recovery id {recid_byte}: {e:?}"))
            })?;
            let vk = VerifyingKey::recover_from_prehash(prefixed_hash.as_slice(), &k_sig, recid)
                .map_err(|e| {
                    AggregatorError::Stellar(format!(
                        "secp256k1 recovery failed for stellar submit: {e:?}"
                    ))
                })?;
            let pubkey_bytes = vk.to_sec1_bytes();
            let compressed: [u8; 33] = pubkey_bytes.as_ref().try_into().map_err(|_| {
                AggregatorError::Stellar(format!(
                    "expected 33-byte compressed pubkey, got {}",
                    pubkey_bytes.len()
                ))
            })?;
            let mut sig_arr = [0u8; 65];
            sig_arr.copy_from_slice(sig_bytes);
            signers_and_sigs.push((compressed, sig_arr));
        }

        // Verification contract expects signers in ascending pubkey order.
        signers_and_sigs.sort_by_key(|a| a.0);
        let (signers, signatures): (Vec<[u8; 33]>, Vec<[u8; 65]>) =
            signers_and_sigs.into_iter().unzip();

        // ── Reference block = current ledger sequence at submit time.
        // The verification contract checks each signer's weight against
        // the operator set _as of this block_, so passing "now" means we
        // re-validate against the latest set on every retry. Operators
        // joining or having their weight changed between sig generation
        // and submission is handled correctly without any cache.
        let rpc = stellar_rpc_client::Client::new(&stellar_chain_config.rpc_url).map_err(|e| {
            AggregatorError::Stellar(format!("failed to build stellar rpc client: {e:?}"))
        })?;
        let reference_block: u32 = rpc
            .get_latest_ledger()
            .await
            .map_err(|e| AggregatorError::BlockNumber(anyhow::anyhow!("{e:?}")))?
            .sequence;

        // ── Build the soroban env + source account for the handler call.
        let env = soroban_rs::Env::new(soroban_rs::EnvConfigs {
            rpc_url: stellar_chain_config.rpc_url.clone(),
            network_passphrase: stellar_chain_config.network_passphrase.clone(),
        })
        .map_err(|e| AggregatorError::Stellar(format!("soroban env: {e:?}")))?;
        let account = soroban_rs::Account::single(soroban_rs::Signer::new(signing_key));

        // ── Submit.
        //
        // `verify_eth` (via `warpdrive-client::utils::execute`) performs
        // a free read-only simulation against the Soroban RPC node
        // _before_ signing or broadcasting:
        //   1. build tx
        //   2. simulate_transaction (free RPC call)
        //   3. if simulation reports an error → bail out, no signing,
        //      no broadcast, no fees paid
        //   4. otherwise → sign + broadcast (this consumes fees)
        //
        // The handler delegates signature validation to the verification
        // contract on-chain. Bad signatures, unregistered signers, and
        // insufficient quorum all panic inside that delegate, which the
        // simulation sees as `Error(Contract, #N)` — caught at step 3,
        // returned to us as `SorobanHelperError::TransactionSimulationFailed`,
        // and translated below into a typed `AggregatorError`.
        //
        // Net effect: doomed submissions cost zero fees; only
        // submissions that would actually succeed make it to step 4.
        let contract_id = stellar_strkey::Contract(action.address);
        let handler_cfg = soroban_rs::ClientContractConfigs {
            contract_id,
            env: env.clone(),
            source_account: account.clone(),
        };
        let mut handler =
            warpdrive_client::ethereum_handler::EthereumHandlerClient::new(handler_cfg);
        let num_signers = signers.len();
        let sig_data = warpdrive_client::ethereum_handler::SignatureData {
            signers: signers.clone(),
            signatures: signatures.clone(),
            reference_block,
        };

        tracing::info!(
            chain = %action.chain,
            handler = %contract_id,
            num_signers,
            reference_block,
            "Stellar: submitting via EthereumHandlerClient::verify_eth"
        );

        // ── First attempt: try verify_eth with all queued sigs.
        //
        // The happy path is just this match. The cleanup path below is
        // only reached when simulation reports #302
        // (SignerNotRegistered) — i.e. at least one signer in the
        // queue isn't in the operator set. The simulation tells us
        // *that* it failed but not *which* signer, so we have to do
        // the per-signer query work ourselves.
        match handler.verify_eth(envelope_bytes.clone(), sig_data).await {
            Ok(resp) => return Ok(AnyTransactionReceipt::Stellar(format!("{resp:?}"))),
            Err(err) => {
                let err_str = format!("{err:?}");
                if !err_str.contains("Error(Contract, #302)") {
                    // Any other failure (insufficient quorum, bad sig,
                    // network error, etc.) gets the standard mapping.
                    return Err(map_verify_eth_error(err, num_signers));
                }
                // Fall through to per-signer cleanup. The queue
                // currently contains at least one sig that will never
                // validate (operator-set joins are not retroactive: a
                // sig generated before the signer joined the set is
                // permanently invalid). If we don't filter it out,
                // every future submit attempt re-includes it and #302
                // fires again — quorum can never be reached, the
                // queue blocks forever. Classic DoS shape: a single
                // peer that signs from outside the set permanently
                // wedges this submission's queue.
                //
                // Cleanup strategy (Approach A — filter locally,
                // don't mutate persisted queue):
                //   1. Identify the unregistered signers.
                //   2. Retry verify_eth with only the registered
                //      subset.
                //   3. Persisted queue stays as-is. The dispatch loop
                //      saves the original queue on error (and burns
                //      it on success). Bad sigs stay in storage but
                //      are filtered out of every future attempt, so
                //      they no longer block quorum. Trade-off: O(N)
                //      bytes of wasted storage per stuck queue, no
                //      plumbing through the dispatch loop's closure.
                tracing::warn!(
                    chain = %action.chain,
                    "Stellar: verify_eth reported SignerNotRegistered (#302); attempting per-signer cleanup"
                );
            }
        }

        // ── Per-signer cleanup path (only reached on #302).
        //
        // We need to find which signer(s) are unregistered. The
        // verification contract exposes `signer_weight(pubkey)` which
        // returns the signer's current weight, or 0 if they're not in
        // the set. weight == 0 ⇔ unregistered.
        //
        // Note: `signer_weight` reads the *current* operator set; it
        // does NOT take a reference_block. The first verify_eth call
        // checked against `reference_block` (which we set to "now" at
        // submit time). In normal operation these are within seconds
        // of each other and agree. The two corner cases are handled
        // explicitly below (no-drops case and all-drops case).
        let verification_contract = handler.verification_contract().await.map_err(|e| {
            AggregatorError::Stellar(format!(
                "verification_contract lookup during #302 cleanup failed: {e:?}"
            ))
        })?;
        let verification_cfg = soroban_rs::ClientContractConfigs {
            contract_id: verification_contract,
            env: env.clone(),
            source_account: account.clone(),
        };
        let verification_client =
            warpdrive_client::secp256k1_verification::Secp256k1VerificationClient::new(
                verification_cfg,
            );

        let mut weights: Vec<u64> = Vec::with_capacity(signers.len());
        for pubkey in &signers {
            let w = verification_client
                .signer_weight(*pubkey)
                .await
                .map_err(|e| {
                    AggregatorError::Stellar(format!(
                        "signer_weight query during #302 cleanup failed for 0x{}: {e:?}",
                        const_hex::encode(pubkey)
                    ))
                })?;
            weights.push(w);
        }

        // ── Decide who to keep, who to drop.
        //
        // partition_by_registration is a pure helper (and unit-tested
        // separately): given `weights`, returns two index lists in
        // input order. We use indices rather than filtering the
        // signer/signature vectors directly so the parallel arrays
        // stay aligned without an extra zip+sort.
        let (keep_idx, drop_idx) = partition_by_registration(&weights);
        for idx in &drop_idx {
            tracing::warn!(
                chain = %action.chain,
                pubkey = %format!("0x{}", const_hex::encode(signers[*idx])),
                "Stellar: dropping signature from unregistered signer (will be filtered on every future submit attempt until queue is burned)"
            );
        }
        if keep_idx.is_empty() {
            // Every signer in the queue is unregistered. Nothing we
            // can submit. Surface as SignerNotRegistered so the
            // dispatch loop saves and retries — by the next attempt
            // either some of these have registered, or new sigs from
            // valid signers have been appended.
            return Err(AggregatorError::SignerNotRegistered(format!(
                "stellar #302 cleanup: all {num_signers} queued signers unregistered"
            )));
        }
        if drop_idx.is_empty() {
            // verify_eth said #302 but `signer_weight` reports every
            // signer is registered. Possible explanation: a signer
            // was registered between the simulation (which checked
            // weights at our `reference_block`) and our cleanup
            // queries (which read "current" weight). They were at 0
            // when simulation ran, are at >0 now.
            //
            // The race resolves itself: dispatch will save the queue
            // and the next submission attempt picks a fresh
            // reference_block (later than the registration), so the
            // simulation will see them as registered. No retry from
            // here — let the dispatch loop's normal save+retry path
            // handle it.
            return Err(AggregatorError::SignerNotRegistered(format!(
                "stellar #302 but all signers report registered (likely registration race between simulation and cleanup); {num_signers} sigs"
            )));
        }

        // Slice the original parallel arrays by the keep indices.
        // Order is preserved (partition_by_registration emits indices
        // in input order), so the sorted-pubkey invariant required by
        // the verification contract still holds for the survivors.
        let kept_signers: Vec<[u8; 33]> = keep_idx.iter().map(|i| signers[*i]).collect();
        let kept_signatures: Vec<[u8; 65]> = keep_idx.iter().map(|i| signatures[*i]).collect();
        let kept_count = kept_signers.len();
        let kept_sig_data = warpdrive_client::ethereum_handler::SignatureData {
            signers: kept_signers,
            signatures: kept_signatures,
            reference_block,
        };

        tracing::info!(
            chain = %action.chain,
            kept = kept_count,
            dropped = drop_idx.len(),
            "Stellar: retrying verify_eth with registered-only subset"
        );

        match handler.verify_eth(envelope_bytes, kept_sig_data).await {
            Ok(resp) => Ok(AnyTransactionReceipt::Stellar(format!("{resp:?}"))),
            Err(err) => Err(map_verify_eth_error(err, kept_count)),
        }
    }
}

/// Pure helper: split signer indices by whether they're registered
/// (weight > 0) or not. Used by the Stellar #302 cleanup path —
/// caller takes the parallel `signers`/`signatures` arrays, queries
/// `signer_weight` per pubkey, then feeds the resulting weights
/// here to find out which entries to keep and which to drop.
///
/// Kept separate from the async submit code so it's trivially
/// unit-testable.
///
/// Both output vectors list indices in *input order*. This matters:
/// the verification contract requires `signers` to be sorted by
/// pubkey ascending, and the input arrays already are; preserving
/// order keeps that invariant intact for the kept subset.
fn partition_by_registration(weights: &[u64]) -> (Vec<usize>, Vec<usize>) {
    let mut keep = Vec::new();
    let mut drop = Vec::new();
    for (i, w) in weights.iter().enumerate() {
        if *w > 0 {
            keep.push(i);
        } else {
            drop.push(i);
        }
    }
    (keep, drop)
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

#[cfg(test)]
mod partition_by_registration_tests {
    use super::partition_by_registration;

    #[test]
    fn empty_input_yields_empty_partitions() {
        let (keep, drop) = partition_by_registration(&[]);
        assert!(keep.is_empty());
        assert!(drop.is_empty());
    }

    #[test]
    fn all_registered_keeps_all() {
        let (keep, drop) = partition_by_registration(&[1, 100, 5000]);
        assert_eq!(keep, vec![0, 1, 2]);
        assert!(drop.is_empty());
    }

    #[test]
    fn all_unregistered_drops_all() {
        let (keep, drop) = partition_by_registration(&[0, 0, 0]);
        assert!(keep.is_empty());
        assert_eq!(drop, vec![0, 1, 2]);
    }

    #[test]
    fn mixed_preserves_input_order_in_both_partitions() {
        // weight=0 signers are unregistered; the rest are kept.
        // Both output vectors must list indices in input order so
        // callers can use them to slice the original `signers`/
        // `signatures` arrays without re-sorting.
        let (keep, drop) = partition_by_registration(&[10, 0, 20, 0, 30]);
        assert_eq!(keep, vec![0, 2, 4]);
        assert_eq!(drop, vec![1, 3]);
    }

    #[test]
    fn weight_one_is_registered() {
        // Boundary: weight==1 is a registered signer. Only weight==0
        // means "not in the set".
        let (keep, drop) = partition_by_registration(&[1]);
        assert_eq!(keep, vec![0]);
        assert!(drop.is_empty());
    }
}
