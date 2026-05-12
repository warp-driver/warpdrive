use soroban_sdk::{Address, Bytes, BytesN, Env, String, contracttype};
use warpdrive_shared::ttl;

#[contracttype]
pub enum DataKey {
    Admin,
    Version,
    VerificationContract,
    // Use unique keys like this, not Map, as Map requires loading all data into memory on load, and these are potentially unbounded.
    EventSeen(BytesN<20>),
    Payload(BytesN<20>),
}

pub fn get_admin(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Admin).unwrap()
}

pub fn set_admin(env: &Env, admin: &Address) {
    env.storage().instance().set(&DataKey::Admin, admin);
}

pub fn get_version(env: &Env) -> String {
    env.storage().instance().get(&DataKey::Version).unwrap()
}

pub fn set_version(env: &Env, version: &String) {
    env.storage().instance().set(&DataKey::Version, version);
}

pub fn get_verification_contract(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::VerificationContract)
        .unwrap()
}

pub fn set_verification_contract(env: &Env, address: &Address) {
    env.storage()
        .instance()
        .set(&DataKey::VerificationContract, address);
}

pub fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(ttl::INSTANCE_RENEWAL_THRESHOLD, ttl::INSTANCE_TARGET_TTL);
}

pub fn is_event_seen(env: &Env, event_id: &BytesN<20>) -> bool {
    let key = DataKey::EventSeen(event_id.clone());
    if env.storage().persistent().has(&key) {
        env.storage().persistent().extend_ttl(
            &key,
            ttl::PERSISTENT_RENEWAL_THRESHOLD,
            ttl::PERSISTENT_TARGET_TTL,
        );
        true
    } else {
        false
    }
}

pub fn mark_event_seen(env: &Env, event_id: &BytesN<20>) {
    let key = DataKey::EventSeen(event_id.clone());
    env.storage().persistent().set(&key, &true);
    env.storage().persistent().extend_ttl(
        &key,
        ttl::PERSISTENT_RENEWAL_THRESHOLD,
        ttl::PERSISTENT_TARGET_TTL,
    );
}

pub fn save_payload(env: &Env, event_id: BytesN<20>, payload: Bytes) {
    let key = DataKey::Payload(event_id);
    env.storage().persistent().set(&key, &payload);
    env.storage().persistent().extend_ttl(
        &key,
        ttl::PERSISTENT_RENEWAL_THRESHOLD,
        ttl::PERSISTENT_TARGET_TTL,
    );
}

pub fn get_payload(env: &Env, event_id: BytesN<20>) -> Option<Bytes> {
    let key = DataKey::Payload(event_id);
    let result = env.storage().persistent().get(&key);
    if result.is_some() {
        env.storage().persistent().extend_ttl(
            &key,
            ttl::PERSISTENT_RENEWAL_THRESHOLD,
            ttl::PERSISTENT_TARGET_TTL,
        );
    }
    result
}
