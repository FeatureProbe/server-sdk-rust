use std::time::Duration;

#[cfg(feature = "use_tokio")]
use reqwest::Client;
use tracing::info;
use url::Url;

use crate::FPError;

#[derive(Debug, Default, Clone)]
pub struct FPConfigBuilder {
    pub remote_url: String,
    pub toggles_url: Option<String>,
    pub events_url: Option<String>,
    pub server_sdk_key: String,
    pub refresh_interval: Duration,
    #[cfg(feature = "use_tokio")]
    pub http_client: Option<Client>,
    pub start_wait: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct FPConfig {
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
            toggles_url: Url::parse("http://127.0.0.1:8080").unwrap(),
            events_url: Url::parse("http://127.0.0.1:8080").unwrap(),
            refresh_interval: Duration::from_secs(5),
            start_wait: None,
            #[cfg(feature = "use_tokio")]
            http_client: None,
        }
    }
}

impl FPConfigBuilder {
    pub fn new(
        remote_url: String,
        server_sdk_key: String,
        refresh_interval: Duration,
    ) -> FPConfigBuilder {
        Self {
            remote_url,
            server_sdk_key,
            refresh_interval,
            ..Default::default()
        }
    }

    pub fn toggles_url(mut self, toggles_url: String) -> FPConfigBuilder {
        self.toggles_url = Some(toggles_url);
        self
    }

    pub fn events_url(mut self, events_url: String) -> FPConfigBuilder {
        self.events_url = Some(events_url);
        self
    }

    pub fn start_wait(mut self, start_wait: Duration) -> FPConfigBuilder {
        self.start_wait = Some(start_wait);
        self
    }

    #[cfg(feature = "use_tokio")]
    pub fn http_client(mut self, http_client: Client) -> FPConfigBuilder {
        self.http_client = Some(http_client);
        self
    }

    pub fn build(&self) -> Result<FPConfig, FPError> {
        info!("build_config from {:?}", self);
        let remote_url = {
            if !self.remote_url.ends_with('/') {
                self.remote_url.clone() + "/"
            } else {
                self.remote_url.clone()
            }
        };
        let toggles_url = match &self.toggles_url {
            None => remote_url.clone() + "api/server-sdk/toggles",
            Some(url) => url.to_owned(),
        };

        let toggles_url: Url = match Url::parse(&toggles_url) {
            Err(e) => return Err(FPError::UrlError(e.to_string())),
            Ok(url) => url,
        };

        let events_url = match &self.events_url {
            None => remote_url + "api/events",
            Some(url) => url.to_owned(),
        };

        let events_url: Url = match Url::parse(&events_url) {
            Err(e) => return Err(FPError::UrlError(e.to_string())),
            Ok(url) => url,
        };

        Ok(FPConfig {
            toggles_url,
            events_url,
            server_sdk_key: self.server_sdk_key.clone(),
            refresh_interval: self.refresh_interval,
            start_wait: self.start_wait,
            #[cfg(feature = "use_tokio")]
            http_client: self.http_client.clone(),
        })
    }
}
