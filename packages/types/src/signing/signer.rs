use crate::{
    ByteArray, ChainAddress, Envelope, SignatureAlgorithm, SignatureData, SignaturePrefix,
    SigningError, WavsSignable, WavsSignature,
};
use alloy_primitives::FixedBytes;
use alloy_signer::Signer;
use alloy_signer_local::PrivateKeySigner;
use anyhow::bail;
use async_trait::async_trait;
use k256::ecdsa::signature::SignerMut;

#[derive(Clone, Debug)]
pub enum VectrSigner {
    Evm(PrivateKeySigner),
    EvmNoPrefix(PrivateKeySigner),
    Stellar(ed25519_dalek::SigningKey),
}

impl VectrSigner {
    pub async fn sign_envelope(&mut self, envelope: &Envelope) -> anyhow::Result<WavsSignature> {
        match self {
            VectrSigner::Evm(signer) => {
                let hash = envelope.prefix_eip191_hash()?;
                let signature: Vec<u8> = signer.sign_hash(&hash).await?.into();
                if signature.len() != 65 {
                    bail!("Recoverable Secp256k1 expects signature length of 65");
                }
                Ok(WavsSignature::Secp256k1 {
                    signature: signature.try_into()?,
                    prefix: Some(SignaturePrefix::Eip191),
                })
            }
            VectrSigner::EvmNoPrefix(signer) => {
                let hash = envelope.unprefixed_hash()?;
                let signature: Vec<u8> = signer.sign_hash(&hash).await?.into();
                if signature.len() != 65 {
                    bail!("Recoverable Secp256k1 expects signature length of 65");
                }
                Ok(WavsSignature::Secp256k1 {
                    signature: signature.try_into()?,
                    prefix: None,
                })
            }
            VectrSigner::Stellar(signer) => {
                let hash = envelope.prefix_sep53_hash()?;
                let sig_bytes = signer.sign(hash.as_slice()).to_bytes();
                let pubkey_bytes = *signer.verifying_key().as_bytes();
                Ok(WavsSignature::Ed25519 {
                    signature: ByteArray::new(sig_bytes),
                    pubkey: ByteArray::new(pubkey_bytes),
                })
            }
        }
    }

    pub fn address(&self) -> ChainAddress {
        match self {
            VectrSigner::Evm(signer) => ChainAddress::Evm(signer.address()),
            VectrSigner::EvmNoPrefix(signer) => ChainAddress::Evm(signer.address()),
            VectrSigner::Stellar(signer) => {
                ChainAddress::StellarPubKey(ByteArray::new(*signer.verifying_key().as_bytes()))
            }
        }
    }

    /// The operator's public key in the form on-chain registration
    /// expects, hex-encoded (no `0x` prefix).
    ///
    /// - `Evm` / `EvmNoPrefix`: the 33-byte compressed SEC1 secp256k1
    ///   public key. This is what both an EVM service manager and a
    ///   Stellar *secp256k1* security contract register against. It is
    ///   NOT recoverable from the 20-byte EVM address (that's a one-way
    ///   keccak hash), which is why the address alone is insufficient
    ///   for Stellar-secp256k1 registration.
    /// - `Stellar`: the raw 32-byte Ed25519 public key, as the Stellar
    ///   *ed25519* security contract's `add_signer` expects (the `G...`
    ///   strkey is an account-id encoding of these same bytes).
    pub fn registration_pubkey_hex(&self) -> String {
        match self {
            VectrSigner::Evm(signer) | VectrSigner::EvmNoPrefix(signer) => {
                const_hex::encode(signer.credential().verifying_key().to_sec1_bytes())
            }
            VectrSigner::Stellar(signer) => const_hex::encode(signer.verifying_key().as_bytes()),
        }
    }

