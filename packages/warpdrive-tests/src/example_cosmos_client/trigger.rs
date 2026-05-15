use anyhow::Result;
use cosmwasm_std::{Empty, Uint64};
use cw_warpdrive_trigger_api::simple::{ExecuteMsg, PushMessageEvent};
use layer_climb::prelude::*;

pub struct SimpleCosmosTriggerClient {
    pub signing_client: deadpool::managed::Object<SigningClientPoolManager>,
    pub contract_address: Address,
}

type TriggerId = Uint64;

impl SimpleCosmosTriggerClient {
    pub fn new(
        signing_client: deadpool::managed::Object<SigningClientPoolManager>,
        contract_address: Address,
    ) -> Self {
        Self {
            signing_client,
            contract_address,
        }
    }

    pub async fn new_code_id(
        signing_client: deadpool::managed::Object<SigningClientPoolManager>,
        code_id: u64,
        label: &str,
    ) -> Result<Self> {
        // instantiate2 with a random salt — see the matching comment in
        // `example_cosmos_client/submit.rs`. Classic instantiate's
        // instanceID-counter-based address can race under concurrent
        // simulates and produce `ErrDuplicate` at simulate time.
        let salt = uuid::Uuid::now_v7().as_bytes().to_vec();
        let (addr, _) = signing_client
            .contract_instantiate2(
                None,
                code_id,
                label,
                &Empty {},
                Vec::new(),
                salt,
                false,
                None,
            )
            .await?;

        Ok(Self::new(signing_client, addr))
    }

    pub async fn add_trigger(&self, data: Vec<u8>) -> Result<TriggerId> {
        let res = self
            .signing_client
            .contract_execute(
                &self.contract_address,
                &ExecuteMsg::Push { data: data.into() },
                Vec::new(),
                None,
            )
            .await?;

        let id = CosmosTxEvents::from(&res)
            .events_iter()
            .find_map(|event| {
                let event: cosmwasm_std::Event = event.into();
                match PushMessageEvent::try_from(&event) {
                    Ok(event) => Some(event.trigger_id),
                    Err(_) => None,
                }
            })
            .ok_or_else(|| anyhow::anyhow!("missing trigger id"))?;

        Ok(id)
    }
}
