pub use crate::solidity_types::Envelope;
use crate::{
    ByteArray, ChainAddress, SignatureAlgorithm, SignatureData, SignatureKind, SignaturePrefix,
    SigningError, WavsSignable, WavsSignature,
};
use alloy_primitives::FixedBytes;
use alloy_signer::Signer;
use alloy_signer_local::PrivateKeySigner;
use async_trait::async_trait;
use k256::ecdsa::signature::SignerMut;

#[derive(Clone, Debug)]
pub enum VectrSigner {
    Evm(PrivateKeySigner),
    EvmNoPrefix(PrivateKeySigner),
    Stellar(ed25519_dalek::SigningKey),
}

impl VectrSigner {
    pub fn kind(&self) -> SignatureKind {
        match self {
            VectrSigner::Evm(_) => SignatureKind {
                algorithm: SignatureAlgorithm::Secp256k1,
                prefix: Some(SignaturePrefix::Eip191),
            },
            VectrSigner::EvmNoPrefix(_) => SignatureKind {
                algorithm: SignatureAlgorithm::Secp256k1,
                prefix: None,
            },
            VectrSigner::Stellar(_) => SignatureKind {
                algorithm: SignatureAlgorithm::Ed25519,
                prefix: Some(SignaturePrefix::Sep53),
            },
        }
    }

    pub async fn sign_envelope(&mut self, envelope: &Envelope) -> anyhow::Result<WavsSignature> {
        let hash = match self {
            VectrSigner::Evm(_) => envelope.prefix_eip191_hash()?,
            VectrSigner::EvmNoPrefix(_) => envelope.unprefixed_hash()?,
            VectrSigner::Stellar(_) => envelope.prefix_sep53_hash()?,
        };

        let data = self.sign_hash(&hash).await?;

        Ok(WavsSignature {
            data,
            kind: self.kind(),
        })
    }