    // just used in tests
    pub fn as_evm_signer(&self) -> Option<&PrivateKeySigner> {
        match self {
            VectrSigner::Evm(signer) | VectrSigner::EvmNoPrefix(signer) => Some(signer),
            VectrSigner::Stellar(_) => None,
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait WavsSigner: WavsSignable {
    /// Build the EVM service-manager `SignatureData` (sorted signer
    /// addresses + signatures + reference block). EVM-only: a Stellar
    /// path would produce a different on-chain shape and lives next to
    /// the Stellar submit handler.
    fn evm_signature_data(
        &self,
        signatures: Vec<WavsSignature>,
        block_height: u64,
    ) -> std::result::Result<SignatureData, SigningError> {
        let mut signers_and_signatures: Vec<(alloy_primitives::Address, alloy_primitives::Bytes)> =
            signatures
                .into_iter()
                .map(|sig| {
                    let bytes: alloy_primitives::Bytes = match &sig {
                        WavsSignature::Secp256k1 {
                            signature: bytes, ..
                        } => bytes.into_inner().to_vec().into(),
                        WavsSignature::Ed25519 { .. } => {
                            return Err(SigningError::WrongAddressKind {
                                expected: SignatureAlgorithm::Secp256k1,
                                actual: SignatureAlgorithm::Ed25519,
                            });
                        }
                    };
                    let addr = sig.signer_address(self)?.try_as_evm().ok_or(
                        SigningError::WrongAddressKind {
                            expected: SignatureAlgorithm::Secp256k1,
                            actual: SignatureAlgorithm::Ed25519,
                        },
                    )?;
                    Ok((addr, bytes))
                })
                .collect::<Result<_, _>>()?;

        // Solidity‑compatible ascending order (lexicographic / numeric)
        signers_and_signatures.sort_by_key(|(addr, _)| *addr);

        // unzip back into two parallel, sorted vectors
        let (signers, signatures): (Vec<alloy_primitives::Address>, Vec<alloy_primitives::Bytes>) =
            signers_and_signatures.into_iter().unzip();

        Ok(SignatureData {
            signers,
            signatures,
            referenceBlock: block_height as u32,
        })
    }
}

impl<T> WavsSigner for T where T: WavsSignable {}

impl WavsSignature {
    /// Chain-agnostic signer identifier:
    /// - Secp256k1: recover the 20-byte address from `(sig, prehash)`
    ///   and wrap it as `ChainAddress::Evm`.
    /// - Ed25519: verify the bundled pubkey against `(sig, prehash)`
    ///   and wrap it as `ChainAddress::StellarPubKey`.
    pub fn signer_address<T: WavsSignable + ?Sized>(
        &self,
        signable: &T,
    ) -> std::result::Result<ChainAddress, SigningError> {
        match self {
            WavsSignature::Secp256k1 { signature, prefix } => {
                let signature = alloy_primitives::Signature::from_raw(&signature.into_inner())
                    .map_err(SigningError::RecoverSignerAddress)?;
                let prehash = prehash_for(signable, prefix.as_ref())?;
                let addr = signature
                    .recover_address_from_prehash(&prehash)
                    .map_err(SigningError::RecoverSignerAddress)?;
                Ok(ChainAddress::Evm(addr))
            }
            WavsSignature::Ed25519 { signature, pubkey } => {
                use ed25519_dalek::{Signature, Verifier, VerifyingKey};

                let prehash = signable
                    .prefix_sep53_hash()
                    .map_err(SigningError::DataHash)?;
                let vk = VerifyingKey::from_bytes(&pubkey.into_inner())
                    .map_err(|_| SigningError::Ed25519Verify)?;
                let signature = Signature::from_bytes(&signature.into_inner());
                vk.verify(prehash.as_slice(), &signature)
                    .map_err(|_| SigningError::Ed25519Verify)?;
                Ok(ChainAddress::StellarPubKey(*pubkey))
            }
        }
    }

    /// Recover the 33-byte SEC1-compressed secp256k1 public key from
    /// the signature, given the data the operator signed. This is the
    /// shape the Stellar verification contract expects (its
    /// `signer_pubkey` argument). EVM and Cosmos service managers
    /// instead want the 20-byte address, available via
    /// `signer_address`.
    ///
    /// Both return values come from the same signature and the same
    /// recovered public key — `signer_address` keccak-256-hashes
    /// it and takes the last 20 bytes; this method keeps the full
    /// SEC1 compressed encoding.
    pub fn secp256k1_compressed_pubkey<T: WavsSignable + ?Sized>(
        &self,
        signable: &T,
    ) -> std::result::Result<[u8; 33], SigningError> {
        use k256::ecdsa::{RecoveryId, Signature as K256Sig, VerifyingKey};

        let (sig_bytes, prefix) = match self {
            WavsSignature::Secp256k1 { signature, prefix } => (signature.into_inner(), prefix),
            WavsSignature::Ed25519 { .. } => {
                return Err(SigningError::WrongAddressKind {
                    expected: SignatureAlgorithm::Secp256k1,
                    actual: SignatureAlgorithm::Ed25519,
                });
            }
        };

        let r_s: [u8; 64] = sig_bytes[..64]
            .try_into()
            .expect("65-byte slice gives 64-byte head");
        let v = sig_bytes[64];
        let k_sig = K256Sig::from_slice(&r_s).map_err(|_| {
            SigningError::RecoverSignerAddress(alloy_primitives::SignatureError::FromBytes(
                "invalid secp256k1 r||s",
            ))
        })?;
        // Operator signatures use Ethereum-style v (27/28); reduce to
        // the 0/1 recovery id k256 expects.
        let recid_byte = if v >= 27 { v - 27 } else { v };
        let recid = RecoveryId::try_from(recid_byte).map_err(|_| {
            SigningError::RecoverSignerAddress(alloy_primitives::SignatureError::FromBytes(
                "invalid recovery id",
            ))
        })?;
        let prehash = prehash_for(signable, prefix.as_ref())?;
        let vk = VerifyingKey::recover_from_prehash(prehash.as_slice(), &k_sig, recid).map_err(
            |_| {
                SigningError::RecoverSignerAddress(alloy_primitives::SignatureError::FromBytes(
                    "recovery failed",
                ))
            },
        )?;
        let pubkey_bytes = vk.to_sec1_bytes();
        pubkey_bytes.as_ref().try_into().map_err(|_| {
            SigningError::RecoverSignerAddress(alloy_primitives::SignatureError::FromBytes(
                "expected 33-byte compressed pubkey",
            ))
        })
    }
}

fn prehash_for<T: WavsSignable + ?Sized>(
    signable: &T,
    prefix: Option<&SignaturePrefix>,
) -> std::result::Result<FixedBytes<32>, SigningError> {
    match prefix {
        Some(SignaturePrefix::Eip191) => signable
            .prefix_eip191_hash()
            .map_err(SigningError::DataHash),
        // Covered for completeness; secp256k1 + Sep53 is not a path we
        // emit today.
        Some(SignaturePrefix::Sep53) => {
            signable.prefix_sep53_hash().map_err(SigningError::DataHash)
        }
        None => signable.unprefixed_hash().map_err(SigningError::DataHash),
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::{Envelope, EvmEnvelope, SignaturePrefix};
    use alloy_primitives::FixedBytes;
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;
    use k256::ecdsa::VerifyingKey;

    fn sample_envelope() -> Envelope {
        Envelope::Evm {
            data: EvmEnvelope {
                eventId: FixedBytes::from([7u8; 20]),
                ordering: FixedBytes::from([0u8; 12]),
                payload: alloy_primitives::Bytes::from(b"hello world".to_vec()),
            },
        }
    }

    fn signer_sign(env: &Envelope, signer: &PrivateKeySigner) -> WavsSignature {
        let hash = env.prefix_eip191_hash().expect("eip-191 hash");
        let signature = signer.sign_hash_sync(&hash).expect("sign");
        WavsSignature::Secp256k1 {
            signature: signature.as_bytes().into(),
            prefix: Some(SignaturePrefix::Eip191),
        }
    }

    /// `secp256k1_compressed_pubkey` and `signer_address` recover the
    /// *same* public key — they differ only in encoding. This test
    /// signs a real envelope, recovers both forms, and verifies the
    /// relationship: `keccak256(uncompressed_pubkey)[12..] == address`.
    #[test]
    fn compressed_pubkey_decompresses_to_signer_address() {
        let signer = PrivateKeySigner::random();
        let expected_address = signer.address();
        let env = sample_envelope();
        let sig = signer_sign(&env, &signer);

        let recovered = sig
            .signer_address(&env)
            .expect("recover evm address")
            .try_as_evm()
            .expect("secp256k1 path returns evm address");
        assert_eq!(recovered, expected_address, "address recovery sanity");

        let compressed = sig
            .secp256k1_compressed_pubkey(&env)
            .expect("recover compressed pubkey");

        // Decompress the SEC1-compressed pubkey, drop the 0x04 prefix
        // byte that uncompressed SEC1 starts with, hash the remaining
        // 64 bytes with keccak256, and take the last 20 bytes — that
        // should equal the EVM address.
        let vk = VerifyingKey::from_sec1_bytes(&compressed).expect("decompress sec1 pubkey");
        let uncompressed = vk.to_encoded_point(false);
        let xy = &uncompressed.as_bytes()[1..]; // strip the 0x04 prefix
        let digest = alloy_primitives::keccak256(xy);
        let derived = alloy_primitives::Address::from_slice(&digest[12..]);

        assert_eq!(
            derived, expected_address,
            "compressed pubkey did not decompress to the expected EVM address"
        );
    }

    #[tokio::test]
    async fn ed25519_round_trip_recovers_stellar_pubkey() {
        use ed25519_dalek::SigningKey;

        // Deterministic seed — we only care that sign → verify recovers
        // the same pubkey, not that the key was randomly generated.
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let expected_pubkey = *signing_key.verifying_key().as_bytes();
        let env = sample_envelope();

        let mut vsigner = VectrSigner::Stellar(signing_key);
        let sig = vsigner.sign_envelope(&env).await.expect("sign");

        let recovered = sig.signer_address(&env).expect("verify ed25519").as_bytes();
        assert_eq!(recovered, expected_pubkey.to_vec());

        // tampering invalidates verification
        let WavsSignature::Ed25519 {
            signature: ed_sig,
            pubkey,
        } = sig
        else {
            panic!("expected Ed25519 variant");
        };
        let mut tampered = ed_sig.into_inner();
        tampered[0] ^= 0x01;
        let bad = WavsSignature::Ed25519 {
            signature: ByteArray::new(tampered),
            pubkey,
        };
        assert!(matches!(
            bad.signer_address(&env),
            Err(SigningError::Ed25519Verify)
        ));
    }

    /// An `Evm` signer's `registration_pubkey_hex` is the 33-byte
    /// compressed SEC1 secp256k1 pubkey, and it decompresses back to the
    /// signer's EVM address — i.e. it carries strictly more information
    /// than `evm_address` (which is a one-way keccak hash).
    #[test]
    fn registration_pubkey_hex_evm_is_compressed_secp256k1() {
        let signer = PrivateKeySigner::random();
        let expected_address = signer.address();

        let vsigner = VectrSigner::Evm(signer);
        let hex = vsigner.registration_pubkey_hex();
        let bytes = const_hex::decode(&hex).expect("valid hex");
        assert_eq!(bytes.len(), 33, "compressed SEC1 pubkey is 33 bytes");

        let vk = VerifyingKey::from_sec1_bytes(&bytes).expect("decompress sec1 pubkey");
        let uncompressed = vk.to_encoded_point(false);
        let xy = &uncompressed.as_bytes()[1..];
        let digest = alloy_primitives::keccak256(xy);
        let derived = alloy_primitives::Address::from_slice(&digest[12..]);

        assert_eq!(
            derived, expected_address,
            "registration pubkey did not decompress to the signer's EVM address"
        );
    }

    /// A `Stellar` signer's `registration_pubkey_hex` is the raw 32-byte
    /// Ed25519 public key (what the ed25519 security contract expects),
    /// not the `G...` strkey encoding.
    #[test]
    fn registration_pubkey_hex_stellar_is_raw_ed25519() {
        use ed25519_dalek::SigningKey;

        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let expected = *signing_key.verifying_key().as_bytes();

        let vsigner = VectrSigner::Stellar(signing_key);
        let hex = vsigner.registration_pubkey_hex();
        let bytes = const_hex::decode(&hex).expect("valid hex");

        assert_eq!(bytes.len(), 32, "raw ed25519 pubkey is 32 bytes");
        assert_eq!(bytes, expected.to_vec());
    }
}
