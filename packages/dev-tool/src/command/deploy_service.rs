use warpdrive_cli::clients::HttpClient;
use warpdrive_types::ComponentDigest;

use crate::service::{create_service, WARPDRIVE_COMPONENT_BYTES};

pub async fn run(sleep_ms: Option<u64>) {
    let client = HttpClient::new("http://127.0.0.1:8000".to_string());

    if client
        .upload_component(WARPDRIVE_COMPONENT_BYTES.clone())
        .await
        .unwrap()
        != ComponentDigest::hash(&*WARPDRIVE_COMPONENT_BYTES)
    {
        panic!("wavs component bytes got unexpected hash!");
    }

    let service = create_service(sleep_ms);

    client.dev_add_service_direct(&service).await.unwrap();
}
