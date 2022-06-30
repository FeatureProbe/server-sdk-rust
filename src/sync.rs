use crate::Repository;
use headers::HeaderValue;
use parking_lot::RwLock;
#[cfg(feature = "use_tokio")]
use reqwest::{header::AUTHORIZATION, Client, Method};
use std::{sync::Arc, time::Duration};
use tracing::{debug, error};
use url::Url;

#[derive(Debug, Clone)]
pub struct Synchronizer {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    toggles_url: Url,
    refresh_interval: Duration,
    auth: HeaderValue,
    #[cfg(feature = "use_tokio")]
    client: Option<Client>,
    repo: Arc<RwLock<Repository>>,
}

//TODO: graceful shutdown
impl Synchronizer {
    pub fn new(
        toggles_url: Url,
        refresh_interval: Duration,
        auth: HeaderValue,
        #[cfg(feature = "use_tokio")] client: Option<Client>,
        repo: Arc<RwLock<Repository>>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                toggles_url,
                refresh_interval,
                auth,
                #[cfg(feature = "use_tokio")]
                client,
                repo,
            }),
        }
    }

    #[cfg(feature = "use_std")]
    pub fn sync(&self, wait_first_resp: bool) {
        let inner = self.inner.clone();
        if wait_first_resp {
            inner.do_sync()
        }
        std::thread::spawn(move || {
            if wait_first_resp {
                std::thread::sleep(inner.refresh_interval);
            }
            loop {
                inner.do_sync();
                std::thread::sleep(inner.refresh_interval);
            }
        });
    }

    #[cfg(feature = "use_tokio")]
    pub fn sync(&self, wait_first_resp: bool) {
        use std::sync::mpsc::sync_channel;
        let inner = self.inner.clone();
        let client = match &self.inner.client {
            Some(c) => c.clone(),
            None => reqwest::Client::new(),
        };
        let (tx, rx) = sync_channel(1);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(inner.refresh_interval);
            if wait_first_resp {
                inner.do_sync(&client).await;
                let _ = tx.send(true);
                interval.tick().await;
            }
            loop {
                inner.do_sync(&client).await;
                interval.tick().await;
            }
        });

        if wait_first_resp {
            let _ = rx.recv();
        }
    }

    #[cfg(test)]
    pub fn repository(&self) -> Arc<RwLock<Repository>> {
        self.inner.repo.clone()
    }
}

impl Inner {
    #[cfg(feature = "use_tokio")]
    async fn do_sync(&self, client: &Client) {
        use http::header::USER_AGENT;

        let request = client
            .request(Method::GET, self.toggles_url.clone())
            .header(AUTHORIZATION, self.auth.clone())
            .header(USER_AGENT, &*crate::USER_AGENT)
            .timeout(self.refresh_interval);

        //TODO: report failure
        match request.send().await {
            Err(e) => error!("sync error: {}", e),
            Ok(resp) => match resp.text().await {
                Err(e) => error!("sync error: {}", e),
                Ok(body) => match serde_json::from_str::<Repository>(&body) {
                    Err(e) => error!("sync error: {} {}", e, body),
                    Ok(r) => {
                        // TODO: validate repo
                        // TODO: diff change, notify subscriber
                        debug!("sync success {:?}", r);
                        let mut repo = self.repo.write();
                        *repo = r
                    }
                },
            },
        }
    }

    #[cfg(feature = "use_std")]
    fn do_sync(&self) {
        //TODO: report failure
        match ureq::get(self.toggles_url.as_str())
            .set(
                "authorization",
                self.auth.to_str().expect("already valid header value"),
            )
            .set("user-agent", &*crate::USER_AGENT)
            .timeout(self.refresh_interval)
            .call()
        {
            Err(e) => error!("do_sync: ureq error {}", e),
            Ok(r) => match r.into_string() {
                Err(e) => error!("sync error: {}", e),
                Ok(body) => {
                    match serde_json::from_str::<Repository>(&body) {
                        Err(e) => error!("sync error: {} {}", e, body),
                        Ok(r) => {
                            // TODO: validate repo
                            debug!("sync success {:?}", r);
                            let mut repo = self.repo.write();
                            *repo = r
                        }
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SdkAuthorization;
    use axum::{routing::get, Json, Router, TypedHeader};
    use std::{fs, net::SocketAddr, path::PathBuf};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_sync() {
        let _ = tracing_subscriber::fmt().init();

        let port = 9009;
        setup_mock_api(port).await;
        let syncer = build_synchronizer(port);
        syncer.sync(true);

        let repo = syncer.repository();
        let repo = repo.read();
        assert!(!repo.toggles.is_empty());
    }

    fn build_synchronizer(port: u16) -> Synchronizer {
        let toggles_url =
            Url::parse(&format!("http://127.0.0.1:{}/api/server-sdk/toggles", port)).unwrap();
        let refresh_interval = Duration::from_secs(10);
        let auth = SdkAuthorization("sdk-key".to_owned()).encode();
        Synchronizer {
            inner: Arc::new(Inner {
                toggles_url,
                refresh_interval,
                auth,
                #[cfg(feature = "use_tokio")]
                client: None,
                repo: Default::default(),
            }),
        }
    }

    async fn setup_mock_api(port: u16) {
        let app = Router::new().route("/api/server-sdk/toggles", get(server_sdk_toggles));
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        tokio::spawn(async move {
            let _ = axum::Server::bind(&addr)
                .serve(app.into_make_service())
                .await;
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    async fn server_sdk_toggles(
        TypedHeader(SdkAuthorization(sdk_key)): TypedHeader<SdkAuthorization>,
    ) -> Json<Repository> {
        assert_eq!(sdk_key, "sdk-key");
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/fixtures/repo.json");
        let json_str = fs::read_to_string(path).unwrap();
        let repo = serde_json::from_str::<Repository>(&json_str).unwrap();
        repo.into()
    }
}
