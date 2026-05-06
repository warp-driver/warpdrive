mod gas_oracle;
mod world;

use example_types::{KvStoreError, KvStoreResult};
use warpdrive_types::ChainKey;
use warpdrive_wasi_utils::impl_u128_conversions;
use world::{
    host,
    warpdrive::aggregator::input::AggregatorInput,
    warpdrive::aggregator::output::{
        AggregatorAction, CosmosAddress, CosmosSubmitAction, EvmSubmitAction, StellarSubmitAction,
        SubmitAction, U128,
    },
    warpdrive::types::chain::{AnyTxHash, EvmAddress, StellarAddress},
    Guest,
};

use crate::world::wasi::keyvalue::store;

impl_u128_conversions!(U128);

struct Component;

impl Guest for Component {
    fn process_input(_input: AggregatorInput) -> Result<Vec<AggregatorAction>, String> {
        let chain = host::config_var("chain").ok_or("chain config variable is required")?;
        let chain =
            AnyChainKey::from_host(&chain).ok_or(format!("no chain config for {}", chain))?;

        let service_handler_str = host::config_var("service_handler")
            .ok_or("service_handler config variable is required")?;

        let submit_action = match chain {
            AnyChainKey::Evm(chain) => {
                let address: alloy_primitives::Address = service_handler_str
                    .parse()
                    .map_err(|e| format!("Failed to parse service handler address: {e}"))?;

                // Get gas price from Etherscan if configured
                // will fail the entire operation if API key is configured but fetching fails
                let gas_price = gas_oracle::get_gas_price()?;

                SubmitAction::Evm(EvmSubmitAction {
                    chain: chain.to_string(),
                    address: EvmAddress {
                        raw_bytes: address.to_vec(),
                    },
                    gas_price: gas_price.map(|x| x.into()),
                })
            }
            AnyChainKey::Cosmos(chain) => {
                let address = layer_climb_address::CosmosAddr::new_str(&service_handler_str, None)
                    .map_err(|e| e.to_string())?;

                SubmitAction::Cosmos(CosmosSubmitAction {
                    chain: chain.to_string(),
                    address: CosmosAddress {
                        bech32_addr: address.to_string(),
                        prefix_len: address.prefix().len() as u32,
                    },
                    gas_price: None,
                })
            }
            AnyChainKey::Stellar(chain) => {
                let contract = stellar_strkey::Contract::from_string(&service_handler_str)
                    .map_err(|e| format!("invalid stellar service_handler: {e:?}"))?;
                SubmitAction::Stellar(StellarSubmitAction {
                    chain: chain.to_string(),
                    address: StellarAddress {
                        raw_bytes: contract.0.to_vec(),
                    },
                })
            }
        };

        // Sanity check that we can get the event id
        if host::get_event_id().iter().all(|x| *x == 0) {
            return Err("event id is all zeros".to_string());
        }

        Ok(vec![AggregatorAction::Submit(submit_action)])
    }

    fn handle_timer_callback(_input: AggregatorInput) -> Result<Vec<AggregatorAction>, String> {
        Ok(Vec::new())
    }

    fn handle_submit_callback(
        _input: AggregatorInput,
        tx_result: Result<AnyTxHash, String>,
    ) -> Result<(), String> {
        write_kv_value("submit-result", "completed", "true".as_bytes())
            .map_err(|e| e.to_string())?;
        write_kv_value(
            "submit-result",
            "success",
            if tx_result.is_ok() {
                "true".as_bytes()
            } else {
                "false".as_bytes()
            },
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }
}

fn write_kv_value(bucket_id: &str, key: &str, value: &[u8]) -> KvStoreResult<()> {
    let bucket = open_kv_bucket(bucket_id)?;
    bucket.set(key, value).map_err(|e| KvStoreError::WriteKey {
        bucket: bucket_id.to_string(),
        key: key.to_string(),
        reason: e.to_string(),
    })
}

fn open_kv_bucket(id: &str) -> KvStoreResult<store::Bucket> {
    store::open(id).map_err(|e| KvStoreError::BucketOpen {
        id: id.to_string(),
        reason: e.to_string(),
    })
}

enum AnyChainKey {
    Evm(ChainKey),
    Cosmos(ChainKey),
    Stellar(ChainKey),
}

impl AnyChainKey {
    pub fn from_host(chain: &str) -> Option<Self> {
        let parsed: ChainKey = chain.parse().ok()?;
        // Stellar has no host-side `get_stellar_chain_config` shim today;
        // dispatch on the namespace prefix instead. EVM/Cosmos still go
        // through their host config lookups so we get an early "no config"
        // error if the chain isn't actually wired.
        if parsed.namespace.as_str() == warpdrive_types::ChainKeyNamespace::STELLAR {
            return Some(AnyChainKey::Stellar(parsed));
        }
        #[allow(clippy::manual_map)]
        match host::get_evm_chain_config(chain) {
            Some(_) => Some(AnyChainKey::Evm(parsed)),
            None => match host::get_cosmos_chain_config(chain) {
                Some(_) => Some(AnyChainKey::Cosmos(parsed)),
                None => None,
            },
        }
    }
}

export_aggregator_world!(Component);
