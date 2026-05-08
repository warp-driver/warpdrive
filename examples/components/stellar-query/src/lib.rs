//! Stellar (Soroban) query component — host-side plumbing only.
//!
//! Mirrors the shape of the `cosmos-query` component but stops short
//! of actually querying the chain: a wasip2-compatible Soroban RPC
//! client for components doesn't exist yet, so the component logs the
//! resolved chain config and `unimplemented!()`s.
//!
//! See issue #5 for the full picture; this PR delivers steps 1-3
//! (WIT host fn + component scaffold + e2e wiring) and leaves the
//! actual query work for a follow-up that integrates a Soroban
//! client compiled to wasip2.

use anyhow::anyhow;
use example_helpers::bindings::world::{host, Guest, TriggerAction, WasmResponse};
use example_helpers::trigger::encode_trigger_output;
use example_helpers::{export_layer_trigger_world, trigger::decode_trigger_event};
use example_types::{StellarQueryRequest, StellarQueryResponse};
use wasi_soroban_rs::{Env, EnvConfigs};

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
    let req: StellarQueryRequest = serde_json::from_slice(&req)?;

    // Pull the chain key out of the request. All current
    // `StellarQueryRequest` variants carry a `chain` string; this
    // pattern stays correct as new variants are added because they
    // share the same convention.
    let chain_key = match &req {
        StellarQueryRequest::Balance { chain, .. } => chain.clone(),
    };

    // Resolve the chain config via the new host function. This is
    // the proof-of-life for issue #5's primary deliverable: WIT
    // hosts can hand back a Stellar config to a component that
    // asks for it.
    let chain_config = host::get_stellar_chain_config(&chain_key)
        .ok_or_else(|| anyhow!("chain config for {chain_key} not found"))?;

    // Log everything we got. The runtime forwards `host::log` to the
    // WarpDrive node's tracing layer, so the e2e test can grep for
    // these lines to verify the round-trip works.
    host::log(
        host::LogLevel::Info,
        &format!(
            "stellar-query: chain_id={} rpc_url={} network_passphrase={}",
            chain_config.chain_id, chain_config.rpc_url, chain_config.network_passphrase,
        ),
    );

    let env = Env::new(EnvConfigs {
        rpc_url: chain_config.rpc_url,
        network_passphrase: chain_config.network_passphrase,
    })?;

    // Actually do the queries or whatever
    let resp = match req {
        StellarQueryRequest::Balance { account_id, .. } => {
            let account_entry = env.get_account(&account_id).await?;
            StellarQueryResponse::Balance(account_entry.balance)
        }
    };

    let output = serde_json::to_vec(&resp)?;
    Ok(encode_trigger_output(
        trigger_id,
        output,
        host::get_service().service.manager,
    ))
}

export_layer_trigger_world!(Component);
