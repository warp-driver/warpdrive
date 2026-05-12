extern crate alloc;

use alloy_sol_types::sol;
use soroban_sdk::Bytes;

sol! {
    struct Envelope {
        bytes20 eventId;
        bytes12 ordering;
        bytes payload;
    }

    /// Solidity-side trigger identifier (mirrors `ISimpleTrigger.TriggerId`).
    type TriggerId is uint64;

    /// Inner payload carried inside `Envelope.payload`. Mirrors WAVS's
    /// `IWavsServiceHandler.DataWithId` so the same bytes can be exchanged
    /// across the EVM/WAVS boundary.
    struct DataWithId {
        TriggerId triggerId;
        bytes data;
    }
}

impl Envelope {
    pub fn abi_decode_from(data: &Bytes) -> Option<Self> {
        let mut buf = alloc::vec![0u8; data.len() as usize];
        data.copy_into_slice(&mut buf);
        <Envelope as alloy_sol_types::SolValue>::abi_decode(&buf).ok()
    }
}

impl DataWithId {
    pub fn abi_decode_from_bytes(data: &[u8]) -> Option<Self> {
        <DataWithId as alloy_sol_types::SolValue>::abi_decode(data).ok()
    }
}
