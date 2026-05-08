//! Most of the stellar client code is in the `warpdrive-client` crate
//! This module is just for some helpers

use std::sync::LazyLock;

pub static STELLAR_QUERY_KEY: LazyLock<ed25519_dalek::SigningKey> =
    LazyLock::new(|| ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]));
