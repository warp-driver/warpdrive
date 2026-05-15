use example_helpers::bindings::world::{
    host,
    warpdrive::vectr::{
        input::{TriggerAction, TriggerData},
        output::WasmResponse,
    },
    Guest,
};
use example_helpers::export_layer_trigger_world;
use example_helpers::trigger::encode_trigger_output;

// hardcoding this because our tests are mostly about event-based triggers
// but this component is not event-based
const TRIGGER_ID: u64 = 1338;

struct Component;

impl Guest for Component {
    fn run(trigger_action: TriggerAction) -> std::result::Result<Vec<WasmResponse>, String> {
        let return_data = b"cron-interval data";
        if let TriggerData::Cron(_data) = trigger_action.data {
            let service = host::get_service().service;
            Ok(vec![encode_trigger_output(
                TRIGGER_ID,
                return_data,
                service.manager,
                service.signature_kind,
            )])
        } else {
            Err("Invalid trigger data".to_string())
        }
    }
}

export_layer_trigger_world!(Component);
