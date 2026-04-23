use stellar_rpc_client::Event;
use tokio::sync::mpsc::UnboundedSender;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::subsystems::trigger::streams::stellar_stream::filters::StellarRpcId;

pub struct StellarChannelReceivers {
    pub ledger_rx: UnboundedReceiverStream<u32>,
    pub event_rx: UnboundedReceiverStream<(Event, Vec<StellarRpcId>)>,
}

pub struct StellarChannelSenders {
    pub ledger_tx: UnboundedSender<u32>,
    pub event_tx: UnboundedSender<(Event, Vec<StellarRpcId>)>,
}

pub struct StellarChannels {
    pub receivers: StellarChannelReceivers,
    pub senders: StellarChannelSenders,
}

impl StellarChannels {
    pub fn new() -> Self {
        let (ledger_tx, ledger_rx) = tokio::sync::mpsc::unbounded_channel();
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();

        Self {
            receivers: StellarChannelReceivers {
                ledger_rx: UnboundedReceiverStream::new(ledger_rx),
                event_rx: UnboundedReceiverStream::new(event_rx),
            },
            senders: StellarChannelSenders {
                ledger_tx,
                event_tx,
            },
        }
    }
}
