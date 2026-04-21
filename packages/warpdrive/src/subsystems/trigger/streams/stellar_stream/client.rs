use std::sync::{
    atomic::{AtomicBool, AtomicU32},
    Arc,
};

use super::filters::EventFilters;
use stellar_rpc_client::{Event, EventStart, EventType};
use utils::error::{StellarClientError, StellarClientResult};
use warpdrive_types::StellarChainConfig;

#[derive(Clone)]
pub struct StellarStreamClient {
    pub config: StellarChainConfig,
    event_filters: Arc<std::sync::RwLock<EventFilters>>,
    inner: stellar_rpc_client::Client,
    http_client: reqwest::Client,
    event_last_ledger: Arc<AtomicU32>,
    event_ledger_once: Arc<AtomicBool>,
    friendbot_url: Option<String>,
}

impl StellarStreamClient {
    pub fn new(config: StellarChainConfig) -> StellarClientResult<Self> {
        let inner = stellar_rpc_client::Client::new(&config.rpc_url)?;
        let event_last_ledger = Arc::new(AtomicU32::new(0));
        let event_ledger_once = Arc::new(AtomicBool::new(false));
        let friendbot_url = config.friendbot_url.clone();

        Ok(Self {
            config,
            event_filters: Arc::new(std::sync::RwLock::new(EventFilters::default())),
            inner,
            event_last_ledger,
            event_ledger_once,
            http_client: reqwest::Client::new(),
            friendbot_url,
        })
    }

    pub fn clone_event_filters(&self) -> EventFilters {
        self.event_filters.read().unwrap().clone()
    }

    pub fn update_event_filters(&self, f: impl FnOnce(&mut EventFilters)) {
        let mut filters = self.event_filters.write().unwrap();
        f(&mut filters);
    }

    pub async fn fetch_next_events(&self) -> StellarClientResult<Vec<Event>> {
        let curr_ledger = self.inner.get_latest_ledger().await?.sequence;

        if !self
            .event_ledger_once
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            self.event_last_ledger
                .store(curr_ledger, std::sync::atomic::Ordering::Relaxed);
            self.event_ledger_once
                .store(true, std::sync::atomic::Ordering::Relaxed);

            Ok(Vec::new())
        } else {
            let next_ledger = self
                .event_last_ledger
                .load(std::sync::atomic::Ordering::Relaxed)
                + 1;

            if curr_ledger < next_ledger {
                return Ok(Vec::new());
            }

            let range = EventStart::ledger_range(next_ledger, curr_ledger + 1).map_err(|_| {
                StellarClientError::InvalidLedgerRange {
                    start: next_ledger,
                    end: curr_ledger + 1,
                }
            })?;
            let event_type = EventType::Contract;

            let mut all_events = Vec::new();

            let rpc_list = {
                let lock = self.event_filters.read().unwrap();
                lock.rpc_list.clone()
            };

            for filter in rpc_list {
                let resp = self
                    .inner
                    .get_events(
                        range.clone(),
                        Some(event_type),
                        &filter.contract_ids,
                        &filter.topics_xdr_base64,
                        None,
                    )
                    .await?;

                all_events.extend(resp.events);
            }

            self.event_last_ledger
                .store(curr_ledger, std::sync::atomic::Ordering::Relaxed);

            Ok(all_events)
        }
    }

    pub async fn fetch_ledger_sequence(&self) -> StellarClientResult<u32> {
        Ok(self.inner.get_latest_ledger().await?.sequence)
    }

    pub async fn check_friendbot_health(&self) -> Option<bool> {
        let friendbot_url = self.friendbot_url.as_ref()?;
        let resp = self.http_client.get(friendbot_url).send().await;

        match resp {
            Ok(r) if r.status().is_success() => Some(true),
            Ok(r) if r.status().as_u16() == 400 => Some(true), // Friendbot returns 400 if it's up but the request is invalid (e.g., missing "addr" param)
            _ => Some(false),
        }
    }
}
