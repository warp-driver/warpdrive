extern crate alloc;

use alloy_sol_types::sol;
use soroban_sdk::Bytes;

// Mirrors `Envelope` from the warpdrive `ethereum_handler` contract: this is
// the wrapper struct the aggregator builds around the WASI component output
// before submitting it on-chain.
sol! {
    struct Envelope {
        bytes20 eventId;
        bytes12 ordering;
        bytes payload;
    }
}

// Mirrors `ISimpleSubmit.DataWithId` from the EVM mock contract — the
// payload format the WASI component produces. We use the same ABI encoding
// for stellar so the test-side trigger output encoder can be shared.
sol! {
    struct DataWithId {
        uint64 triggerId;
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
    pub fn abi_decode_from_slice(buf: &[u8]) -> Option<Self> {
        <DataWithId as alloy_sol_types::SolValue>::abi_decode(buf).ok()
    }
}
