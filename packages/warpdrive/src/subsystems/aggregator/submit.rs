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
                            tracing::warn!(
                                "Signer not registered yet for submission {}. Queue will be saved for retry.",
                                queue.last().unwrap().label()
                            );
                            return Err(AggregatorError::EvmServiceManagerValidateAnyRevert(
                                format!("SignerNotRegistered ({})", raw_str),
                            ));
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
    /// Walks the queue, recovers the compressed secp256k1 public key from
    /// each WavsSignature (operators sign with the same secp256k1 key
    /// they use everywhere — the chain just consumes a different
    /// representation), sorts by pubkey ascending (the verification
    /// contract requires sorted input), runs the same per-signature
    /// `check_one` + `required_weight` pre-flight as the EVM path does
    /// against `IWavsServiceManager.validate`, then calls `verify_eth`
    /// on the destination handler contract.
    ///
    /// Pre-flight failures are translated into typed `AggregatorError`
    /// variants so the dispatch loop's queue-save-and-retry machinery
    /// kicks in:
    /// - **Insufficient summed weight** → `InsufficientQuorum` (queue is
    ///   saved, retried when the next vector signs).
    /// - **Any signer not registered yet** → error stringified as
    ///   `SignerNotRegistered` so the substring detection in
    ///   `aggregator.rs::handle_submit_action` recognizes it as
    ///   transient (common during multi-vector registration races).
    pub async fn handle_action_submit_stellar(
        &self,
        signing_key: ed25519_dalek::SigningKey,
        service: &Service,
        queue: &[Submission],
        action: StellarSubmitAction,
    ) -> Result<AnyTransactionReceipt, AggregatorError> {
        use k256::ecdsa::{RecoveryId, Signature as K256Sig, VerifyingKey};
        use warpdrive_client::project_root::{ProjectRootClient, VerificationType};
        use warpdrive_client::secp256k1_verification::Secp256k1VerificationClient;

        // ── Resolve chain config + project_root from the service manager.
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

        let project_root = match &service.manager {
            ServiceManager::Stellar { address, .. } => *address,
            other => {
                return Err(AggregatorError::Stellar(format!(
                    "stellar submit on non-stellar service manager: {other:?}"
                )));
            }
        };

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
        signers_and_sigs.sort_by(|a, b| a.0.cmp(&b.0));
        let (signers, signatures): (Vec<[u8; 33]>, Vec<[u8; 65]>) =
            signers_and_sigs.into_iter().unzip();

        // ── Reference block = current ledger sequence at submit time.
        let rpc = stellar_rpc_client::Client::new(&stellar_chain_config.rpc_url).map_err(|e| {
            AggregatorError::Stellar(format!("failed to build stellar rpc client: {e:?}"))
        })?;
        let reference_block: u32 = rpc
            .get_latest_ledger()
            .await
            .map_err(|e| AggregatorError::BlockNumber(anyhow::anyhow!("{e:?}")))?
            .sequence;

        // ── Build a soroban env once; the verification + handler clients
        // share it. The signing key here only matters for write txs; for
        // the verification queries below it just supplies a source account
        // (the simulation never validates the signature).
        let env = soroban_rs::Env::new(soroban_rs::EnvConfigs {
            rpc_url: stellar_chain_config.rpc_url.clone(),
            network_passphrase: stellar_chain_config.network_passphrase.clone(),
        })
        .map_err(|e| AggregatorError::Stellar(format!("soroban env: {e:?}")))?;
        let account = soroban_rs::Account::single(soroban_rs::Signer::new(signing_key.clone()));

        // ── Pre-flight validation: walk to verification_contract via
        // project_root (matching the design spec), then check_one each
        // signature and tally weights against required_weight.
        let project_root_cfg = soroban_rs::ClientContractConfigs {
            contract_id: project_root,
            env: env.clone(),
            source_account: account.clone(),
        };
        let project_root_client = ProjectRootClient::new(project_root_cfg);
        match project_root_client.verification_type().await.map_err(|e| {
            AggregatorError::Stellar(format!(
                "ProjectRoot::verification_type query failed: {e:?}"
            ))
        })? {
            VerificationType::Ethereum => {}
            VerificationType::Stellar => {
                return Err(AggregatorError::Stellar(
                    "VerificationType::Stellar (ed25519/SEP-0053) is not supported yet — \
                     operators sign with secp256k1 today."
                        .to_string(),
                ));
            }
        }
        let verification_contract =
            project_root_client
                .verification_contract()
                .await
                .map_err(|e| {
                    AggregatorError::Stellar(format!(
                        "ProjectRoot::verification_contract query failed: {e:?}"
                    ))
                })?;

        let verification_cfg = soroban_rs::ClientContractConfigs {
            contract_id: verification_contract,
            env: env.clone(),
            source_account: account.clone(),
        };
        let verification_client = Secp256k1VerificationClient::new(verification_cfg);

        let required_weight = verification_client.required_weight().await.map_err(|e| {
            AggregatorError::Stellar(format!(
                "Secp256k1VerificationClient::required_weight query failed: {e:?}"
            ))
        })?;

        let mut total_weight: u64 = 0;
        for (signer_pubkey, sig) in signers.iter().zip(signatures.iter()) {
            match verification_client
                .check_one(
                    envelope_bytes.clone(),
                    *sig,
                    *signer_pubkey,
                    Some(reference_block),
                )
                .await
            {
                Ok(weight) => {
                    total_weight = total_weight.saturating_add(weight);
                }
                Err(err) => {
                    let err_str = format!("{err:?}");
                    // Translate the verification-contract error name into
                    // the substring `aggregator.rs::handle_submit_action`
                    // matches on for transient-retry behavior. A signer
                    // that hasn't registered yet is the canonical
                    // multi-vector-startup race.
                    if err_str.contains("SignerNotRegistered") {
                        return Err(AggregatorError::Stellar(format!(
                            "SignerNotRegistered (transient): {err_str}"
                        )));
                    }
                    return Err(AggregatorError::Stellar(format!(
                        "check_one for signer 0x{} failed: {err_str}",
                        const_hex::encode(signer_pubkey)
                    )));
                }
            }
        }

        if total_weight < required_weight {
            return Err(AggregatorError::InsufficientQuorum {
                signer_weight: total_weight.to_string(),
                threshold_weight: required_weight.to_string(),
                total_weight: total_weight.to_string(),
            });
        }

        // ── Pre-flight passed: send the actual handler call.
        let contract_id = stellar_strkey::Contract(action.address);
        let handler_cfg = soroban_rs::ClientContractConfigs {
            contract_id,
            env,
            source_account: account,
        };
        let mut handler =
            warpdrive_client::ethereum_handler::EthereumHandlerClient::new(handler_cfg);
        let sig_data = warpdrive_client::ethereum_handler::SignatureData {
            signers,
            signatures,
            reference_block,
        };

        tracing::info!(
            chain = %action.chain,
            handler = %contract_id,
            project_root = %project_root,
            num_signers = sig_data.signers.len(),
            total_weight,
            required_weight,
            reference_block,
            "Stellar pre-flight passed; submitting via EthereumHandlerClient::verify_eth"
        );

        let resp = handler
            .verify_eth(envelope_bytes, sig_data)
            .await
            .map_err(|e| AggregatorError::Stellar(format!("verify_eth failed: {e:?}")))?;

        Ok(AnyTransactionReceipt::Stellar(format!("{resp:?}")))
    }
}
