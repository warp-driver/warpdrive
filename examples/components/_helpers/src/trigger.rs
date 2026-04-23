// Helpers to work with "trigger id" flows - which our example components do
use crate::bindings::world::warpdrive::{
    types::{
        events::{
            TriggerDataAtprotoEvent, TriggerDataCosmosContractEvent, TriggerDataEvmContractEvent,
            TriggerDataStellarContractEvent,
        },
        service::ServiceManager,
    },
    vectr::{input as component_input, output as component_output},
};
use alloy_provider::RootProvider;
use alloy_sol_types::SolValue;
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use cosmwasm_std::HexBinary;
use cw_warpdrive_mock_api::message_with_id::MessageWithId;
use example_submit::DataWithId;
use example_trigger::{NewTrigger, SimpleTrigger, TriggerInfo};
use serde_json::Value as JsonValue;
use warpdrive_wasi_utils::decode_event_log_data;

pub fn decode_trigger_event(trigger_data: component_input::TriggerData) -> Result<(u64, Vec<u8>)> {
    match trigger_data {
        component_input::TriggerData::CosmosContractEvent(TriggerDataCosmosContractEvent {
            event,
            ..
        }) => {
            let event = cosmwasm_std::Event::from(event);
            let event = cw_warpdrive_trigger_api::simple::PushMessageEvent::try_from(&event)?;

            Ok((event.trigger_id.u64(), event.data.to_vec()))
        }
        component_input::TriggerData::EvmContractEvent(TriggerDataEvmContractEvent {
            log, ..
        }) => {
            let event: NewTrigger = decode_event_log_data!(log.data)?;

            let trigger_info = TriggerInfo::abi_decode(&event.triggerData)?;
            Ok((trigger_info.triggerId, trigger_info.data.to_vec()))
        }
        component_input::TriggerData::Raw(bytes) => Ok((0, bytes)),
        component_input::TriggerData::AtprotoEvent(TriggerDataAtprotoEvent {
            record_data,
            sequence,
            ..
        }) => Ok((
            sequence.try_into().expect("Expected sequence to be u64"),
            record_data
                .expect("Record data was not provided")
                .as_bytes()
                .to_vec(),
        )),
        component_input::TriggerData::StellarContractEvent(TriggerDataStellarContractEvent {
            event,
            ..
        }) => {
            let trigger_id = event
                .topic_segments
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("missing stellar trigger id topic segment"))
                .and_then(|value| parse_stellar_u64(value))?;
            let data = parse_stellar_bytes_or_string(&event.value)?;

            Ok((trigger_id, data))
        }
        _ => Err(anyhow::anyhow!("Unsupported trigger data type")),
    }
}

fn parse_stellar_u64(raw: &str) -> Result<u64> {
    if let Ok(value) = parse_stellar_scval_json(raw) {
        return value
            .get("u64")
            .and_then(JsonValue::as_u64)
            .ok_or_else(|| anyhow::anyhow!("stellar topic segment is not a u64: {raw}"));
    }

    let scval = parse_stellar_scval_xdr(raw)?;
    match scval {
        stellar_xdr::curr::ScVal::U64(value) => Ok(value),
        _ => Err(anyhow::anyhow!("stellar topic segment is not a u64: {raw}")),
    }
}

fn parse_stellar_bytes_or_string(raw: &str) -> Result<Vec<u8>> {
    if let Ok(value) = parse_stellar_scval_json(raw) {
        if let Some(string_value) = value.get("string").and_then(JsonValue::as_str) {
            return Ok(string_value.as_bytes().to_vec());
        }

        if let Some(bytes_value) = value.get("bytes").and_then(JsonValue::as_str) {
            return BASE64_STANDARD.decode(bytes_value).map_err(Into::into);
        }
    }

    match parse_stellar_scval_xdr(raw)? {
        stellar_xdr::curr::ScVal::String(value) => Ok(value.to_string().into_bytes()),
        stellar_xdr::curr::ScVal::Bytes(bytes) => Ok(bytes.to_vec()),
        _ => Err(anyhow::anyhow!(
            "stellar event value is not a supported string/bytes ScVal: {raw}"
        )),
    }
}

