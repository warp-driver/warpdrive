use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use warpdrive_types::{
    ChainConfigs, CosmosChainConfigBuilder, EvmChainConfigBuilder, StellarChainConfigBuilder,
};

pub fn mock_chain_configs() -> Arc<RwLock<ChainConfigs>> {
    Arc::new(RwLock::new(ChainConfigs {
        evm: vec![(
            "31337".try_into().unwrap(),
            EvmChainConfigBuilder {
                ws_endpoints: vec!["ws://localhost:8546".to_string()],
                http_endpoint: Some("http://localhost:8545".to_string()),
                faucet_endpoint: None,
                ws_priority_endpoint_index: None,
            },
        )]
        .into_iter()
        .collect(),
        cosmos: vec![(
            "cosmos".try_into().unwrap(),
            CosmosChainConfigBuilder {
                rpc_endpoint: Some("http://localhost:26657".to_string()),
                grpc_endpoint: Some("http://localhost:9090".to_string()),
                bech32_prefix: "cosmos".to_string(),
                gas_denom: "ustake".to_string(),
                gas_price: 0.025,
                faucet_endpoint: None,
            },
        )]
        .into_iter()
        .collect(),
        stellar: vec![(
            "stellar".try_into().unwrap(),
            StellarChainConfigBuilder {
                chain_poll_interval_ms: 1000,
                rpc_url: "http://localhost:8000".to_string(),
                friendbot_url: None,
            },
        )]
        .into_iter()
        .collect(),
        dev: BTreeMap::new(),
    }))
}
