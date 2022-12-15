use crate::FPError;
use crate::Repository;
use headers::HeaderValue;
use parking_lot::{Mutex, RwLock};
#[cfg(feature = "use_tokio")]
use reqwest::{header::AUTHORIZATION, Client, Method};
use std::{sync::mpsc::sync_channel, time::Instant};
use std::{sync::Arc, time::Duration};
use tracing::trace;
use tracing::{debug, error};
use url::Url;

pub type UpdateCallback = Box<dyn Fn(Repository, Repository, SyncType) + Send>;

#[derive(Debug, Clone)]
pub struct Synchronizer {
    inner: Arc<Inner>,
}

#[derive(Debug)]
pub enum SyncType {
    Realtime,
    Polling,
}

struct Inner {
    toggles_url: Url,
    refresh_interval: Duration,
    auth: HeaderValue,
    #[cfg(feature = "use_tokio")]
    client: Client,
    repo: Arc<RwLock<Repository>>,
    is_init: Arc<RwLock<bool>>,
    update_callback: Arc<Mutex<Option<UpdateCallback>>>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SynchronizerInner")
            .field(&self.toggles_url)
            .field(&self.refresh_interval)
            .field(&self.repo)
            .field(&self.is_init)
            .finish()
    }
}

//TODO: graceful shutdown
impl Synchronizer {
    pub fn new(
        toggles_url: Url,
        refresh_interval: Duration,
        auth: HeaderValue,
        #[cfg(feature = "use_tokio")] client: Client,
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
                is_init: Default::default(),
                update_callback: Arc::new(Mutex::new(None)),
            }),
        }
    }

    pub fn initialized(&self) -> bool {
        let lock = self.inner.is_init.read();
        *lock
    }

    #[cfg(feature = "use_std")]
    pub fn start_sync(&self, start_wait: Option<Duration>, should_stop: Arc<RwLock<bool>>) {
        let inner = self.inner.clone();
        let (tx, rx) = sync_channel(1);
        let start = Instant::now();
        let mut is_send = false;
        let interval_duration = inner.refresh_interval;

        let is_timeout = Self::init_timeout_fn(start_wait, interval_duration, start);
        std::thread::spawn(move || loop {
            if let Some(r) =
                Self::should_send(inner.sync_now(SyncType::Polling), &is_timeout, is_send)
            {
                is_send = true;
                let _ = tx.try_send(r);
            }

            if *should_stop.read() {
                break;
            }
            std::thread::sleep(inner.refresh_interval);
        });

        if start_wait.is_some() {
            let _ = rx.recv();
        }
    }

    #[cfg(feature = "use_tokio")]
    pub fn start_sync(&self, start_wait: Option<Duration>, should_stop: Arc<RwLock<bool>>) {
        let inner = self.inner.clone();
        let (tx, rx) = sync_channel(1);
        let start = Instant::now();
        let mut is_send = false;
        let interval_duration = inner.refresh_interval;
        let is_timeout = Self::init_timeout_fn(start_wait, interval_duration, start);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(inner.refresh_interval);
            loop {
                let result = inner.sync_now(SyncType::Polling).await;

                if let Some(r) = Self::should_send(result, &is_timeout, is_send) {
                    is_send = true;
                    let _ = tx.try_send(r);
                }

                if *should_stop.read() {
                    break;
                }
                interval.tick().await;
            }
        });

        if start_wait.is_some() {
            let _ = rx.recv();
        }
    }

    pub fn set_update_callback(&mut self, update_callback: UpdateCallback) {
        let mut lock = self.inner.update_callback.lock();
        *lock = Some(update_callback);
    }

    #[cfg(test)]
    pub fn repository(&self) -> Arc<RwLock<Repository>> {
        self.inner.repo.clone()
    }

    #[cfg(test)]
    fn notify_update(&self, old_repo: Repository, new_repo: Repository, t: SyncType) {
        self.inner.notify_update(old_repo, new_repo, t)
    }

    fn init_timeout_fn(
        start_wait: Option<Duration>,
        interval: Duration,
        start: Instant,
    ) -> Option<Box<dyn Fn() -> bool + Send>> {
        match start_wait {
            Some(timeout) => Some(Box::new(move || start.elapsed() + interval > timeout)),
            None => None,
        }
    }

    fn should_send(
        result: Result<(), FPError>,
        is_timeout: &Option<Box<dyn Fn() -> bool + Send>>,
        is_send: bool,
    ) -> Option<Result<(), FPError>> {
        if let Some(is_timeout) = is_timeout {
            match result {
                Ok(_) if !is_send => {
                    return Some(Ok(()));
                }
                Err(e) if !is_send && is_timeout() => {
                    error!("sync error: {}", e);
                    return Some(Err(e));
                }
                Err(e) => error!("sync error: {}", e),
                _ => {}
            }
        }
        None
    }

    pub fn sync_now(&self, t: SyncType) {
        #[cfg(feature = "use_tokio")]
        {
            let slf = self.clone();
            tokio::spawn(async move { slf.inner.sync_now(t).await });
        }

        #[cfg(feature = "use_std")]
        let _ = self.inner.sync_now(t);
    }
}

