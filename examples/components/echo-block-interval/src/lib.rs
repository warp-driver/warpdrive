use example_helpers::bindings::world::{
    host,
    warpdrive::{
        types::{events::TriggerDataBlockInterval, service::TriggerBlockInterval},
        vectr::{
            input::{Trigger, TriggerAction, TriggerData},
            output::WasmResponse,
        },
    },
    wasi::keyvalue::{atomics, store},
    Guest,
};
use example_helpers::export_layer_trigger_world;
use example_helpers::trigger::encode_trigger_output;
use example_types::BlockIntervalResponse;

// hardcoding this because our tests are mostly about event-based triggers
// but this component is not event-based
const TRIGGER_ID: u64 = 1337;

struct Component;

impl Guest for Component {
    fn run(trigger_action: TriggerAction) -> std::result::Result<Vec<WasmResponse>, String> {
        match (trigger_action.config.trigger, trigger_action.data) {
            (Trigger::BlockInterval(config), TriggerData::BlockInterval(data)) => {
                run_maybe_one(config, data)
                    .map_err(|e| format!("{e:?}"))
                    .map(|r| r.into_iter().collect())
            }
            _ => Err("Invalid trigger config or data".to_string()),
        }
    }
}

fn run_maybe_one(
    config: TriggerBlockInterval,
    data: TriggerDataBlockInterval,
) -> std::result::Result<Option<WasmResponse>, anyhow::Error> {
    let bucket = store::open("foo")?;
    let count = atomics::increment(&bucket, "bar", 1)?.try_into()?;

    // TIP: temporarily comment out the `config.start_block.is_none()` condition
    // but leave the `count == 1`
    // and recompile to confirm that the trigger is deleted after the first trigger
    // (test will hang because trigger is deleted as a one-shot, but we haven't returned a response yet)
    if count == 1 && config.start_block.is_none() {
        // If this is the first trigger and no start block is set, wait for the next trigger
        return Ok(None);
    }
    let resp = BlockIntervalResponse {
        trigger_config_start: config.start_block,
        trigger_config_end: config.end_block,
        trigger_config_interval: config.n_blocks,
        trigger_data_block_height: data.block_height,
        count,
    };
    let resp = serde_json::to_vec(&resp)?;
    let service = host::get_service().service;
    Ok(Some(encode_trigger_output(
        TRIGGER_ID,
        resp,
        service.manager,
        service.signature_kind,
    )))
}

export_layer_trigger_world!(Component);
