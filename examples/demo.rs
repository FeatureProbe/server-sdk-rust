use feature_probe_server_sdk::{FPConfig, FPUser, FeatureProbe};
use std::time::Duration;

// Connect to demo docker environment.
// cargo run --example demo

#[tokio::main]
async fn main() {
    // let _ = tracing_subscriber::fmt()
    //     .with_env_filter("feature_probe_server_sdk=trace")
    //     .init();

    let remote_url = "http://localhost:4007";
    let server_sdk_key = "server-8ed48815ef044428826787e9a238b9c6a479f98c";
    let interval = Duration::from_millis(1000);
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

    let user = FPUser::new("user_id").with("city", "Paris");
    let discount = fp.number_value("promotion_activity", &user, 9.0);
    println!("Result => discount for user in Paris is : {:?}", discount);

    let detail = fp.number_detail("promotion_activity", &user, 9.0);
    println!("       => reason : {:?}", detail.reason);
    println!("       => rule index  : {:?}", detail.rule_index);

    let user2 = FPUser::new("user_id").with("city", "New York");
    let discount2 = fp.number_value("promotion_activity", &user2, 9.0);
    println!(
        "Result => discount for user in New York is : {:?}",
        discount2
    );

    tokio::time::sleep(interval).await;
}
