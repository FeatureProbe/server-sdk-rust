use feature_probe_server_sdk::{FPConfig, FPUser, FeatureProbe};
use std::time::Duration;

// Connect to demo docker environment.
// cargo run --example demo

#[tokio::main]
async fn main() {
    // let remote_url = "http://localhost:4007"; // for local docker
    let remote_url = "https://featureprobe.io/server";
    // this key can fetch data, but can not change toggle
    let server_sdk_key = "server-7fa2f771259cb7235b96433d70b91e99abcf6ff8";
    // let server_sdk_key = /* paste server key from project list for changing toggle */;
    let interval = Duration::from_millis(2000);
    let config = FPConfig {
        remote_url: remote_url.to_owned(),
        server_sdk_key: server_sdk_key.to_owned(),
        refresh_interval: interval,
        #[cfg(feature = "use_tokio")]
        http_client: None,
        wait_first_resp: true,
        ..Default::default()
    };

    let fp = match FeatureProbe::new(config) {
        Ok(fp) => fp,
        Err(e) => {
            tracing::error!("{:?}", e);
            return;
        }
    };

    let mut user = FPUser::new();
    user = user.with("userId", "00001");
    let toggle_key = "campaign_allow_list";
    let enable = fp.bool_value(toggle_key, &user, false);
    println!("Result =>  : {:?}", enable);

    let detail = fp.bool_detail(toggle_key, &user, false);
    println!("       => reason : {:?}", detail.reason);
    println!("       => rule index  : {:?}", detail.rule_index);

    fp.close();
}