fn parse_stellar_scval_json(raw: &str) -> Result<JsonValue> {
    serde_json::from_str(raw).map_err(Into::into)
}

fn parse_stellar_scval_xdr(raw: &str) -> Result<stellar_xdr::curr::ScVal> {
    use stellar_xdr::curr::{Limits, ReadXdr};

    stellar_xdr::curr::ScVal::from_xdr_base64(raw, Limits::none()).map_err(Into::into)
}

pub fn encode_trigger_output(
    trigger_id: u64,
    output: impl AsRef<[u8]>,
    service_manager: ServiceManager,
) -> component_output::WasmResponse {
    match service_manager {
        ServiceManager::Evm(_) => evm_encode_trigger_output(trigger_id, output),
        ServiceManager::Cosmos(_) => cosmos_encode_trigger_output(trigger_id, output),
    }
}
// For EVM ServiceHandler contracts, encode output using DataWithId struct
fn evm_encode_trigger_output(
    trigger_id: u64,
    output: impl AsRef<[u8]>,
) -> component_output::WasmResponse {
    component_output::WasmResponse {
        payload: DataWithId {
            triggerId: trigger_id,
            data: output.as_ref().to_vec().into(),
        }
        .abi_encode(),
        ordering: None,
        event_id_salt: None,
    }
}

// For Cosmos ServiceHandler contracts, encode output using MessageWithId struct
fn cosmos_encode_trigger_output(
    trigger_id: u64,
    output: impl AsRef<[u8]>,
) -> component_output::WasmResponse {
    component_output::WasmResponse {
        payload: MessageWithId {
            trigger_id: cosmwasm_std::Uint64::from(trigger_id),
            message: cosmwasm_std::HexBinary::from(output.as_ref().to_vec()),
        }
        .to_bytes()
        .unwrap(),
        ordering: None,
        event_id_salt: None,
    }
}

// extension traits for Cosmos and EVM queriers to add Trigger support
#[allow(async_fn_in_trait)]
pub trait ChainQuerierExt {
    async fn trigger_data(
        &self,
        address: layer_climb::prelude::Address,
        trigger_id: u64,
    ) -> Result<Vec<u8>>;
}

//new_cosmos_query_client
impl ChainQuerierExt for layer_climb::prelude::QueryClient {
    // on Cosmos, the contract *must* implement a handler for the QueryMsg::TriggerData variant
    async fn trigger_data(
        &self,
        address: layer_climb::prelude::Address,
        trigger_id: u64,
    ) -> Result<Vec<u8>> {
        let resp: HexBinary = self
            .contract_smart(
                &address,
                &cw_warpdrive_trigger_api::simple::QueryMsg::TriggerMessage {
                    trigger_id: trigger_id.into(),
                },
            )
            .await?;

        Ok(resp.into())
    }
}

impl ChainQuerierExt for RootProvider {
    // convenience helper for typical use-case of querying an EVM event trigger
    async fn trigger_data(
        &self,
        address: layer_climb::prelude::Address,
        trigger_id: u64,
    ) -> Result<Vec<u8>> {
        let contract = SimpleTrigger::new(address.try_into()?, self);

        Ok(contract.getTrigger(trigger_id).call().await?.data.to_vec())
    }
}

mod example_trigger {
    use alloy_sol_macro::sol;
    pub use ISimpleTrigger::TriggerInfo;
    pub use SimpleTrigger::NewTrigger;

    sol!(
        #[allow(missing_docs)]
        #[sol(rpc)]
        SimpleTrigger,
        "../../contracts/solidity/abi/SimpleTrigger.sol/SimpleTrigger.json"
    );
}

mod example_submit {
    use alloy_sol_types::sol;
    pub use ISimpleSubmit::DataWithId;

    sol!(
        #[allow(missing_docs)]
        #[sol(rpc)]
        ISimpleSubmit,
        "../../contracts/solidity/abi/ISimpleSubmit.sol/ISimpleSubmit.json"
    );
}
