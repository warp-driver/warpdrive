use anyhow::Result;
use cw_warpdrive_mock_api::service_handler::TriggerMessageResponse;
use layer_climb::{pool::SigningClientPoolManager, prelude::*};
use warpdrive_types::contracts::cosmwasm::service_handler::WarpDriveSignatureData;

pub struct SimpleCosmosSubmitClient {
    pub signing_client: deadpool::managed::Object<SigningClientPoolManager>,
    pub contract_address: Address,
}

impl SimpleCosmosSubmitClient {
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
        service_manager: &CosmosAddr,
        label: &str,
    ) -> Result<Self> {
        // Use `instantiate2` with a random salt so the predicted contract
        // address is guaranteed unique per call. `contract_instantiate` (the
        // classic message) derives the address from a global `instanceID`
        // counter, which can race under concurrent test execution: two
        // simulates from the chain's pre-commit state may both read the
        // same stale `instanceID`, predict the same address, and the
        // second simulate then fails with `ErrDuplicate` ("contract address
        // already exists, try a different combination of creator, checksum
        // and salt"). instantiate2 sidesteps this entirely because the
        // address is `keccak(creator || code_hash || salt)`.
        let salt = uuid::Uuid::now_v7().as_bytes().to_vec();
        let (addr, _) = signing_client
            .contract_instantiate2(
                None,
                code_id,
                label,
                &cw_warpdrive_mock_api::service_handler::InstantiateMsg {
                    service_manager: service_manager.to_string(),
                },
                Vec::new(),
                salt,
                false,
                None,
            )
            .await?;

        Ok(Self::new(signing_client, addr))
    }

    pub async fn trigger_validated(&self, trigger_id: u64) -> Result<bool> {
        self.signing_client
            .querier
            .contract_smart(
                &self.contract_address,
                &cw_warpdrive_mock_api::service_handler::QueryMsg::TriggerValidated {
                    trigger_id: trigger_id.into(),
                },
            )
            .await
    }

    pub async fn signature_data(&self, trigger_id: u64) -> Result<WarpDriveSignatureData> {
        if !self.trigger_validated(trigger_id).await? {
            return Err(anyhow::anyhow!("trigger not validated"));
        }
        self.signing_client
            .querier
            .contract_smart(
                &self.contract_address,
                &cw_warpdrive_mock_api::service_handler::QueryMsg::SignatureData {
                    trigger_id: trigger_id.into(),
                },
            )
            .await
    }

    pub async fn trigger_message(&self, trigger_id: u64) -> Result<Vec<u8>> {
        let resp: TriggerMessageResponse = self
            .signing_client
            .querier
            .contract_smart(
                &self.contract_address,
                &cw_warpdrive_mock_api::service_handler::QueryMsg::TriggerMessage {
                    trigger_id: trigger_id.into(),
                },
            )
            .await?;

        Ok(resp.message.into())
    }
}