    pub async fn sign_hash(&mut self, hash: &FixedBytes<32>) -> anyhow::Result<Vec<u8>> {
        match self {
            VectrSigner::Evm(signer) => Ok(signer.sign_hash(hash).await?.into()),
            VectrSigner::EvmNoPrefix(signer) => Ok(signer.sign_hash(hash).await?.into()),
            VectrSigner::Stellar(signer) => Ok(signer.sign(hash.as_slice()).to_vec()),
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
    fn evm_signature_data(
        &self,
        signatures: Vec<WavsSignature>,
        block_height: u64,
    ) -> std::result::Result<SignatureData, SigningError> {
        let mut signers_and_signatures: Vec<(alloy_primitives::Address, alloy_primitives::Bytes)> =
            signatures
                .into_iter()
                .map(|sig| {
                    sig.evm_signer_address(self)
                        .map(|addr| (addr, sig.data.into()))
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
    pub fn evm_signer_address<T: WavsSignable + ?Sized>(
        &self,
        signable: &T,
    ) -> std::result::Result<alloy_primitives::Address, SigningError> {
        match self.kind.algorithm {
            SignatureAlgorithm::Secp256k1 => {
                let signature = alloy_primitives::Signature::from_raw(&self.data)
                    .map_err(SigningError::RecoverSignerAddress)?;

                match self.kind.prefix {
                    Some(SignaturePrefix::Eip191) => signature
                        .recover_address_from_prehash(
                            &signable
                                .prefix_eip191_hash()
                                .map_err(SigningError::DataHash)?,
                        )
                        .map_err(SigningError::RecoverSignerAddress),
                    // This probably doesn't make sense for EIP-191, but we might as well cover the variant
                    Some(SignaturePrefix::Sep53) => signature
                        .recover_address_from_prehash(
                            &signable
                                .prefix_sep53_hash()
                                .map_err(SigningError::DataHash)?,
                        )
                        .map_err(SigningError::RecoverSignerAddress),
                    None => signature
                        .recover_address_from_prehash(
                            &signable.unprefixed_hash().map_err(SigningError::DataHash)?,
                        )
                        .map_err(SigningError::RecoverSignerAddress),
                }
            }
            SignatureAlgorithm::Ed25519 => Err(SigningError::WrongAddressKind {
                expected: SignatureAlgorithm::Secp256k1,
                actual: SignatureAlgorithm::Ed25519,
            }),
        }
    }

    /// Recover the 33-byte SEC1-compressed secp256k1 public key from
    /// the signature, given the data the operator signed. This is the
    /// shape the Stellar verification contract expects (its
    /// `signer_pubkey` argument). EVM and Cosmos service managers
    /// instead want the 20-byte address, available via
    /// `evm_signer_address`.
    ///
    /// Both return values come from the same signature and the same
    /// recovered public key — `evm_signer_address` keccak-256-hashes
    /// it and takes the last 20 bytes; this method keeps the full
    /// SEC1 compressed encoding.
    pub fn secp256k1_compressed_pubkey<T: WavsSignable + ?Sized>(
        &self,
        signable: &T,
    ) -> std::result::Result<[u8; 33], SigningError> {
        use k256::ecdsa::{RecoveryId, Signature as K256Sig, VerifyingKey};

        match self.kind.algorithm {
            SignatureAlgorithm::Secp256k1 => {
                if self.data.len() != 65 {
                    return Err(SigningError::RecoverSignerAddress(
                        alloy_primitives::SignatureError::FromBytes(
                            "expected 65-byte secp256k1 signature",
                        ),
                    ));
                }
                let r_s: [u8; 64] = self.data[..64]
                    .try_into()
                    .expect("65-byte slice gives 64-byte head");
                let v = self.data[64];
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
                let prehash = match self.kind.prefix {
                    Some(SignaturePrefix::Eip191) => signable
                        .prefix_eip191_hash()
                        .map_err(SigningError::DataHash)?,
                    // Again, this probably doesn't make sense for EIP-191, but we might as well cover the variant
                    Some(SignaturePrefix::Sep53) => signable
                        .prefix_sep53_hash()
                        .map_err(SigningError::DataHash)?,
                    None => signable.unprefixed_hash().map_err(SigningError::DataHash)?,
                };
                let vk = VerifyingKey::recover_from_prehash(prehash.as_slice(), &k_sig, recid)
                    .map_err(|_| {
                        SigningError::RecoverSignerAddress(
                            alloy_primitives::SignatureError::FromBytes("recovery failed"),
                        )
                    })?;
                let pubkey_bytes = vk.to_sec1_bytes();
                pubkey_bytes.as_ref().try_into().map_err(|_| {
                    SigningError::RecoverSignerAddress(alloy_primitives::SignatureError::FromBytes(
                        "expected 33-byte compressed pubkey",
                    ))
                })
            }

            SignatureAlgorithm::Ed25519 => Err(SigningError::WrongAddressKind {
                expected: SignatureAlgorithm::Secp256k1,
                actual: SignatureAlgorithm::Ed25519,
            }),
        }
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::{Envelope, SignatureAlgorithm, SignatureKind, SignaturePrefix};
    use alloy_primitives::FixedBytes;
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;
    use k256::ecdsa::VerifyingKey;

    fn sample_envelope() -> Envelope {
        Envelope {
            eventId: FixedBytes::from([7u8; 20]),
            ordering: FixedBytes::from([0u8; 12]),
            payload: alloy_primitives::Bytes::from(b"hello world".to_vec()),
        }
    }

    fn signer_sign(env: &Envelope, signer: &PrivateKeySigner) -> WavsSignature {
        let hash = env.prefix_eip191_hash().expect("eip-191 hash");
        let sig = signer.sign_hash_sync(&hash).expect("sign");
        WavsSignature {
            data: sig.into(),
            kind: SignatureKind {
                algorithm: SignatureAlgorithm::Secp256k1,
                prefix: Some(SignaturePrefix::Eip191),
            },
        }
    }

    /// `secp256k1_compressed_pubkey` and `evm_signer_address` recover
    /// the *same* public key — they differ only in encoding. This
    /// test signs a real envelope, recovers both forms, and verifies
    /// the relationship: `keccak256(uncompressed_pubkey)[12..] == address`.
    #[test]
    fn compressed_pubkey_decompresses_to_signer_address() {
        let signer = PrivateKeySigner::random();
        let expected_address = signer.address();
        let env = sample_envelope();
        let sig = signer_sign(&env, &signer);

        let recovered_address = sig.evm_signer_address(&env).expect("recover evm address");
        assert_eq!(
            recovered_address, expected_address,
            "address recovery sanity"
        );

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

    #[test]
    fn rejects_wrong_length_signature() {
        let env = sample_envelope();
        let bad_sig = WavsSignature {
            data: vec![0u8; 10], // too short
            kind: SignatureKind {
                algorithm: SignatureAlgorithm::Secp256k1,
                prefix: Some(SignaturePrefix::Eip191),
            },
        };
        let err = bad_sig.secp256k1_compressed_pubkey(&env).unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("65-byte"),
            "expected length-error message, got {msg}"
        );
    }
}
