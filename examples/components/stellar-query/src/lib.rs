//! Stellar (Soroban) query component.
//!
//! Resolves the chain config for a Stellar service manager and uses
//! `wasi-soroban-rs` + `warpdrive-client` to issue read-only queries
//! against the Soroban RPC. Currently supports:
//!   - `Balance`: account balance via `Env::get_account`.
//!   - `RequiredWeight`: total required signer weight, walking
//!     `project_root` → secp256k1 security contract.

use anyhow::{anyhow, bail};
use example_helpers::bindings::world::warpdrive::types::service::ServiceManager;
use example_helpers::bindings::world::{host, Guest, TriggerAction, WasmResponse};
use example_helpers::trigger::encode_trigger_output;
use example_helpers::{export_layer_trigger_world, trigger::decode_trigger_event};
use example_types::{StellarQueryRequest, StellarQueryResponse};
use warpdrive_client::project_root::{ProjectRootClient, VerificationType};
use warpdrive_client::secp256k1_security::Secp256k1SecurityClient;
use wasi_soroban_rs::{Account, ClientContractConfigs, ContractId, Env, EnvConfigs, Signer};

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

    let service_manager = host::get_service().service.manager;

    // Pull the chain key out of the request. All current
    // `StellarQueryRequest` variants carry a `chain` string; this
    // pattern stays correct as new variants are added because they
    // share the same convention.
    let (chain_key, project_root) = match &service_manager {
        ServiceManager::Stellar(m) => (m.chain.clone(), m.address.clone()),
        _ => bail!("Only supports stellar"),
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

    let resp = match req {
        StellarQueryRequest::Balance { account_id } => {
            let account_entry = env.get_account(&account_id).await?;
            StellarQueryResponse::Balance(account_entry.balance)
        }
        StellarQueryRequest::RequiredWeight {} => {
            // Read-only Soroban simulations require a source account
            // on the tx body, but the signature is never validated.
            // Mirrors the host-side `STELLAR_QUERY_KEY` pattern.
            let account = Account::single(Signer::from(&[1u8; 32]));

            let project_root_id: [u8; 32] = project_root
                .raw_bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow!("project_root address is not 32 bytes"))?;

            let project_root_client = ProjectRootClient::new(ClientContractConfigs {
                contract_id: ContractId(project_root_id),
                env: env.clone(),
                source_account: account.clone(),
            });

            // Only the secp256k1 (Ethereum-style) verification path is
            // supported here; reject anything else explicitly.
            match project_root_client.verification_type().await? {
                VerificationType::Ethereum => {}
                other => bail!("only secp256k1 verification is supported, got {other:?}"),
            }

            let security_contract = project_root_client.security_contract().await?;
            let security_client = Secp256k1SecurityClient::new(ClientContractConfigs {
                contract_id: security_contract,
                env,
                source_account: account,
            });

            let weight = security_client.required_weight().await?;
            StellarQueryResponse::RequiredWeight(weight)
        }
    };

    let output = serde_json::to_vec(&resp)?;
    Ok(encode_trigger_output(trigger_id, output, service_manager))
}

export_layer_trigger_world!(Component);
