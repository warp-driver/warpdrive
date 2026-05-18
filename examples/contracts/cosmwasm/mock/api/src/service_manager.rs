use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Empty, Uint256};
use layer_climb_address::EvmAddr;
use warpdrive_types::contracts::cosmwasm::service_manager::{
    ServiceManagerExecuteMessages, ServiceManagerQueryMessages,
};

pub type InstantiateMsg = Empty;

#[cw_serde]
#[schemaifier(mute_warnings)]
pub enum ExecuteMsg {
    /// Mock contracts get superpowers
    SetSigningKey {
        // wire name kept as `operator` for compatibility with the pre-rename
        // mock service-manager contract bundled in `lay3rlabs/cw-middleware`.
        #[serde(rename = "operator")]
        vector: EvmAddr,
        signing_key: EvmAddr,
        weight: Uint256,
    },
    #[serde(untagged)]
    Wavs(ServiceManagerExecuteMessages),
}

#[cw_serde]
#[schemaifier(mute_warnings)]
pub enum QueryMsg {
    #[serde(untagged)]
    Wavs(ServiceManagerQueryMessages),
}
