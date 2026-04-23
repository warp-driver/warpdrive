use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, Env, String, Symbol};

const TRIGGER_TOPIC: Symbol = symbol_short!("trigger");

#[contracttype]
enum DataKey {
    NextTriggerId,
    Trigger(u64),
}

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    pub fn add_trigger(env: Env, data: String) -> u64 {
        let trigger_id = env
            .storage()
            .persistent()
            .get::<_, u64>(&DataKey::NextTriggerId)
            .unwrap_or(0)
            + 1;

        env.storage()
            .persistent()
            .set(&DataKey::NextTriggerId, &trigger_id);
        env.storage()
            .persistent()
            .set(&DataKey::Trigger(trigger_id), &data);

        env.events().publish((TRIGGER_TOPIC, trigger_id), data);

        trigger_id
    }

    pub fn get_trigger(env: Env, trigger_id: u64) -> String {
        env.storage()
            .persistent()
            .get(&DataKey::Trigger(trigger_id))
            .unwrap()
    }
}
