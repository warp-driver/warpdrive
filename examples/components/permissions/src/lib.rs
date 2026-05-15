use example_helpers::bindings::world::{
    host,
    warpdrive::{
        types::{
            core::LogLevel,
            service::{ComponentSource, ServiceAndWorkflowId},
        },
        vectr::{input::TriggerAction, output::WasmResponse},
    },
    Guest,
};

use example_helpers::export_layer_trigger_world;
use example_helpers::trigger::{decode_trigger_event, encode_trigger_output};
use std::{fs, io::Write, path::Path};
use warpdrive_wasi_utils::http::{
    fetch_json, fetch_string, http_request_get, http_request_post_json,
};

use anyhow::{Context, Result};
use serde::Deserialize;

use example_types::{PermissionsRequest, PermissionsResponse};

struct Component;

impl Guest for Component {
    fn run(trigger_action: TriggerAction) -> std::result::Result<Vec<WasmResponse>, String> {
        run_one(trigger_action)
            .map_err(|e: anyhow::Error| format!("{e:?}"))
            .map(|res| vec![res])
    }
}

#[tokio::main(flavor = "current_thread")]
async fn run_one(trigger_action: TriggerAction) -> Result<WasmResponse> {
    let (trigger_id, req) = decode_trigger_event(trigger_action.data).context("Decode event")?;
    let input: PermissionsRequest = serde_json::from_slice(&req).context("Parsing request")?;

    println!("(permissions println!) trigger id: {trigger_id}");
    eprintln!("(permissions eprintln!) trigger id: {trigger_id}");
    host::log(
        LogLevel::Info,
        &format!("(permissions host log) trigger id: {trigger_id}"),
    );

    const DIRECTORY_NAME: &str = "./responses";

    let responses_path = Path::new(DIRECTORY_NAME);
    if !responses_path.exists() {
        fs::create_dir_all(DIRECTORY_NAME)?;
    }

    let response_path = responses_path.join(format!("{}.txt", input.timestamp));
    let mut response_file = fs::File::create(&response_path)?;

    let get_response = fetch_string(http_request_get(&input.get_url)?).await?;

    #[derive(Deserialize, Debug)]
    struct PostResponse {
        json: (String, String),
    }

    let post_response: PostResponse =
        fetch_json(http_request_post_json(&input.post_url, &input.post_data)?).await?;

    if post_response.json != input.post_data {
        return Err(anyhow::anyhow!(
            "The post data is not the same as the one sent"
        ));
    }

    let contents = format!("GET RESPONSE: {get_response}\n\nPOST RESPONSE: {post_response:?}");

    response_file.write_all(contents.as_bytes())?;

    let responses_count = fs::read_dir(responses_path)?.count();

    let ServiceAndWorkflowId {
        service,
        workflow_id,
    } = host::get_service();

    let workflow = service
        .workflows
        .into_iter()
        .find_map(|(id, workflow)| {
            if id == workflow_id {
                Some(workflow)
            } else {
                None
            }
        })
        .ok_or(anyhow::anyhow!("Failed to find workflow"))?;

    let digest = match workflow.component.source {
        ComponentSource::Download(component_source_download) => component_source_download.digest,
        ComponentSource::Registry(registry) => registry.digest,
        ComponentSource::Digest(digest) => digest,
    };

    let resp = PermissionsResponse {
        filename: response_path.to_path_buf(),
        contents,
        filecount: responses_count,
        digest,
    };
    let resp = serde_json::to_vec(&resp).context("Serializing response")?;
    Ok(encode_trigger_output(
        trigger_id,
        resp,
        service.manager,
        service.signature_kind,
    ))
}

export_layer_trigger_world!(Component);
