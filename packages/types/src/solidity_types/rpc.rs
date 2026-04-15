use alloy_provider::DynProvider;

mod service_manager {
    alloy_sol_macro::sol!(
        #[allow(missing_docs)]
        #[sol(rpc)]
        #[derive(Debug)]
        IWarpDriveServiceManager,
        "./src/contracts/solidity/abi/IWarpDriveServiceManager.sol/IWarpDriveServiceManager.json"
    );
}

mod service_handler {
    alloy_sol_macro::sol!(
        #[allow(missing_docs)]
        #[sol(rpc)]
        #[derive(serde::Deserialize, serde::Serialize, Debug, PartialEq, Eq)]
        IWarpDriveServiceHandler,
        "./src/contracts/solidity/abi/IWarpDriveServiceHandler.sol/IWarpDriveServiceHandler.json"
    );
}

pub use service_handler::{
    IWarpDriveServiceHandler, IWarpDriveServiceHandler::Envelope,
    IWarpDriveServiceHandler::SignatureData,
};
pub use service_manager::IWarpDriveServiceManager;
// yup, the service handler interface as seen by the service manager is a different service handler interface
// even though it's literally a direct import of the same file
pub use service_manager::{
    IWarpDriveServiceHandler::Envelope as ServiceManagerEnvelope,
    IWarpDriveServiceHandler::SignatureData as ServiceManagerSignatureData,
};

pub type IWarpDriveServiceHandlerSigningT =
    IWarpDriveServiceHandler::IWarpDriveServiceHandlerInstance<DynProvider>;

pub type IWarpDriveServiceHandlerQueryT =
    IWarpDriveServiceHandler::IWarpDriveServiceHandlerInstance<DynProvider>;

pub type IWarpDriveServiceManagerSigningT =
    IWarpDriveServiceManager::IWarpDriveServiceManagerInstance<DynProvider>;

pub type IWarpDriveServiceManagerQueryT =
    IWarpDriveServiceManager::IWarpDriveServiceManagerInstance<DynProvider>;

pub type ServiceManagerError = IWarpDriveServiceManager::IWarpDriveServiceManagerErrors;

pub fn decode_service_manager_error(err: alloy_contract::Error) -> Option<ServiceManagerError> {
    err.as_decoded_interface_error::<ServiceManagerError>()
}
