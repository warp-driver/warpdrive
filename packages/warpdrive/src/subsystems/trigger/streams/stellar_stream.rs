pub mod channels;
pub mod client;
pub mod controller;
pub mod filters;
pub mod poller;

use futures::{Stream, StreamExt};
use std::pin::Pin;
use tokio_stream::wrappers::UnboundedReceiverStream;
use utils::telemetry::TriggerMetrics;
use warpdrive_types::ChainKey;

use crate::subsystems::trigger::{error::TriggerError, streams::StreamTriggers};

pub async fn start_stellar_event_stream(
    chain: ChainKey,
    event_stream: UnboundedReceiverStream<(wasi_stellar_rpc_client::Event, Vec<filters::StellarRpcId>)>,
    _metrics: TriggerMetrics,
) -> Result<Pin<Box<dyn Stream<Item = Result<StreamTriggers, TriggerError>> + Send>>, TriggerError>
{
    let chain = chain.clone();

    let event_stream = Box::pin(event_stream.filter_map(move |(event, rpc_ids)| {
        let chain = chain.clone();
        async move {
            match event.tx_hash {
                Some(tx_hash) => {
                    let wasi_stellar_rpc_client::Event {
                        contract_id,
                        event_type,
                        ledger,
                        ledger_closed_at,
                        id,
                        operation_index,
                        transaction_index,
                        tx_hash: _,
                        topic,
                        value,
                        #[allow(deprecated)]
                            is_successful_contract_call: _,
                    } = event;

                    Some(Ok(StreamTriggers::StellarEvent {
                        chain,
                        contract_id,
                        event_type,
                        ledger,
                        ledger_closed_at,
                        event_id: id,
                        operation_index,
                        transaction_index,
                        tx_hash,
                        topic_segments: topic,
                        value,
                        rpc_ids,
                    }))
                }
                None => {
                    tracing::warn!("Received event without transaction hash: {:?}", event);
                    None
                }
            }
        }
    }));

    Ok(event_stream)
}

pub async fn start_stellar_ledger_stream(
    chain: ChainKey,
    ledger_stream: UnboundedReceiverStream<u32>,
    _metrics: TriggerMetrics,
) -> Result<Pin<Box<dyn Stream<Item = Result<StreamTriggers, TriggerError>> + Send>>, TriggerError>
{
    let ledger_stream = Box::pin(ledger_stream.map(move |ledger| {
        Ok(StreamTriggers::StellarLedgerSequence {
            chain: chain.clone(),
            ledger,
        })
    }));

    Ok(ledger_stream)
}
