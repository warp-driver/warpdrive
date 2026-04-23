extern crate std;

use soroban_sdk::{Env, String};

use crate::contract::{Contract, ContractClient};

#[test]
fn stores_and_queries_trigger_data() {
    let env = Env::default();
    let contract_id = env.register(Contract, ());
    let client = ContractClient::new(&env, &contract_id);

    let first = client.add_trigger(&String::from_str(&env, "first"));
    let second = client.add_trigger(&String::from_str(&env, "second"));

    assert_eq!(first, 1);
    assert_eq!(second, 2);
    assert_eq!(client.get_trigger(&1), String::from_str(&env, "first"));
    assert_eq!(client.get_trigger(&2), String::from_str(&env, "second"));
}