impl Inner {
    #[cfg(feature = "use_tokio")]
    pub async fn sync_now(&self, t: SyncType) -> Result<(), FPError> {
        use http::header::USER_AGENT;

        trace!("sync_now {:?} {:?}", self.auth, t);
        let mut request = self
            .client
            .request(Method::GET, self.toggles_url.clone())
            .header(AUTHORIZATION, self.auth.clone())
            .header(USER_AGENT, &*crate::USER_AGENT)
            .timeout(self.refresh_interval);

        {
            let repo = self.repo.read();
            if let Some(version) = &repo.version {
                request = request.query(&[("version", &version.to_string())]);
            }
        } // drop repo lock

        //TODO: report failure
        match request.send().await {
            Err(e) => Err(FPError::HttpError(e.to_string())),
            Ok(resp) => match resp.text().await {
                Err(e) => Err(FPError::HttpError(e.to_string())),
                Ok(body) => match serde_json::from_str::<Repository>(&body) {
                    Err(e) => Err(FPError::JsonError(body, e)),
                    Ok(r) => {
                        // TODO: validate repo
                        // TODO: diff change, notify subscriber
                        debug!("sync success {:?}", r);
                        let mut repo = self.repo.write();
                        if r.version > repo.version {
                            let old = (*repo).clone();
                            let new = r.clone();
                            *repo = r;
                            self.notify_update(old, new, t);
                        }
                        let mut is_init = self.is_init.write();
                        *is_init = true;
                        Ok(())
                    }
                },
            },
        }
    }

    #[cfg(feature = "use_std")]
    pub fn sync_now(&self, t: SyncType) -> Result<(), FPError> {
        trace!("sync_now {:?}, {:?}", self.auth, t);
        //TODO: report failure
        let mut request = ureq::get(self.toggles_url.as_str())
            .set(
                "Authorization",
                self.auth.to_str().expect("already valid header value"),
            )
            .set("User-Agent", &crate::USER_AGENT)
            .timeout(self.refresh_interval);

        {
            let repo = self.repo.read();
            if let Some(version) = &repo.version {
                request = request.query("version", &version.to_string())
            }
        } // drop repo lock

        match request.call() {
            Err(e) => Err(FPError::HttpError(e.to_string())),
            Ok(r) => match r.into_string() {
                Err(e) => Err(FPError::HttpError(e.to_string())),
                Ok(body) => {
                    match serde_json::from_str::<Repository>(&body) {
                        Err(e) => Err(FPError::JsonError(body, e)),
                        Ok(r) => {
                            // TODO: validate repo
                            debug!("sync success {:?}", r);
                            let mut repo = self.repo.write();
                            if r.version > repo.version {
                                let old = (*repo).clone();
                                let new = r.clone();
                                *repo = r;
                                self.notify_update(old, new, t);
                            }
                            let mut is_init = self.is_init.write();
                            *is_init = true;
                            Ok(())
                        }
                    }
                }
            },
        }
    }

