use soroban_sdk::{Address, Bytes, BytesN, Env, String, contract, contractimpl, xdr::FromXdr};

use warpdrive_shared::interfaces::{
    handler::{Ed25519SignatureData, HandlerError, StellarHandlerInterface, Verified, XlmEnvelope},
    verification::{Ed25519VerificationClient, VerifyError},
    warpdrive::{ContractUpgraded, WarpDriveInterface},
};

use crate::storage;

/// Maximum age (in ledgers) allowed for a reference block.
/// Note: 200 blocks is around 20 minutes with 5-6 second blocks
const MAX_REFERENCE_BLOCK_AGE: u32 = 200;

/// Validates that `reference_block` is strictly in the past and within the allowed age window.
fn validate_reference_block(env: &Env, reference_block: u32) -> Result<(), HandlerError> {
    let current = env.ledger().sequence();
    if reference_block >= current {
        return Err(HandlerError::InvalidReferenceBlock);
    }
    if current - reference_block > MAX_REFERENCE_BLOCK_AGE {
        return Err(HandlerError::InvalidReferenceBlock);
    }
    Ok(())
}

/// Maps a `try_verify` result from the verification contract into a `HandlerError`.
fn map_verify_result(
    res: Result<
        Result<(), soroban_sdk::ConversionError>,
        Result<VerifyError, soroban_sdk::InvokeError>,
    >,
) -> Result<(), HandlerError> {
    match res {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_conversion)) => Err(HandlerError::UnknownVerificationError),
        Err(Ok(e)) => Err(HandlerError::from(e)),
        Err(Err(_invoke_err)) => Err(HandlerError::OtherInvocationError),
    }
}

#[contract]
pub struct StellarHandler;

#[contractimpl]
impl StellarHandler {
    pub fn __constructor(env: Env, admin: Address, verification_contract: Address) {
        storage::set_admin(&env, &admin);
        storage::set_version(&env, &String::from_str(&env, env!("CARGO_PKG_VERSION")));
        storage::set_verification_contract(&env, &verification_contract);
        storage::extend_instance_ttl(&env);
    }
}

#[contractimpl]
impl WarpDriveInterface for StellarHandler {
    fn upgrade(env: Env, new_wasm_hash: BytesN<32>, new_version: String) {
        let admin = storage::get_admin(&env);
        admin.require_auth();

        storage::set_version(&env, &new_version);
        storage::extend_instance_ttl(&env);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        ContractUpgraded::new(new_version).publish(&env);
    }

    fn admin(env: Env) -> Address {
        storage::get_admin(&env)
    }

    fn pending_admin(env: Env) -> Option<Address> {
        warpdrive_shared::admin::pending(&env)
    }

    fn propose_admin(env: Env, new_admin: Address) {
        warpdrive_shared::admin::propose(&env, &storage::get_admin(&env), new_admin);
    }

    fn accept_admin(env: Env) {
        let new_admin = warpdrive_shared::admin::accept(&env);
        storage::set_admin(&env, &new_admin);
    }

    fn version(env: Env) -> String {
        storage::get_version(&env)
    }
}

#[contractimpl]
impl StellarHandlerInterface for StellarHandler {
    fn verification_contract(env: Env) -> Address {
        storage::get_verification_contract(&env)
    }

    fn payload(env: Env, event_id: BytesN<20>) -> Option<Bytes> {
        storage::get_payload(&env, event_id)
    }

    /// Verifies the packet, assuming the envelope is XDR-encoded (Soroban native format).
    ///
    /// No caller authorization is required — this is intentional. Security is enforced entirely
    /// through cryptographic signature verification of the envelope contents.
    fn verify_xlm(
        env: Env,
        envelope_bytes: Bytes,
        sig_data: Ed25519SignatureData,
    ) -> Result<(), HandlerError> {
        storage::extend_instance_ttl(&env);
        validate_reference_block(&env, sig_data.reference_block)?;

        // Parse XDR bytes into the typed envelope
        let envelope = XlmEnvelope::from_xdr(&env, &envelope_bytes)
            .map_err(|_| HandlerError::InvalidEnvelope)?;
        let event_id = envelope.event_id;

        // Check for duplicate event
        if storage::is_event_seen(&env, &event_id) {
            return Err(HandlerError::EventAlreadySeen);
        }

        // Verify signatures via the verification contract
        let verification_addr = storage::get_verification_contract(&env);
        let verification = Ed25519VerificationClient::new(&env, &verification_addr);
        let res = verification.try_verify(
            &envelope_bytes,
            &sig_data.signatures,
            &sig_data.signers,
            &sig_data.reference_block,
        );
        map_verify_result(res)?;

        // Mark event as seen
        storage::mark_event_seen(&env, &event_id);

        // Save payload
        storage::save_payload(&env, event_id.clone(), envelope.payload);

        Verified::new(event_id).publish(&env);

        Ok(())
    }
}
