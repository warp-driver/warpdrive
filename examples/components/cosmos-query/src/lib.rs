use anyhow::anyhow;
use example_helpers::bindings::world::{host, Guest, TriggerAction, WasmResponse};
use example_helpers::{
    export_layer_trigger_world,
    trigger::{decode_trigger_event, encode_trigger_output},
};
use example_types::{CosmosQueryRequest, CosmosQueryResponse};

struct Component;

impl Guest for Component {
    fn run(trigger_action: TriggerAction) -> std::result::Result<Vec<WasmResponse>, String> {
        run_one(trigger_action)
            .map_err(|e: anyhow::Error| format!("{e:?}"))
            .map(|res| vec![res])
    }
}

#[tokio::main(flavor = "current_thread")]
async fn run_one(
    trigger_action: TriggerAction,
) -> std::result::Result<WasmResponse, anyhow::Error> {
    let (trigger_id, req) = decode_trigger_event(trigger_action.data)?;

    let req: CosmosQueryRequest = serde_json::from_slice(&req).map_err(|e| anyhow!("{:?}", e))?;

    let resp = match req {
        CosmosQueryRequest::BlockHeight { chain } => {
            let chain_config = host::get_cosmos_chain_config(&chain)
                .ok_or(anyhow!("chain config for {chain} not found"))?;

            let querier = layer_climb::querier::QueryClient::new(chain_config.into(), None).await?;

            querier
                .block_height()
                .await
                .map(CosmosQueryResponse::BlockHeight)
        }

        CosmosQueryRequest::Balance { chain, address } => {
            let chain_config = host::get_cosmos_chain_config(&chain)
                .ok_or(anyhow!("chain config for {chain} not found"))?;

            let querier = layer_climb::querier::QueryClient::new(chain_config.into(), None).await?;

            querier
                .balance(address, None)
                .await
                .map(|amount| match amount {
                    Some(amount) => CosmosQueryResponse::Balance(amount.to_string()),
                    None => CosmosQueryResponse::Balance("0".to_string()),
                })
        }
    }?;

    let output = serde_json::to_vec(&resp)?;
    let service = host::get_service().service;
    Ok(encode_trigger_output(
        trigger_id,
        output,
        service.manager,
        service.signature_kind,
    ))
}

export_layer_trigger_world!(Component);
