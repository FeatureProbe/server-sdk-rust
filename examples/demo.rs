use clap::{ArgEnum, Parser};
use feature_probe_server_sdk::{FPConfig, FPUser, FeatureProbe};
use serde_json::json;
use std::error::Error;
use std::time::Duration;

// cargo run --example demo -- -t test_jg_toggle_string -r string -u 10 -a userId=102 -a email=test@mail.com -D
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(
        short,
        long,
        default_value = "http://www.featureprobe.com/feature-probe-server"
    )]
    host_url: String,

    #[clap(
        short,
        long,
        default_value = "server-3df72071f12a143ba67769ae6837323a2392da6c"
    )]
    server_sdk_key: String,

    /// Toggle key
    #[clap(short, long)]
    toggle: String,

    /// Toggle return type
    #[clap(short, long, arg_enum)]
    return_type: ReturnType,

    /// User key
    #[clap(short, long)]
    user: String,

    /// User fields, k=v
    #[clap(short, long, parse(try_from_str = parse_key_val), multiple_occurrences(true))]
    attrs: Vec<(String, String)>,

    /// Make the toggle execute show detail
    #[clap(short = 'D', long)]
    show_detail: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ArgEnum)]
enum ReturnType {
    String,
    Boolean,
    JSON,
    Number,
}

fn parse_key_val<T, U>(s: &str) -> Result<(T, U), Box<dyn Error + Send + Sync + 'static>>
where
    T: std::str::FromStr,
    T::Err: Error + Send + Sync + 'static,
    U: std::str::FromStr,
    U::Err: Error + Send + Sync + 'static,
{
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{}`", s))?;
    Ok((s[..pos].parse()?, s[pos + 1..].parse()?))
}

#[tokio::main]
async fn main() {
    let _ = tracing_subscriber::fmt().init();
    let args = Args::parse();
    let remote_url = args.host_url;

    let config = FPConfig {
        remote_url: remote_url.to_owned(),
        server_sdk_key: args.server_sdk_key.to_owned(),
        refresh_interval: Duration::from_millis(400),
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

    let user = FPUser::new(&args.user).with_attrs(args.attrs.into_iter());

    let detail = match args.return_type {
        ReturnType::Boolean => json!(fp.bool_detail(&args.toggle, &user, false)),
        ReturnType::String => json!(fp.string_detail(&args.toggle, &user, "".to_string())),
        ReturnType::Number => json!(fp.number_detail(&args.toggle, &user, 0.0)),
        ReturnType::JSON => json!(fp.json_detail(&args.toggle, &user, json!(""))),
    };
    tracing::info!(
        "Args => \n\tServer sdk key: {}\n\tToggle: {}\n\tReturn type: {:?} \n\tUser: {:?}",
        args.server_sdk_key,
        args.toggle,
        args.return_type,
        user,
    );
    if args.show_detail {
        tracing::warn!("Detail => \n\t{:?}\n", detail);
    }
    tracing::info!("Result => \n\t{:?}\n", detail.get("value").unwrap());
    tokio::time::sleep(Duration::from_secs(1)).await; // wait event flush
}
