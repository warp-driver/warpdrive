use alloy_primitives::Address;
use warpdrive_types::{
    IWarpDriveServiceHandler, IWarpDriveServiceHandlerQueryT, IWarpDriveServiceHandlerSigningT,
    IWarpDriveServiceManager, IWarpDriveServiceManagerQueryT, IWarpDriveServiceManagerSigningT,
};

use super::{EvmQueryClient, EvmSigningClient};

impl EvmSigningClient {
    pub fn service_handler(&self, address: Address) -> IWarpDriveServiceHandlerSigningT {
        IWarpDriveServiceHandler::new(address, self.provider.clone())
    }

    pub fn service_manager(&self, address: Address) -> IWarpDriveServiceManagerSigningT {
        IWarpDriveServiceManager::new(address, self.provider.clone())
    }
}

impl EvmQueryClient {
    pub fn service_handler(&self, address: Address) -> IWarpDriveServiceHandlerQueryT {
        IWarpDriveServiceHandler::new(address, self.provider.clone())
    }

    pub fn service_manager(&self, address: Address) -> IWarpDriveServiceManagerQueryT {
        IWarpDriveServiceManager::new(address, self.provider.clone())
    }
}