    fn notify_update(&self, old_repo: Repository, new_repo: Repository, t: SyncType) {
        let lock = self.update_callback.lock();
        if let Some(cb) = &*lock {
            cb(old_repo, new_repo, t)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SdkAuthorization;
    use axum::{routing::get, Json, Router, TypedHeader};
    use headers::UserAgent;
    use std::{fs, net::SocketAddr, path::PathBuf, sync::mpsc::channel};

    #[test]
    fn test_update_callback() {
        let mut syncer = build_synchronizer(9000);
        let (tx, rx) = channel();

        syncer.set_update_callback(Box::new(move |_old, _new, _| tx.send(()).unwrap()));
        let old = Repository::default();
        let new = Repository::default();
        syncer.notify_update(old, new, SyncType::Polling);

        assert!(rx.try_recv().is_ok())
    }

    #[tokio::test]
    async fn test_init_timeout_fn() {
        let now = Instant::now();
        let now = now - Duration::from_millis(10);

        let is_timeout_fn = Synchronizer::init_timeout_fn(None, Duration::from_millis(1), now);
        assert!(is_timeout_fn.is_none());

        let is_timeout_fn = Synchronizer::init_timeout_fn(
            Some(Duration::from_millis(20)),
            Duration::from_millis(1),
            now,
        );
        assert!(!is_timeout_fn.unwrap()());

        let is_timeout_fn = Synchronizer::init_timeout_fn(
            Some(Duration::from_millis(5)),
            Duration::from_millis(1),
            now,
        );
        assert!(is_timeout_fn.unwrap()());
    }

    #[test]
    fn test_should_send() {
        let is_timeout_fn = None;
        let r = Synchronizer::should_send(Ok(()), &is_timeout_fn, false);
        assert!(r.is_none(), "no need send because not set timeout");

        let is_timeout_fn: Option<Box<dyn Fn() -> bool + Send>> = Some(Box::new(|| false));
        let r = Synchronizer::should_send(Ok(()), &is_timeout_fn, false);
        assert!(r.is_some(), "need send because not timeout, and return Ok");
        let r = r.unwrap();
        assert!(r.is_ok());

        let is_timeout_fn: Option<Box<dyn Fn() -> bool + Send>> = Some(Box::new(|| false));
        let r = Synchronizer::should_send(Ok(()), &is_timeout_fn, true);
        assert!(
            r.is_none(),
            "no need send because not timeout, and return error, wait next loop"
        );

        let is_timeout_fn: Option<Box<dyn Fn() -> bool + Send>> = Some(Box::new(|| false));
        let is_send = true;
        let r = Synchronizer::should_send(
            Err(FPError::InternalError("unkown".to_owned())),
            &is_timeout_fn,
            is_send,
        );
        assert!(r.is_none(), "no need send because already send before");

        let is_timeout_fn: Option<Box<dyn Fn() -> bool + Send>> = Some(Box::new(|| true));
        let r = Synchronizer::should_send(
            Err(FPError::InternalError("unkown".to_owned())),
            &is_timeout_fn,
            is_send,
        );
        assert!(r.is_none(), "no need send because already send before");

        let is_send = false;
        let is_timeout_fn: Option<Box<dyn Fn() -> bool + Send>> = Some(Box::new(|| true));
        let r = Synchronizer::should_send(
            Err(FPError::InternalError("unkown".to_owned())),
            &is_timeout_fn,
            is_send,
        );
        assert!(r.is_some(), "need send because already timeout");
        let r = r.unwrap();
        assert!(r.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_sync() {
        // let _ = tracing_subscriber::fmt().init();

        let port = 9009;
        setup_mock_api(port).await;
        let syncer = build_synchronizer(port);
        let should_stop = Arc::new(RwLock::new(false));
        syncer.start_sync(Some(Duration::from_secs(5)), should_stop);

        let repo = syncer.repository();
        let repo = repo.read();
        assert!(!repo.toggles.is_empty());
        assert!(syncer.initialized());
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
                client: Default::default(),
                repo: Default::default(),
                is_init: Default::default(),
                update_callback: Default::default(),
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
        TypedHeader(user_agent): TypedHeader<UserAgent>,
    ) -> Json<Repository> {
        assert_eq!(sdk_key, "sdk-key");
        assert!(!user_agent.to_string().is_empty());
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/fixtures/repo.json");
        let json_str = fs::read_to_string(path).unwrap();
        let repo = serde_json::from_str::<Repository>(&json_str).unwrap();
        repo.into()
    }
}
