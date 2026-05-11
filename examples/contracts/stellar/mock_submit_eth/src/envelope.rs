extern crate alloc;

use alloy_sol_types::sol;
use soroban_sdk::Bytes;

sol! {
    struct Envelope {
        bytes20 eventId;
        bytes12 ordering;
        bytes payload;
    }
}

impl Envelope {
    pub fn abi_decode_from(data: &Bytes) -> Option<Self> {
        let mut buf = alloc::vec![0u8; data.len() as usize];
        data.copy_into_slice(&mut buf);
        <Envelope as alloy_sol_types::SolValue>::abi_decode(&buf).ok()
    }
}
