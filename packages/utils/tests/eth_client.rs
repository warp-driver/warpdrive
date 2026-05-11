use alloy_primitives::FixedBytes;
use utils::{
    evm_client::{EvmSigningClient, EvmSigningClientConfig},
    init_tracing_tests,
    test_utils::anvil::safe_spawn_anvil,
};
use warpdrive_types::{Credential, Envelope};

#[tokio::test]
async fn client_sign_message() {
    init_tracing_tests();
    let anvil = safe_spawn_anvil();

    let config = EvmSigningClientConfig::new(
        anvil.endpoint().parse().unwrap(),
        Credential::new(
            "work man father plunge mystery proud hollow address reunion sauce theory bonus"
                .to_string(),
        ),
    );
    let client = EvmSigningClient::new(config).await.unwrap();

    let envelope = Envelope {
        eventId: FixedBytes::new([0u8; 20]),
        ordering: FixedBytes::new([0u8; 12]),
        payload: b"hello world".to_vec().into(),
    };

    // client.wallet doesn't itself allow signing messages, but we created the wallet from the signer
    let signature = client
        .signer
        .write()
        .await
        .sign_envelope(&envelope)
        .await
        .unwrap();

    let recovered_address = signature
        .signer_address(&envelope)
        .unwrap()
        .try_as_evm()
        .expect("evm signer returns an evm address");

    // check that the wallet's default signer is the same as the recovered address
    assert_eq!(recovered_address, client.wallet.default_signer().address());
}
