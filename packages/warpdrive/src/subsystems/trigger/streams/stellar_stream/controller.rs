use utils::error::StellarClientResult;
use warpdrive_types::StellarChainConfig;

use super::client::StellarStreamClient;
use std::sync::{atomic::AtomicBool, Arc};

#[derive(Clone)]
pub struct StellarStreamController {
    ledger_polling_enabled: Arc<AtomicBool>,
    pub client: StellarStreamClient,
}

impl StellarStreamController {
    pub fn new(config: StellarChainConfig) -> StellarClientResult<Self> {
        let client = StellarStreamClient::new(config)?;

        Ok(Self {
            ledger_polling_enabled: Arc::new(AtomicBool::new(false)),
            client,
        })
    }

    pub fn is_ledger_polling_enabled(&self) -> bool {
        self.ledger_polling_enabled
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn enable_ledger_polling(&self) {
        self.ledger_polling_enabled
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn disable_ledger_polling(&self) {
        self.ledger_polling_enabled
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
}
