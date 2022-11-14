use feature_probe_server_sdk::{FPConfig, FPError, FPUser, FeatureProbe};
use std::time::Duration;
use url::Url;

// Connect to demo docker environment.
// cargo run --example demo

#[tokio::main]
async fn main() -> Result<(), FPError> {
    tracing_subscriber::fmt::init();
    // let remote_url = "http://localhost:4009/server"; // for local docker
    let remote_url = Url::parse("https://featureprobe.io/server").expect("invalid url");
    // Server SDK key in Project List Page.
    let server_sdk_key = "server-7fa2f771259cb7235b96433d70b91e99abcf6ff8".to_owned();
    let refresh_interval = Duration::from_millis(2000);

    let config = FPConfig {
        remote_url,
        server_sdk_key,
        refresh_interval,
        start_wait: Some(Duration::from_secs(5)),
        ..Default::default()
    };

    let fp = FeatureProbe::new(config);
    if !fp.initialized() {
        println!("FeatureProbe failed to initialize, will return default value");
    }

    let mut user = FPUser::new();
    user = user.with("userId", "00001");
    let toggle_key = "campaign_allow_list";
    let enable = fp.bool_value(toggle_key, &user, false);
    println!("Result =>  : {:?}", enable);

    let detail = fp.bool_detail(toggle_key, &user, false);
    println!("       => reason : {:?}", detail.reason);
    println!("       => rule index  : {:?}", detail.rule_index);

    fp.close();
    Ok(())
}
