use std::{sync::Arc, time::Duration};

use feature_probe_server::{
    http::{serve_http, FpHttpHandler, LocalFileHttpHandler},
    repo::SdkRepository,
    ServerConfig,
};
use feature_probe_server_sdk::{FPConfigBuilder, FPUser, FeatureProbe, Url};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn integration_test() {
    let _ = tracing_subscriber::fmt().init();

    let api_port = 19980;
    let server_port = 19990;
    setup_server(api_port, server_port).await;

    let config = FPConfigBuilder::new(
        format!("http://127.0.0.1:{}", server_port),
        "server-sdk-key1".to_owned(),
        Duration::from_secs(5),
    )
    .start_wait(Duration::from_secs(5))
    .build();
    assert!(config.is_ok());
    let config = config.unwrap();

    let fp = FeatureProbe::new(config);

    let user = FPUser::new();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let b = fp.bool_detail("bool_toggle", &user, false);
    assert_eq!(b.value, true);
}

async fn setup_server(api_port: u16, server_port: u16) {
    // mock fp api
    tokio::spawn(serve_http::<LocalFileHttpHandler>(
        api_port,
        LocalFileHttpHandler {},
    ));

    let server_sdk_key = "server-sdk-key1".to_owned();
    let client_sdk_key = "client-sdk-key1".to_owned();

    tokio::time::sleep(Duration::from_secs(1)).await;

    // start fp server
    let toggles_url = format!("http://0.0.0.0:{}/api/server-sdk/toggles", api_port)
        .parse()
        .unwrap();
    let events_url: Url = format!("http://0.0.0.0:{}/api/events", api_port)
        .parse()
        .unwrap();
    let refresh_interval = Duration::from_secs(1);
    let repo = SdkRepository::new(ServerConfig {
        toggles_url,
        server_port,
        refresh_interval,
        keys_url: None,
        events_url: events_url.clone(),
        client_sdk_key: Some(client_sdk_key.clone()),
        server_sdk_key: Some(server_sdk_key.clone()),
    });
    repo.sync(client_sdk_key, server_sdk_key);
    let repo = Arc::new(repo);
    let feature_probe_server = FpHttpHandler {
        repo: repo.clone(),
        events_url,
        events_timeout: Duration::from_secs(1),
        http_client: Default::default(),
    };
    tokio::spawn(serve_http::<FpHttpHandler>(
        server_port,
        feature_probe_server,
    ));
    tokio::time::sleep(Duration::from_secs(1)).await;
}
