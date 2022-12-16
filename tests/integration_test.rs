use std::{sync::Arc, time::Duration};

use feature_probe_server::{
    http::{serve_http, FpHttpHandler, LocalFileHttpHandlerForTest},
    realtime::RealtimeSocket,
    repo::SdkRepository,
    ServerConfig,
};
use feature_probe_server_sdk::{FPConfig, FPUser, FeatureProbe, SyncType, Url};
use parking_lot::Mutex;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn integration_test() {
    // tracing_subscriber::fmt()
    //     .with_env_filter(
    //         "feature_probe_server_sdk=trace,integration=trace,socket=trace,engine=trace",
    //     )
    //     .pretty()
    //     .init();

    let api_port = 19980;
    let server_port = 19990;
    let realtime_port = 19999;
    let realtime_path = "/".to_owned();
    setup_server(api_port, server_port, realtime_port, realtime_path).await;

    let config = FPConfig {
        remote_url: Url::parse(&format!("http://127.0.0.1:{}", server_port)).unwrap(),
        server_sdk_key: "server-sdk-key1".to_owned(),
        refresh_interval: Duration::from_secs(2),
        start_wait: Some(Duration::from_secs(5)),
        #[cfg(feature = "realtime")]
        realtime_url: Some(Url::parse(&format!("http://127.0.0.1:{}", realtime_port)).unwrap()),
        ..Default::default()
    };

    let mut fp = FeatureProbe::new(config);
    #[cfg(all(feature = "use_tokio", feature = "realtime"))]
    fp.sync_now(SyncType::Polling);
    let did_update = {
        let did_update = Arc::new(Mutex::new((false, false)));
        let did_update_clone = did_update.clone();

        fp.set_update_callback(Box::new(move |_old, _new, t| {
            let mut lock = did_update_clone.lock();
            match t {
                SyncType::Realtime => lock.0 = true,
                SyncType::Polling => lock.1 = true,
            };
        }));

        did_update
    };

    let user = FPUser::new();

    tokio::time::sleep(Duration::from_millis(500)).await;

    assert!(fp.initialized());

    let b = fp.bool_detail("bool_toggle", &user, false);
    assert!(b.value);

    tokio::time::sleep(Duration::from_millis(3000)).await;
    let lock = did_update.lock();
    #[cfg(feature = "realtime")]
    assert!(lock.0);
    #[cfg(not(feature = "realtime"))]
    assert!(lock.1);
}

async fn setup_server(api_port: u16, server_port: u16, realtime_port: u16, realtime_path: String) {
    let mut mock_api = LocalFileHttpHandlerForTest::default();
    mock_api.version_update = true;
    // mock fp api
    tokio::spawn(serve_http::<LocalFileHttpHandlerForTest>(
        api_port, mock_api,
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
    let config = ServerConfig {
        toggles_url,
        server_port,
        realtime_port,
        realtime_path,
        refresh_interval,
        keys_url: None,
        events_url: events_url.clone(),
        client_sdk_key: Some(client_sdk_key.clone()),
        server_sdk_key: Some(server_sdk_key.clone()),
    };
    let realtime_socket = RealtimeSocket::serve(config.realtime_port, &config.realtime_path);
    let repo = SdkRepository::new(config, realtime_socket);
    repo.sync(client_sdk_key, server_sdk_key, 1);
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
