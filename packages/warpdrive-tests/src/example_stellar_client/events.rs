//! Shared poll/parse helpers for the `Verified` and `Triggered` events
//! published by both `mock_submit_eth` and `mock_submit_xlm` after a
//! successful `verify_*`. Each handler decodes `trigger_id` out of its
//! own envelope payload (ABI `DataWithId` for eth, XDR `MessageWithId`
//! for xlm), but the event shape on Soroban RPC is identical for both
//! handlers — same `Verified` (topic `Symbol("verified")` + 20-byte
//! event_id) and same `Triggered` (`Symbol("triggered"), U64(trigger_id)`
//! topics + bare `ScVal::Bytes(20)` body) — so the poll/parse code
//! lives once here and both client structs delegate.

use std::time::Duration;

use anyhow::{anyhow, Result};
use stellar_xdr::curr::{Limits, ReadXdr, ScSymbol, ScVal, WriteXdr};

/// Poll Soroban RPC for a `Triggered` event with the given `trigger_id`
/// topic and return the embedded event_id as a 40-char hex string.
pub(super) async fn wait_for_triggered_event_id(
    rpc_url: &str,
    contract_id: &str,
    trigger_id: u64,
    start_ledger: u32,
    timeout: Duration,
) -> Result<String> {
    let rpc = wasi_stellar_rpc_client::Client::new(rpc_url)
        .map_err(|e| anyhow!("failed to construct stellar rpc client: {e:?}"))?;

    // Soroban RPC wants each topic segment as an XDR-base64 ScVal.
    // Filter on [Symbol("triggered"), U64(trigger_id)] so we only see
    // our test's specific trigger.
    let triggered_symbol_xdr = ScVal::Symbol(ScSymbol(b"triggered".try_into().unwrap()))
        .to_xdr_base64(Limits::none())
        .map_err(|e| anyhow!("encode triggered topic symbol: {e:?}"))?;
    let trigger_id_xdr = ScVal::U64(trigger_id)
        .to_xdr_base64(Limits::none())
        .map_err(|e| anyhow!("encode trigger_id topic: {e:?}"))?;
    let topic_filter = vec![vec![triggered_symbol_xdr, trigger_id_xdr]];

    tokio::time::timeout(timeout, async {
        let mut start = wasi_stellar_rpc_client::EventStart::Ledger(start_ledger);
        loop {
            let resp = rpc
                .get_events(
                    start.clone(),
                    Some(wasi_stellar_rpc_client::EventType::Contract),
                    &[contract_id.to_string()],
                    &topic_filter,
                    None,
                )
                .await;
            match resp {
                Ok(resp) => {
                    for event in resp.events {
                        if let Some(hex) = extract_triggered_event_id(&event, trigger_id) {
                            return Ok(hex);
                        }
                    }
                    if !resp.cursor.is_empty() {
                        start = wasi_stellar_rpc_client::EventStart::Cursor(resp.cursor);
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        "stellar get_events transient error while polling {}: {e:?}",
                        contract_id
                    );
                }
            }
            tracing::debug!(
                "Waiting for Triggered event on stellar contract {} (trigger_id={})",
                contract_id,
                trigger_id
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("Timeout waiting for Triggered event on {}", contract_id))?
}

/// Pull the 20-byte event_id out of a `Triggered { trigger_id, event_id }`
/// contract event. Matching topic chain is
/// `[Symbol("triggered"), U64(trigger_id)]`; the body is an `ScMap` with
/// one entry `{ Symbol("event_id") -> Bytes(20) }` because soroban-sdk's
/// `#[contractevent]` defaults to `data_format = "map"`. We tolerate a
/// bare `ScVal::Bytes(20)` body too in case a future version of the
/// macro switches to `single-value`.
fn extract_triggered_event_id(
    event: &wasi_stellar_rpc_client::Event,
    expected_trigger_id: u64,
) -> Option<String> {
    let topics: Vec<ScVal> = event
        .topic
        .iter()
        .filter_map(|t| ScVal::from_xdr_base64(t, Limits::none()).ok())
        .collect();
    let first_topic_is_triggered = matches!(
        topics.first(),
        Some(ScVal::Symbol(ScSymbol(sym))) if sym.as_slice() == b"triggered"
    );
    if !first_topic_is_triggered {
        return None;
    }
    let trigger_id_matches = matches!(
        topics.get(1),
        Some(ScVal::U64(v)) if *v == expected_trigger_id
    );
    if !trigger_id_matches {
        return None;
    }
    let value = ScVal::from_xdr_base64(&event.value, Limits::none()).ok()?;
    // Default `data_format = "map"` shape: { event_id -> Bytes(20) }.
    if let ScVal::Map(Some(map)) = &value {
        for entry in map.0.iter() {
            let key_matches = matches!(
                &entry.key,
                ScVal::Symbol(ScSymbol(sym)) if sym.as_slice() == b"event_id"
            );
            if key_matches {
                if let Some(hex) = scval_as_bytesn20_hex(&entry.val) {
                    return Some(hex);
                }
            }
        }
    }
    // `data_format = "single-value"` fallback.
    scval_as_bytesn20_hex(&value)
}

pub(super) fn scval_as_bytesn20_hex(v: &ScVal) -> Option<String> {
    match v {
        ScVal::Bytes(b) if b.0.len() == 20 => Some(const_hex::encode(&b.0)),
        _ => None,
    }
}

/// Poll Soroban RPC for the contract's first `Verified` event and
/// return the embedded event_id as a 40-char hex string. Unlike the
/// `Triggered` poll, this doesn't apply a server-side topic filter:
/// each handler is freshly deployed per test and emits exactly one
/// `Verified`, so the contract-id filter is enough.
pub(super) async fn wait_for_verified_event_id(
    rpc_url: &str,
    contract_id: &str,
    start_ledger: u32,
    timeout: Duration,
) -> Result<String> {
    let rpc = wasi_stellar_rpc_client::Client::new(rpc_url)
        .map_err(|e| anyhow!("failed to construct stellar rpc client: {e:?}"))?;

    tokio::time::timeout(timeout, async {
        // Advance the cursor across iterations so each poll only asks
        // for ledgers we haven't seen yet. The first call seeds from
        // `start_ledger`; subsequent calls resume from the cursor that
        // Soroban RPC returns.
        let mut start = wasi_stellar_rpc_client::EventStart::Ledger(start_ledger);
        loop {
            let resp = rpc
                .get_events(
                    start.clone(),
                    Some(wasi_stellar_rpc_client::EventType::Contract),
                    &[contract_id.to_string()],
                    &[],
                    None,
                )
                .await;
            match resp {
                Ok(resp) => {
                    for event in resp.events {
                        if let Some(hex) = extract_verified_event_id(&event) {
                            return Ok(hex);
                        }
                    }
                    if !resp.cursor.is_empty() {
                        start = wasi_stellar_rpc_client::EventStart::Cursor(resp.cursor);
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        "stellar get_events transient error while polling {}: {e:?}",
                        contract_id
                    );
                }
            }
            tracing::debug!(
                "Waiting for Verified event on stellar contract {}",
                contract_id
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("Timeout waiting for Verified event on {}", contract_id))?
}

/// Pulls the 20-byte event_id out of a `Verified` contract event.
///
/// `warpdrive_shared::interfaces::handler::Verified::publish` publishes
/// the event with `topics = [Symbol("verified"), BytesN<20>]` and an
/// empty payload (the body is `Verified::new(event_id)`). We accept both
/// the "event_id in topic[1]" shape and a fallback where the event_id is
/// in the event body — whichever the deployed Soroban runtime uses — so
/// the matcher is robust to upstream tweaks.
fn extract_verified_event_id(event: &wasi_stellar_rpc_client::Event) -> Option<String> {
    let topics: Vec<ScVal> = event
        .topic
        .iter()
        .filter_map(|t| ScVal::from_xdr_base64(t, Limits::none()).ok())
        .collect();
    let first_topic_is_verified = matches!(
        topics.first(),
        Some(ScVal::Symbol(ScSymbol(sym))) if sym.as_slice() == b"verified"
    );
    if !first_topic_is_verified {
        return None;
    }
    for tail in topics.iter().skip(1) {
        if let Some(hex) = scval_as_bytesn20_hex(tail) {
            return Some(hex);
        }
    }
    if let Ok(value) = ScVal::from_xdr_base64(&event.value, Limits::none()) {
        if let Some(hex) = scval_as_bytesn20_hex(&value) {
            return Some(hex);
        }
    }
    None
}
