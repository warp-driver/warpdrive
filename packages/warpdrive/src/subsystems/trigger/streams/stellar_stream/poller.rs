//! Polls the chain for events

use std::time::Duration;

use crate::subsystems::trigger::streams::stellar_stream::controller::StellarStreamController;

pub async fn start_stellar_event_poller(
    controller: StellarStreamController,
    sender: tokio::sync::mpsc::UnboundedSender<stellar_rpc_client::Event>,
) {
    let interval = Duration::from_millis(controller.client.config.chain_poll_interval_ms);

    loop {
        match controller.client.fetch_next_events().await {
            Ok(events) => {
                if !events.is_empty() {
                    for event in events {
                        if let Err(err) = sender.send(event) {
                            tracing::error!("Failed to send events: {err}");
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!("chain poll error: {err}");
            }
        }

        tokio::time::sleep(interval).await;
    }
}

pub async fn start_stellar_ledger_poller(
    controller: StellarStreamController,
    sender: tokio::sync::mpsc::UnboundedSender<u32>,
) {
    let interval = Duration::from_millis(controller.client.config.chain_poll_interval_ms);

    let mut last_ledger = 0;

    loop {
        if !controller.is_ledger_polling_enabled() {
            tokio::time::sleep(interval).await;
            continue;
        }

        match controller.client.fetch_ledger_sequence().await {
            Ok(ledger) => {
                if ledger > last_ledger {
                    last_ledger = ledger;
                    if let Err(err) = sender.send(ledger) {
                        tracing::error!("Failed to send ledger sequence: {err}");
                    }
                } else {
                    // No new ledger, skip sending
                    tokio::time::sleep(interval).await;
                    continue;
                }
            }
            Err(err) => {
                tracing::warn!("chain poll error: {err}");
            }
        }
    }
}
