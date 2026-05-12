//! Most of the stellar client code is in the `warpdrive-client` crate
//! This module is just for some helpers

use std::sync::LazyLock;

use bip39::Mnemonic;
use thiserror::Error;
use warpdrive_types::Credential;

pub static STELLAR_QUERY_KEY: LazyLock<ed25519_dalek::SigningKey> =
    LazyLock::new(|| ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]));

#[derive(Debug, Error)]
pub enum StellarSignerError {
    #[error("invalid bip39 mnemonic: {0}")]
    BadMnemonic(bip39::Error),
    #[error(
        "stellar signing requires a BIP-39 mnemonic; raw private-key credentials are not supported"
    )]
    RawPrivateKeyUnsupported,
}

/// Derive a Stellar/Ed25519 signing key from the operator's BIP-39
/// mnemonic using SEP-0005 / SLIP-0010 (path `m/44'/148'/{hd_index}'`).
///
/// The same `hd_index` used on the EVM side produces a *different*
/// (independent) Stellar key, since the two chains use different
/// elliptic curves with different derivation rules. Callers can reuse
/// the index counter without worrying about cross-chain collisions.
///
/// Raw `0x`-prefixed private-key credentials are rejected: a single
/// secp256k1 secret can't be reused as an Ed25519 secret.
pub fn make_stellar_signer(
    credentials: &Credential,
    hd_index: Option<u32>,
) -> Result<ed25519_dalek::SigningKey, StellarSignerError> {
    if credentials.as_str().starts_with("0x") {
        return Err(StellarSignerError::RawPrivateKeyUnsupported);
    }
    let mnemonic =
        Mnemonic::parse(credentials.as_str()).map_err(StellarSignerError::BadMnemonic)?;
    let seed: [u8; 64] = mnemonic.to_seed("");

    // slip10_ed25519 treats every index as hardened (ed25519 has no
    // non-hardened child derivation), so we pass the raw component
    // numbers and get hardened semantics implicitly.
    let path: [u32; 3] = [44, 148, hd_index.unwrap_or(0)];
    let secret = slip10_ed25519::derive_ed25519_private_key(&seed, &path);
    Ok(ed25519_dalek::SigningKey::from_bytes(&secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHRASE: &str = "illness spike retreat truth genius clock brain pass fit cave bargain toe";

    /// Derivation is deterministic — same phrase + same index always
    /// yields the same key.
    #[test]
    fn deterministic_across_calls() {
        let cred = Credential::new(PHRASE.to_string());
        let a = make_stellar_signer(&cred, Some(0)).unwrap();
        let b = make_stellar_signer(&cred, Some(0)).unwrap();
        assert_eq!(a.verifying_key().as_bytes(), b.verifying_key().as_bytes());
    }

    /// Different indices yield independent keys; this is what lets us
    /// reuse the EVM hd_index counter without cross-account collisions.
    #[test]
    fn different_indices_yield_different_keys() {
        let cred = Credential::new(PHRASE.to_string());
        let a = make_stellar_signer(&cred, Some(0)).unwrap();
        let b = make_stellar_signer(&cred, Some(1)).unwrap();
        assert_ne!(a.verifying_key().as_bytes(), b.verifying_key().as_bytes());
    }

    /// Index `None` and `Some(0)` are the same account (SEP-0005 m/44'/148'/0').
    #[test]
    fn default_index_matches_zero() {
        let cred = Credential::new(PHRASE.to_string());
        let none = make_stellar_signer(&cred, None).unwrap();
        let zero = make_stellar_signer(&cred, Some(0)).unwrap();
        assert_eq!(
            none.verifying_key().as_bytes(),
            zero.verifying_key().as_bytes()
        );
    }

    #[test]
    fn raw_private_key_credential_is_rejected() {
        let cred = Credential::new("0xdeadbeef".to_string());
        assert!(matches!(
            make_stellar_signer(&cred, Some(0)),
            Err(StellarSignerError::RawPrivateKeyUnsupported)
        ));
    }

    #[test]
    fn malformed_mnemonic_is_rejected() {
        let cred = Credential::new("not actually a mnemonic phrase".to_string());
        assert!(matches!(
            make_stellar_signer(&cred, Some(0)),
            Err(StellarSignerError::BadMnemonic(_))
        ));
    }
}
