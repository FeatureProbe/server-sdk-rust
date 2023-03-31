use std::time::Duration;

use reqwest::Client;
use tracing::info;
use url::Url;

#[derive(Debug, Clone)]
pub struct FPConfig {
    pub remote_url: Url,
    pub toggles_url: Option<Url>,
    pub events_url: Option<Url>,
    pub server_sdk_key: String,
    pub refresh_interval: Duration,
    pub http_client: Option<Client>,
    pub start_wait: Option<Duration>,

    #[cfg(feature = "realtime")]
    pub realtime_url: Option<Url>,
    #[cfg(feature = "realtime")]
    pub realtime_path: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub toggles_url: Url,
    pub events_url: Url,
    pub server_sdk_key: String,
    pub refresh_interval: Duration,
    pub http_client: Option<Client>,
    pub start_wait: Option<Duration>,

    #[cfg(feature = "realtime")]
    pub realtime_url: Url,
    pub realtime_path: String,
    pub max_prerequisites_deep: u8,
}

impl Default for FPConfig {
    fn default() -> Self {
        Self {
            server_sdk_key: "".to_owned(),
            remote_url: Url::parse("https://featureprobe.io/server").unwrap(),
            toggles_url: None,
            events_url: None,
            refresh_interval: Duration::from_secs(5),
            start_wait: None,
            http_client: None,

            #[cfg(feature = "realtime")]
            realtime_url: None,
            #[cfg(feature = "realtime")]
            realtime_path: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_sdk_key: "".to_owned(),
            toggles_url: Url::parse("https://featureprobe.io/server/api/server-sdk/toggles")
                .unwrap(),
            events_url: Url::parse("https://featureprobe.io/server/api/events").unwrap(),
            refresh_interval: Duration::from_secs(60),
            start_wait: None,
            http_client: None,

            #[cfg(feature = "realtime")]
            realtime_url: Url::parse("https://featureprobe.io/server/realtime").unwrap(),
            #[cfg(feature = "realtime")]
            realtime_path: "/server/realtime".to_owned(),
            max_prerequisites_deep: 20,
        }
    }
}

impl FPConfig {
    pub(crate) fn build(&self) -> Config {
        info!("build_config from {:?}", self);
        let remote_url = self.remote_url.to_string();
        let remote_url = match remote_url.ends_with('/') {
            true => remote_url,
            false => remote_url + "/",
        };

        #[cfg(feature = "realtime")]
        let realtime_url = match &self.realtime_url {
            None => Url::parse(&(remote_url.clone() + "realtime")).expect("invalid realtime url"),
            Some(url) => url.to_owned(),
        };

        #[cfg(feature = "realtime")]
        let realtime_path = match &self.realtime_path {
            Some(p) => p.to_owned(),
            None => realtime_url.path().to_owned(),
        };

        let toggles_url = match &self.toggles_url {
            None => Url::parse(&(remote_url.clone() + "api/server-sdk/toggles"))
                .expect("invalid toggles url"),
            Some(url) => url.to_owned(),
        };

        let events_url = match &self.events_url {
            None => Url::parse(&(remote_url + "api/events")).expect("invalid events url"),
            Some(url) => url.to_owned(),
        };

        Config {
            toggles_url,
            events_url,
            server_sdk_key: self.server_sdk_key.clone(),
            refresh_interval: self.refresh_interval,
            start_wait: self.start_wait,
            http_client: self.http_client.clone(),
            #[cfg(feature = "realtime")]
            realtime_url,
            #[cfg(feature = "realtime")]
            realtime_path,
            ..Default::default()
        }
    }
}
