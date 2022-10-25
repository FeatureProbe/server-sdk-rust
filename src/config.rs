use std::time::Duration;

#[cfg(feature = "use_tokio")]
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
    #[cfg(feature = "use_tokio")]
    pub http_client: Option<Client>,
    pub start_wait: Option<Duration>,
}

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub toggles_url: Url,
    pub events_url: Url,
    pub server_sdk_key: String,
    pub refresh_interval: Duration,
    #[cfg(feature = "use_tokio")]
    pub http_client: Option<Client>,
    pub start_wait: Option<Duration>,
}

impl Default for FPConfig {
    fn default() -> Self {
        Self {
            server_sdk_key: "".to_owned(),
            remote_url: Url::parse("http://127.0.0.1:8080").unwrap(),
            toggles_url: None,
            events_url: None,
            refresh_interval: Duration::from_secs(5),
            start_wait: None,
            #[cfg(feature = "use_tokio")]
            http_client: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_sdk_key: "".to_owned(),
            toggles_url: Url::parse("http://127.0.0.1:8080").unwrap(),
            events_url: Url::parse("http://127.0.0.1:8080").unwrap(),
            refresh_interval: Duration::from_secs(5),
            start_wait: None,
            #[cfg(feature = "use_tokio")]
            http_client: None,
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

        let toggles_url = match &self.toggles_url {
            None => {
                Url::parse(&(remote_url.clone() + "api/server-sdk/toggles")).expect("invalid url")
            }
            Some(url) => url.to_owned(),
        };

        let events_url = match &self.events_url {
            None => Url::parse(&(remote_url + "api/events")).expect("invalid url"),
            Some(url) => url.to_owned(),
        };

        Config {
            toggles_url,
            events_url,
            server_sdk_key: self.server_sdk_key.clone(),
            refresh_interval: self.refresh_interval,
            start_wait: self.start_wait,
            #[cfg(feature = "use_tokio")]
            http_client: self.http_client.clone(),
        }
    }
}
