use warpdrive_types::StellarChainConfig;

use crate::context::AppContext;

use anyhow::Result;

#[derive(Clone)]
pub struct StellarMiddleware {}

impl StellarMiddleware {
    pub fn new() -> Result<Self> {
        Ok(Self {})
    }
}
