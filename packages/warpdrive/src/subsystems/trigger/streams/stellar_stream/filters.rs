use std::collections::HashMap;

use slotmap::{new_key_type, SlotMap};
use stellar_xdr::curr::{Limits, WriteXdr};
use utils::error::{StellarClientError, StellarClientResult};
use warpdrive_types::StellarTopicSegment;

// TODO - optimize for batching
// Stellar docs say it allows up to 5 contract ids and 5 topics per RPC call
// ref: https://developers.stellar.org/docs/data/apis/rpc/api-reference/methods/getEvents
#[derive(Default, Debug, Clone)]
pub struct EventFilters {
    // We need stable ids for each filter so we can map it to a distinct Trigger
    list: SlotMap<StellarRpcId, StellarEventFilter>,
    // However, we don't need to actually request each filter separately, we just need to map it back to each id
    dedupe_list: HashMap<StellarEventFilter, Vec<StellarRpcId>>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct StellarEventFilter {
    pub contract_id: String,
    pub topic_segments_xdr_base64: Vec<String>,
}

impl StellarEventFilter {
    pub fn new(
        contract_id: String,
        topic_segments: Vec<StellarTopicSegment>,
    ) -> StellarClientResult<Self> {
        fn encode_topic_segment(
            topic_segment: &StellarTopicSegment,
        ) -> StellarClientResult<String> {
            match topic_segment {
                StellarTopicSegment::Exact(scval) => scval
                    .to_xdr_base64(Limits::none())
                    .map_err(StellarClientError::from),
                StellarTopicSegment::Wildcard => Ok("*".to_string()),
                StellarTopicSegment::RestWildcard => {
                    Err(StellarClientError::InvalidWildcard("**".to_string()))
                }
            }
        }

        let topic_segments_xdr_base64 = topic_segments
            .iter()
            .map(encode_topic_segment)
            .collect::<StellarClientResult<Vec<_>>>()?;

        Ok(Self {
            contract_id,
            topic_segments_xdr_base64,
        })
    }
}

impl EventFilters {
    pub fn add_filter(&mut self, filter: StellarEventFilter) -> StellarClientResult<StellarRpcId> {
        let id = self.list.insert(filter.clone());
        self.dedupe_list.entry(filter).or_default().push(id);

        Ok(id)
    }

    pub fn get_rpc_ids_for_filter(&self, filter: &StellarEventFilter) -> Vec<StellarRpcId> {
        self.dedupe_list
            .get(filter)
            .cloned()
            .unwrap_or_else(Vec::new)
    }
    pub fn get_all_filters_with_ids(&self) -> HashMap<StellarEventFilter, Vec<StellarRpcId>> {
        self.dedupe_list.clone()
    }

    pub fn get_filter(&self, id: StellarRpcId) -> Option<&StellarEventFilter> {
        self.list.get(id)
    }

    pub fn remove_filter(&mut self, filter: &StellarEventFilter) {
        if let Some(ids) = self.dedupe_list.remove(filter) {
            for id in ids {
                self.list.remove(id);
            }
        }
    }

    pub fn remove_filter_by_id(&mut self, id: StellarRpcId) {
        if let Some(filter) = self.list.remove(id) {
            if let Some(ids) = self.dedupe_list.get_mut(&filter) {
                ids.retain(|&existing_id| existing_id != id);
                if ids.is_empty() {
                    self.dedupe_list.remove(&filter);
                }
            }
        }
    }
}

new_key_type! {
    pub struct StellarRpcId;
}
