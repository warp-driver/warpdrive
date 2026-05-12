use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use utoipa::ToSchema;

use crate::ByteArray;

/// A chain-agnostic on-chain address.
///
/// Use this type whenever you need to refer to "an address on some chain"
/// without committing to a specific chain at the type level — for example,
/// the BiMap that maps a service id to its service-manager contract address
/// across EVM / Cosmos / Stellar.
///
/// `From<layer_climb_address::Address>` is infallible: layer-climb only knows
/// EVM and Cosmos, so converting *into* this type can't fail.
/// `TryFrom<ChainAddress> for layer_climb_address::Address` is fallible: it
/// fails on the `Stellar` variant since layer-climb has no Stellar variant.
#[derive(Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "namespace", content = "address")]
pub enum ChainAddress {
    #[schema(value_type = String)]
    Evm(alloy_primitives::Address),
    #[schema(value_type = String)]
    Cosmos(layer_climb_address::CosmosAddr),
    #[schema(value_type = String)]
    StellarContract(stellar_strkey::Contract),
    #[schema(value_type = String)]
    StellarPubKey(ByteArray<32>),
}

impl ChainAddress {
    /// Raw bytes of the address; useful for hashing into a chain-agnostic id.
    pub fn as_bytes(&self) -> Vec<u8> {
        match self {
            ChainAddress::Evm(addr) => addr.as_slice().to_vec(),
            ChainAddress::Cosmos(addr) => addr.to_vec(),
            ChainAddress::StellarContract(contract) => contract.0.to_vec(),
            ChainAddress::StellarPubKey(key) => key.as_slice().to_vec(),
        }
    }

    pub fn try_as_evm(&self) -> Option<alloy_primitives::Address> {
        match self {
            ChainAddress::Evm(addr) => Some(*addr),
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ChainAddressConversionError {
    #[error("Stellar contract id cannot be represented as a layer_climb_address::Address")]
    StellarNotSupportedByLayerClimb,
}

impl From<alloy_primitives::Address> for ChainAddress {
    fn from(addr: alloy_primitives::Address) -> Self {
        ChainAddress::Evm(addr)
    }
}

impl From<layer_climb_address::CosmosAddr> for ChainAddress {
    fn from(addr: layer_climb_address::CosmosAddr) -> Self {
        ChainAddress::Cosmos(addr)
    }
}

impl From<stellar_strkey::Contract> for ChainAddress {
    fn from(contract: stellar_strkey::Contract) -> Self {
        ChainAddress::StellarContract(contract)
    }
}

/// Lossless: layer-climb only knows EVM and Cosmos, both of which round-trip.
impl From<layer_climb_address::Address> for ChainAddress {
    fn from(addr: layer_climb_address::Address) -> Self {
        match addr {
            layer_climb_address::Address::Evm(evm) => {
                let bytes: [u8; 20] = evm.as_bytes();
                ChainAddress::Evm(alloy_primitives::Address::from(bytes))
            }
            layer_climb_address::Address::Cosmos(cosmos) => ChainAddress::Cosmos(cosmos),
        }
    }
}

/// Fallible: Stellar can't be expressed as a `layer_climb_address::Address`.
impl TryFrom<ChainAddress> for layer_climb_address::Address {
    type Error = ChainAddressConversionError;

    fn try_from(value: ChainAddress) -> Result<Self, Self::Error> {
        match value {
            ChainAddress::Evm(addr) => Ok(layer_climb_address::Address::Evm(addr.into())),
            ChainAddress::Cosmos(addr) => Ok(layer_climb_address::Address::Cosmos(addr)),
            ChainAddress::StellarContract(_) | ChainAddress::StellarPubKey(_) => {
                Err(ChainAddressConversionError::StellarNotSupportedByLayerClimb)
            }
        }
    }
}

impl fmt::Display for ChainAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChainAddress::Evm(addr) => write!(f, "evm:{addr}"),
            ChainAddress::Cosmos(addr) => write!(f, "cosmos:{addr}"),
            ChainAddress::StellarContract(contract) => write!(f, "stellar_contract:{contract}"),
            ChainAddress::StellarPubKey(pubkey) => write!(f, "stellar_pubkey:{pubkey}"),
        }
    }
}

#[derive(Debug, Error)]
pub enum ChainAddressParseError {
    #[error("missing chain namespace prefix; expected `evm:...`, `cosmos:...`, or `stellar:...`")]
    MissingNamespace,
    #[error("unknown chain namespace `{0}`")]
    UnknownNamespace(String),
    #[error("invalid evm address: {0}")]
    Evm(String),
    #[error("invalid cosmos address: {0}")]
    Cosmos(String),
    #[error("invalid stellar contract id: {0}")]
    StellarContract(String),
    #[error("invalid stellar contract pubkey: {0}")]
    StellarPubkey(String),
}

impl FromStr for ChainAddress {
    type Err = ChainAddressParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (namespace, rest) = s
            .split_once(':')
            .ok_or(ChainAddressParseError::MissingNamespace)?;
        match namespace {
            "evm" => alloy_primitives::Address::from_str(rest)
                .map(ChainAddress::Evm)
                .map_err(|e| ChainAddressParseError::Evm(e.to_string())),
            "cosmos" => layer_climb_address::CosmosAddr::new_str(rest, None)
                .map(ChainAddress::Cosmos)
                .map_err(|e| ChainAddressParseError::Cosmos(e.to_string())),
            "stellar_contract" => stellar_strkey::Contract::from_string(rest)
                .map(ChainAddress::StellarContract)
                .map_err(|e| ChainAddressParseError::StellarContract(format!("{e:?}"))),
            "stellar_pubkey" => const_hex::decode_to_array(rest)
                .map(|bytes| ChainAddress::StellarPubKey(ByteArray::new(bytes)))
                .map_err(|e| ChainAddressParseError::StellarPubkey(format!("{e:?}"))),
            other => Err(ChainAddressParseError::UnknownNamespace(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evm_round_trip_through_layer_climb() {
        let evm = alloy_primitives::Address::from([0x42u8; 20]);
        let chain: ChainAddress = evm.into();
        let lc: layer_climb_address::Address = chain.clone().try_into().unwrap();
        let round: ChainAddress = lc.into();
        assert_eq!(chain, round);
    }

    #[test]
    fn stellar_does_not_fit_layer_climb() {
        let chain = ChainAddress::StellarContract(stellar_strkey::Contract([0u8; 32]));
        let res: Result<layer_climb_address::Address, _> = chain.try_into();
        assert!(matches!(
            res,
            Err(ChainAddressConversionError::StellarNotSupportedByLayerClimb)
        ));
    }

    #[test]
    fn display_and_parse_round_trip() {
        let cases = [
            ChainAddress::Evm(alloy_primitives::Address::from([0xab; 20])),
            ChainAddress::StellarContract(stellar_strkey::Contract([0xcd; 32])),
        ];
        for case in cases {
            let s = case.to_string();
            let parsed: ChainAddress = s.parse().unwrap();
            assert_eq!(case, parsed);
        }
    }
}
