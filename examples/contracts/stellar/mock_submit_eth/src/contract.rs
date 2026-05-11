use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, Address, Bytes, Env, Symbol,
};

use crate::envelope::{DataWithId, Envelope as EthEnvelope};
use crate::handler::{HandlerError, Secp256k1VerificationClient, SignatureData};

const VERIFICATION_KEY: Symbol = symbol_short!("verif");
const SUBMITTED_TOPIC: Symbol = symbol_short!("submit");

#[contracttype]
enum DataKey {
    Validated(u64),
    Data(u64),
}

/// Test-only Soroban submission destination for the warpdrive e2e tests.
///
/// **This contract IS the service handler** for the test, mirroring the
/// role of the EVM `SimpleSubmit` mock and the real
/// `warpdrive_ethereum_handler` contract. The aggregator targets this
/// contract directly (its address is what gets written into the service
/// definition's `service_handler` config).
///
/// Exposes `verify_eth(envelope_bytes, sig_data)` so it's drop-in
/// compatible with `warpdrive_client::EthereumHandlerClient` — the real
/// aggregator dispatches to either `EthereumHandlerClient` or
/// `StellarHandlerClient` based on `project_root.verification_type()`, and
/// for the secp256k1 (Ethereum-shaped) path our mock IS that handler.
///
/// What it does on `verify_eth`:
///   1. Delegates signature validation to the test's deployed
///      `secp256k1_verification` contract (the address stored at
///      construction). Reverts on validation failure.
///   2. Decodes the envelope's `payload` as ABI-encoded
///      `DataWithId { triggerId: u64, data: bytes }`.
///   3. Stores `(trigger_id → data, trigger_id → validated)` so the test
///      runner can read back via `get_data` / `is_valid_trigger_id`.
///
/// Skipped vs. the real `EthereumHandler` (intentionally — these are
/// handler-level concerns the test doesn't need):
///   - `reference_block` age check (per-test contract is short-lived).
///   - Replay protection / `event_seen` tracking (each test deploys fresh).
#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    pub fn __constructor(env: Env, verification_contract: Address) {
        env.storage()
            .instance()
            .set(&VERIFICATION_KEY, &verification_contract);
    }

    /// Returns the verification contract address (the aggregator's
    /// pre-validation step calls this to discover which secp256k1
    /// verification contract to query for `check_one`/`required_weight`).
    pub fn verification_contract(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&VERIFICATION_KEY)
            .expect("verification contract not set in constructor")
    }

    pub fn verify_eth(
        env: Env,
        envelope_bytes: Bytes,
        sig_data: SignatureData,
    ) -> Result<u64, HandlerError> {
        // Delegate signature validation to the secp256k1 verification
        // contract. Panics (and reverts the whole tx) on bad signatures,
        // unregistered signer, insufficient weight, etc.
        let verification_addr: Address = env
            .storage()
            .instance()
            .get(&VERIFICATION_KEY)
            .expect("verification contract not set in constructor");
        let verification = Secp256k1VerificationClient::new(&env, &verification_addr);
        verification.verify(
            &envelope_bytes,
            &sig_data.signatures,
            &sig_data.signers,
            &sig_data.reference_block,
        );

        // Decode the envelope to pull out the payload.
        let envelope = EthEnvelope::abi_decode_from(&envelope_bytes)
            .ok_or(HandlerError::InvalidEnvelope)?;

        // Payload is itself ABI-encoded `DataWithId { triggerId, data }`.
        let payload_bytes: &[u8] = envelope.payload.as_ref();
        let data_with_id = DataWithId::abi_decode_from_slice(payload_bytes)
            .ok_or(HandlerError::InvalidEnvelope)?;
        let trigger_id: u64 = data_with_id.triggerId;
        let data = Bytes::from_slice(&env, data_with_id.data.as_ref());

        env.storage()
            .persistent()
            .set(&DataKey::Data(trigger_id), &data);
        env.storage()
            .persistent()
            .set(&DataKey::Validated(trigger_id), &true);

        env.events().publish((SUBMITTED_TOPIC, trigger_id), data);
        Ok(trigger_id)
    }

    pub fn is_valid_trigger_id(env: Env, trigger_id: u64) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Validated(trigger_id))
            .unwrap_or(false)
    }

    pub fn get_data(env: Env, trigger_id: u64) -> Option<Bytes> {
        env.storage().persistent().get(&DataKey::Data(trigger_id))
    }
}
