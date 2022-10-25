use feature_probe_server_sdk::{FPConfigBuilder, FPError, FPUser, FeatureProbe};
use std::time::Duration;

// Connect to demo docker environment.
// cargo run --example demo

#[tokio::main]
async fn main() -> Result<(), FPError> {
    let _ = tracing_subscriber::fmt().init();
    // let remote_url = "http://localhost:4009/server"; // for local docker
    let remote_url = "https://featureprobe.io/server";
    // Server SDK key in Project List Page.
    let server_sdk_key = "server-7fa2f771259cb7235b96433d70b91e99abcf6ff8";
    let interval = Duration::from_millis(2000);
    let config = FPConfigBuilder::new(remote_url.to_owned(), server_sdk_key.to_owned(), interval)
        .start_wait(Duration::from_secs(5))
        .build()?;

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
