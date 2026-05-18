use alloy_network::Ethereum;
use alloy_primitives::BlockHash;
use alloy_provider::Provider;

use anyhow::Context;
use example_helpers::bindings::world::{
    host::{self, LogLevel},
    warpdrive::{
        types::events::{TriggerDataCosmosContractEvent, TriggerDataEvmContractEvent},
        vectr::{
            input::{TriggerAction, TriggerData},
            output::WasmResponse,
        },
    },
    Guest,
};
use example_helpers::{
    export_layer_trigger_world,
    trigger::{decode_trigger_event, encode_trigger_output, ChainQuerierExt},
};
use warpdrive_wasi_utils::evm::new_evm_provider;

struct Component;

impl Guest for Component {
    fn run(trigger_action: TriggerAction) -> std::result::Result<Vec<WasmResponse>, String> {
        // layer_climb + warpdrive_wasi_utils::evm both use wstd transports
        // on wasm32-wasip2; this must run under wstd's reactor, not tokio.
        wstd::runtime::block_on(run_one(trigger_action))
            .map_err(|e| format!("{e:?}"))
            .map(|res| vec![res])
    }
}

async fn run_one(
    trigger_action: TriggerAction,
) -> std::result::Result<WasmResponse, anyhow::Error> {
    let (trigger_id, _) = decode_trigger_event(trigger_action.data.clone())?;

    let resp = match trigger_action.data {
        TriggerData::CosmosContractEvent(TriggerDataCosmosContractEvent {
            chain,
            contract_address,
            ..
        }) => {
            let chain_config = host::get_cosmos_chain_config(&chain)
                .ok_or(anyhow::anyhow!("cosmos chain config for {chain} not found"))?;

            layer_climb::querier::QueryClient::new(chain_config.into(), None)
                .await?
                .trigger_data(contract_address.into(), trigger_id)
                .await?
        }
        TriggerData::EvmContractEvent(TriggerDataEvmContractEvent { chain, log, .. }) => {
            let chain_config = host::get_evm_chain_config(&chain)
                .ok_or(anyhow::anyhow!("EVM chain config for {chain} not found"))?;

            let provider = new_evm_provider::<Ethereum>(
                chain_config
                    .http_endpoint
                    .context("http_endpoint is missing")?,
            );

            let block_timestamp = provider
                .get_block(BlockHash::from_slice(&log.block_hash).into())
                .await?
                .ok_or(anyhow::anyhow!("Unable to get block"))?
                .header
                .timestamp;

            if block_timestamp == 0 {
                return Err(anyhow::anyhow!("Block timestamp is zero, invalid block"));
            }

            host::log(
                LogLevel::Info,
                &format!("Block timestamp: {block_timestamp}"),
            );

            provider
                .trigger_data(log.address.into(), trigger_id)
                .await?
        }
        _ => {
            return Err(anyhow::anyhow!("expected cosmos contract event"));
        }
    };

    let service = host::get_service().service;
    Ok(encode_trigger_output(
        trigger_id,
        resp,
        service.manager,
        service.signature_kind,
    ))
}

export_layer_trigger_world!(Component);
