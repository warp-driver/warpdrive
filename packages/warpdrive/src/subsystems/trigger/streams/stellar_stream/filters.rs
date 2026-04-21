use std::collections::HashMap;

use serde::Serialize;
use stellar_xdr::curr::{Limits, WriteXdr};
use utils::error::{StellarClientError, StellarClientResult};
use warpdrive_types::StellarTopicSegment;

// Reference: https://developers.stellar.org/docs/data/apis/rpc/api-reference/methods/getEvents

#[derive(Default, Debug, Clone)]
pub struct EventFilters {
    // ContractId -> List of filters for that contract
    // Keep this as a nice human-readable format for users to add filters in a more intuitive way
    pub nice_list: HashMap<String, Vec<StellarTopic>>,
    // Each RPC call only allows up to 5 contractIds and 5 topics at a time
    // so we need to maintain a separate list that is optimized for that shape
    pub rpc_list: Vec<RpcEventFilter>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StellarTopic(Vec<StellarTopicSegment>); // max 4 segments per topic

impl TryFrom<Vec<StellarTopicSegment>> for StellarTopic {
    type Error = StellarClientError;

    fn try_from(topic: Vec<StellarTopicSegment>) -> StellarClientResult<Self> {
        if topic.len() > 4 {
            return Err(StellarClientError::TooManyTopicSegments(topic.len()));
        }

        if let Some(rest_index) = topic
            .iter()
            .position(|segment| matches!(segment, StellarTopicSegment::RestWildcard))
        {
            if rest_index != topic.len() - 1 {
                return Err(StellarClientError::RestWildcardMustBeLast);
            }
        }

        Ok(Self(topic))
    }
}

#[derive(Debug, Clone)]
pub struct RpcEventFilter {
    // Enforced to be maximum 5 contract ids and 5 topics to fit the RPC requirements
    pub contract_ids: Vec<String>,
    pub topics_xdr_base64: Vec<Vec<String>>,
}

impl RpcEventFilter {
    pub fn new() -> Self {
        Self {
            contract_ids: Vec::new(),
            topics_xdr_base64: Vec::new(),
        }
    }
}

impl EventFilters {
    const MAX_CONTRACT_IDS_PER_RPC: usize = 5;
    const MAX_TOPICS_PER_RPC: usize = 5;

    pub fn add_filter<T>(
        &mut self,
        contract_id: String,
        topic_segments: T,
    ) -> StellarClientResult<()>
    where
        T: TryInto<StellarTopic, Error = StellarClientError>,
    {
        let topic_segments = topic_segments.try_into()?;

        let is_duplicate = self.nice_list.get(&contract_id).map_or(false, |filters| {
            filters.iter().any(|f| f.0 == topic_segments.0)
        });

        if is_duplicate {
            return Ok(()); // No need to add duplicate filters
        }

        self.nice_list
            .entry(contract_id)
            .or_insert_with(Vec::new)
            .push(topic_segments);

        self.rebuild_rpc()
    }

    pub fn remove_filter<T>(
        &mut self,
        contract_id: String,
        topic_segments: T,
    ) -> StellarClientResult<()>
    where
        T: TryInto<StellarTopic, Error = StellarClientError>,
    {
        if let Some(filters) = self.nice_list.get_mut(&contract_id) {
            let topic_segments_to_remove = topic_segments.try_into()?;
            filters.retain(|f| f.0 != topic_segments_to_remove.0);

            if filters.is_empty() {
                self.nice_list.remove(&contract_id);
            }

            self.rebuild_rpc()
        } else {
            Ok(())
        }
    }

    // Rebuilds the rpc_list based on the current human_readable filters
    // Each RPC call can have up to 5 contract ids and 5 topics
    fn rebuild_rpc(&mut self) -> StellarClientResult<()> {
        self.rpc_list.clear();

        let mut current_rpc_filter = RpcEventFilter {
            contract_ids: Vec::new(),
            topics_xdr_base64: Vec::new(),
        };

        let mut contracts = self.nice_list.iter().collect::<Vec<_>>();
        contracts.sort_by(|(a, _), (b, _)| a.cmp(b));

        for (contract_id, filters) in contracts {
            let encoded_topics = filters.iter().try_fold(Vec::new(), |mut acc, filter| {
                acc.extend(Self::encode_topic_segments(filter)?);
                Ok::<_, StellarClientError>(acc)
            })?;

            let contract_fits = current_rpc_filter.contract_ids.contains(contract_id)
                || current_rpc_filter.contract_ids.len() < Self::MAX_CONTRACT_IDS_PER_RPC;
            let topics_fit = current_rpc_filter.topics_xdr_base64.len() + encoded_topics.len()
                <= Self::MAX_TOPICS_PER_RPC;

            // A contract's filters stay together within a single RPC filter. If adding this
            // contract would overflow either RPC limit, flush the current batch first.
            if !current_rpc_filter.contract_ids.is_empty() && (!contract_fits || !topics_fit) {
                self.rpc_list.push(current_rpc_filter);
                current_rpc_filter = RpcEventFilter {
                    contract_ids: Vec::new(),
                    topics_xdr_base64: Vec::new(),
                };
            }

            current_rpc_filter.contract_ids.push(contract_id.clone());
            current_rpc_filter.topics_xdr_base64.extend(encoded_topics);
        }

        if !current_rpc_filter.contract_ids.is_empty() {
            self.rpc_list.push(current_rpc_filter);
        }

        Ok(())
    }

    fn encode_topic_segments(
        topic_segments: &StellarTopic,
    ) -> StellarClientResult<Vec<Vec<String>>> {
        let used_segments = topic_segments.0.iter().cloned().collect::<Vec<_>>();

        let rest_index = used_segments
            .iter()
            .position(|segment| matches!(segment, StellarTopicSegment::RestWildcard));

        let exact_prefix = match rest_index {
            Some(index) => &used_segments[..index],
            None => used_segments.as_slice(),
        };

        let encoded_prefix = exact_prefix
            .iter()
            .map(Self::encode_topic_segment)
            .collect::<StellarClientResult<Vec<_>>>()?;

        if rest_index.is_some() {
            let remaining = 4usize.saturating_sub(encoded_prefix.len());
            let expanded = (0..=remaining)
                .map(|wildcards| {
                    let mut filter = encoded_prefix.clone();
                    filter.extend(std::iter::repeat_n("*".to_string(), wildcards));
                    filter
                })
                .collect();
            Ok(expanded)
        } else {
            Ok(vec![encoded_prefix])
        }
    }

    fn encode_topic_segment(topic_segment: &StellarTopicSegment) -> StellarClientResult<String> {
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
}
