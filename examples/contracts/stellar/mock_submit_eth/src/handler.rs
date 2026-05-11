use soroban_sdk::{contractclient, contracterror, contracttype, Address, Bytes, BytesN, Env, Vec};

// Mirrors `warpdrive_shared::interfaces::handler::SignatureData`. We
// re-declare it here because `warpdrive-shared` is part of the contracts
// repo workspace and not published to crates.io. The on-the-wire ABI must
// match what the deployed `EthereumHandler` contract expects, so the same
// `EthereumHandlerClient` from `warpdrive-client` can target this mock.
#[contracttype]
pub struct SignatureData {
    pub signers: Vec<BytesN<33>>,
    pub signatures: Vec<BytesN<65>>,
    pub reference_block: u32,
}

// Subset of `warpdrive_shared::interfaces::handler::HandlerError`, just
// enough for our mock to surface decode failures. Signature/quorum
// failures inside the verification call panic at the contract level
// (which reverts the whole tx — same effect as the real handler).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum HandlerError {
    InvalidEnvelope = 503,
}

// Subset of `warpdrive_shared::interfaces::verification::Secp256k1VerificationInterface`.
// `#[contractclient]` generates a `Secp256k1VerificationClient` we use to
// delegate signature validation to the test's deployed verification
// contract (per-test isolation: each test's stack has its own).
#[contractclient(name = "Secp256k1VerificationClient")]
pub trait Secp256k1VerificationInterface {
    fn verify(
        env: Env,
        envelope: Bytes,
        signatures: Vec<BytesN<65>>,
        signer_pubkeys: Vec<BytesN<33>>,
        reference_block: u32,
    );
}
