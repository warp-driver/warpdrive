use anyhow::{bail, Context};
use example_helpers::bindings::world::{
    host,
    warpdrive::vectr::{
        input::{TriggerAction, TriggerData},
        output::WasmResponse,
    },
    Guest,
};
use example_helpers::export_layer_trigger_world;
use example_helpers::trigger::{decode_trigger_event, stellar_encode_trigger_output};
struct Component;

impl Guest for Component {
    fn run(trigger_action: TriggerAction) -> std::result::Result<Vec<WasmResponse>, String> {
        run_multi(trigger_action).map_err(|e| format!("{e:?}"))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn run_multi(
    trigger_action: TriggerAction,
) -> std::result::Result<Vec<WasmResponse>, anyhow::Error> {
    if let Some(n) = host::config_var("sleep-ms") {
        let n = n.parse::<u64>().context("invalid sleep-ms")?;

        match host::config_var("sleep-kind").as_deref() {
            Some("async") => {
                tokio::time::sleep(tokio::time::Duration::from_millis(n)).await;
            }
            Some("sync") => {
                std::thread::sleep(std::time::Duration::from_millis(n));
            }
            Some("hotloop") => {
                let start = std::time::Instant::now();
                let expires = std::time::Duration::from_millis(n);
                while start.elapsed() < expires {
                    // busy wait
                }
            }
            _ => {
                bail!("invalid or missing 'sleep-kind', must be 'async', 'sync', or 'hotloop'");
            }
        }
    }

    // Sanity check that we can get the default event id
    if host::get_event_id(None).iter().all(|x| *x == 0) {
        bail!("event id is all zeros");
    }

    let (maybe_trigger_id, data) = match trigger_action.data {
        TriggerData::EvmContractEvent(_)
        | TriggerData::CosmosContractEvent(_)
        | TriggerData::StellarContractEvent(_)
        | TriggerData::AtprotoEvent(_) => {
            let (trigger_id, data) = decode_trigger_event(trigger_action.data)?;

            (Some(trigger_id), data)
        }
        TriggerData::Raw(data) => (None, data),
        _ => bail!("expected trigger data"),
    };

    if let Ok(input_str) = std::str::from_utf8(&data) {
        if input_str.contains("envvar:") {
            let env_var = input_str.split("envvar:").nth(1).unwrap();
            if let Ok(value) = std::env::var(env_var) {
                if let Some(trigger_id) = maybe_trigger_id {
                    return Ok(vec![stellar_encode_trigger_output(trigger_id, value)]);
                }
                return Ok(vec![WasmResponse {
                    payload: value.as_bytes().to_vec(),
                    ordering: None,
                    event_id_salt: None,
                }]);
            } else {
                bail!("env var {env_var} not found");
            }
        } else if input_str.contains("configvar:") {
            let config_var = input_str.split("configvar:").nth(1).unwrap();
            if let Some(value) = host::config_var(config_var) {
                if let Some(trigger_id) = maybe_trigger_id {
                    return Ok(vec![stellar_encode_trigger_output(trigger_id, value)]);
                }
                return Ok(vec![WasmResponse {
                    payload: value.as_bytes().to_vec(),
                    ordering: None,
                    event_id_salt: None,
                }]);
            } else {
                bail!("config var {config_var} not found");
            }
        } else if input_str == "custom-event-id" {
            return Ok(vec![WasmResponse {
                payload: Vec::new(),
                ordering: None,
                event_id_salt: Some(
                    host::config_var("event-id-salt")
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                ),
            }]);
        } else if input_str == "multi-response" {
            return Ok(vec![
                WasmResponse {
                    payload: Vec::new(),
                    ordering: None,
                    event_id_salt: Some(
                        host::config_var("event-id-salt-1")
                            .unwrap()
                            .as_bytes()
                            .to_vec(),
                    ),
                },
                WasmResponse {
                    payload: Vec::new(),
                    ordering: None,
                    event_id_salt: Some(
                        host::config_var("event-id-salt-2")
                            .unwrap()
                            .as_bytes()
                            .to_vec(),
                    ),
                },
            ]);
        } else if input_str == "multi-response-bad" {
            return Ok(vec![
                WasmResponse {
                    payload: Vec::new(),
                    ordering: None,
                    event_id_salt: Some(
                        host::config_var("event-id-salt-1")
                            .unwrap()
                            .as_bytes()
                            .to_vec(),
                    ),
                },
                WasmResponse {
                    payload: Vec::new(),
                    ordering: None,
                    event_id_salt: None,
                },
            ]);
        }
    }

    if let Some(trigger_id) = maybe_trigger_id {
        return Ok(vec![stellar_encode_trigger_output(trigger_id, data)]);
    }
    Ok(vec![WasmResponse {
        payload: data,
        ordering: None,
        event_id_salt: None,
    }])
}

export_layer_trigger_world!(Component);
