use crate::sync::SyncType;
use crate::{
    config::Config,
    evaluate::{EvalDetail, Repository},
};
use crate::{sync::Synchronizer, FPConfig};
use crate::{sync::UpdateCallback, user::FPUser};
use crate::{FPDetail, SdkAuthorization, Toggle};
use event::event::AccessEvent;
use event::event::CustomEvent;
use event::event::DebugEvent;
use event::event::Event;
use event::recorder::unix_timestamp;
use event::recorder::EventRecorder;
use feature_probe_event as event;
#[cfg(feature = "realtime")]
use futures_util::FutureExt;
use parking_lot::RwLock;
use serde_json::Value;
#[cfg(feature = "realtime")]
use socketio_rs::Client;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{trace, warn};

#[cfg(feature = "realtime")]
type SocketCallback = std::pin::Pin<Box<dyn futures_util::Future<Output = ()> + Send>>;

#[derive(Default, Clone)]
pub struct FeatureProbe {
    repo: Arc<RwLock<Repository>>,
    syncer: Option<Synchronizer>,
    event_recorder: Option<EventRecorder>,
    config: Config,
    should_stop: Arc<RwLock<bool>>,
    #[cfg(feature = "realtime")]
    socket: Option<Client>,
}

impl Debug for FeatureProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("FeatureProbe")
            .field(&self.repo)
            .field(&self.syncer)
            .field(&self.config)
            .field(&self.should_stop)
            .finish()
    }
}

impl FeatureProbe {
    pub fn new(config: FPConfig) -> Self {
        let config = config.build();
        let mut slf = Self {
            config,
            ..Default::default()
        };

        slf.start();
        slf
    }

    pub fn new_for_test(toggle: &str, value: Value) -> Self {
        let mut toggles = HashMap::new();
        toggles.insert(toggle.to_owned(), value);
        FeatureProbe::new_for_tests(toggles)
    }

    pub fn new_for_tests(toggles: HashMap<String, Value>) -> Self {
        let mut repo = Repository::default();
        for (key, val) in toggles {
            repo.toggles
                .insert(key.clone(), Toggle::new_for_test(key, val));
        }

        Self {
            repo: Arc::new(RwLock::new(repo)),
            ..Default::default()
        }
    }

    pub fn bool_value(&self, toggle: &str, user: &FPUser, default: bool) -> bool {
        self.generic_eval(toggle, user, default, false, |v| v.as_bool())
            .value
    }

    pub fn string_value(&self, toggle: &str, user: &FPUser, default: String) -> String {
        self.generic_eval(toggle, user, default, false, |v| {
            v.as_str().map(|s| s.to_owned())
        })
        .value
    }

    pub fn number_value(&self, toggle: &str, user: &FPUser, default: f64) -> f64 {
        self.generic_eval(toggle, user, default, false, |v| v.as_f64())
            .value
    }

    pub fn json_value(&self, toggle: &str, user: &FPUser, default: Value) -> Value {
        self.generic_eval(toggle, user, default, false, Some).value
    }

    pub fn bool_detail(&self, toggle: &str, user: &FPUser, default: bool) -> FPDetail<bool> {
        self.generic_eval(toggle, user, default, true, |v| v.as_bool())
    }

    pub fn string_detail(&self, toggle: &str, user: &FPUser, default: String) -> FPDetail<String> {
        self.generic_eval(toggle, user, default, true, |v| {
            v.as_str().map(|x| x.to_owned())
        })
    }

    pub fn number_detail(&self, toggle: &str, user: &FPUser, default: f64) -> FPDetail<f64> {
        self.generic_eval(toggle, user, default, true, |v| v.as_f64())
    }

    pub fn json_detail(&self, toggle: &str, user: &FPUser, default: Value) -> FPDetail<Value> {
        self.generic_eval(toggle, user, default, true, Some)
    }

    pub fn track(&self, event_name: &str, user: &FPUser, value: Option<f64>) {
        let recorder = match self.event_recorder.as_ref() {
            None => {
                warn!("Event Recorder no ready.");
                return;
            }
            Some(recorder) => recorder,
        };
        let event = CustomEvent {
            kind: "custom".to_string(),
            time: unix_timestamp(),
            user: user.key(),
            name: event_name.to_string(),
            value,
        };
        recorder.record_event(Event::CustomEvent(event));
    }

    pub fn new_with(server_key: String, repo: Repository) -> Self {
        Self {
            config: Config {
                server_sdk_key: server_key,
                ..Default::default()
            },
            repo: Arc::new(RwLock::new(repo)),
            syncer: None,
            event_recorder: None,
            should_stop: Arc::new(RwLock::new(false)),
            #[cfg(feature = "realtime")]
            socket: None,
        }
    }

    pub fn close(&self) {
        trace!("closing featureprobe client");
        if let Some(recorder) = &self.event_recorder {
            recorder.flush();
        }
        let mut should_stop = self.should_stop.write();
        *should_stop = true;
    }

    pub fn initialized(&self) -> bool {
        match &self.syncer {
            Some(s) => s.initialized(),
            None => false,
        }
    }

    pub fn set_update_callback(&mut self, update_callback: UpdateCallback) {
        if let Some(syncer) = &mut self.syncer {
            syncer.set_update_callback(update_callback)
        }
    }

    pub fn version(&self) -> Option<u128> {
        self.syncer.as_ref().map(|s| s.version()).flatten()
    }

    fn generic_eval<T: Default + Debug>(
        &self,
        toggle: &str,
        user: &FPUser,
        default: T,
        is_detail: bool,
        transform: fn(Value) -> Option<T>,
    ) -> FPDetail<T> {
        let (value, reason, detail) = match self.eval(toggle, user, is_detail) {
            None => (
                default,
                Some(format!("Toggle:[{toggle}] not exist")),
                Default::default(),
            ),
            Some(mut d) => match d.value.take() {
                None => (default, None, d), // Serve error.
                Some(v) => match transform(v) {
                    None => (default, Some("Value type mismatch.".to_string()), d), // Transform error.
                    Some(typed_v) => (typed_v, None, d),
                },
            },
        };

        FPDetail {
            value,
            reason: reason.unwrap_or(detail.reason),
            rule_index: detail.rule_index,
            variation_index: detail.variation_index,
            version: detail.version,
        }
    }

    fn eval(&self, toggle: &str, user: &FPUser, is_detail: bool) -> Option<EvalDetail<Value>> {
        let repo = self.repo.read();
        let debug_until_time = repo.debug_until_time;
        let detail = repo.toggles.get(toggle).map(|toggle| {
            toggle.eval(
                user,
                &repo.segments,
                &repo.toggles,
                is_detail,
                self.config.max_prerequisites_deep,
                debug_until_time,
            )
        });

        if let Some(recorder) = &self.event_recorder {
            let track_access_events = repo
                .toggles
                .get(toggle)
                .map(|t| t.track_access_events())
                .unwrap_or(false);
            record_event(
                recorder.clone(),
                track_access_events,
                toggle,
                user,
                detail.clone(),
                debug_until_time,
            )
        }

        detail.map(|mut d| {
            d.debug_until_time = debug_until_time;
            d
        })
    }

    fn start(&mut self) {
        self.sync();

        #[cfg(feature = "realtime")]
        self.connect_socket();

        if self.config.track_events {
            self.flush_events();
        }
    }

    fn sync(&mut self) {
        trace!("sync url {}", &self.config.toggles_url);
        let toggles_url = self.config.toggles_url.clone();
        let refresh_interval = self.config.refresh_interval;
        let auth = SdkAuthorization(self.config.server_sdk_key.clone()).encode();
        let repo = self.repo.clone();
        let syncer = Synchronizer::new(
            toggles_url,
            refresh_interval,
            auth,
            self.config.http_client.clone().unwrap_or_default(),
            repo,
        );
        self.syncer = Some(syncer.clone());
        syncer.start_sync(self.config.start_wait, self.should_stop.clone());
    }

    pub fn sync_now(&self, t: SyncType) {
        trace!("sync now url {}", &self.config.toggles_url);
        let syncer = match &self.syncer {
            Some(syncer) => syncer.clone(),
            None => return,
        };
        syncer.sync_now(t);
    }

    #[cfg(feature = "realtime")]
    fn connect_socket(&mut self) {
        let mut slf = self.clone();
        let slf2 = self.clone();
        let nsp = self.config.realtime_path.clone();
        tokio::spawn(async move {
            let url = slf.config.realtime_url;
            let server_sdk_key = slf.config.server_sdk_key.clone();
            trace!("connect_socket {}", url);
            let client = socketio_rs::ClientBuilder::new(url.clone())
                .namespace(&nsp)
                .on(socketio_rs::Event::Connect, move |_, socket, _| {
                    Self::socket_on_connect(socket, server_sdk_key.clone())
                })
                .on(
                    "update",
                    move |payload: Option<socketio_rs::Payload>, _, _| {
                        Self::socket_on_update(slf2.clone(), payload)
                    },
                )
                .on("error", |err, _, _| {
                    async move { tracing::error!("socket on error: {:#?}", err) }.boxed()
                })
                .connect()
                .await;
            match client {
                Err(e) => tracing::error!("connect_socket error: {:?}", e),
                Ok(client) => slf.socket = Some(client),
            };
        });
    }

    #[cfg(feature = "realtime")]
    fn socket_on_connect(socket: socketio_rs::Socket, server_sdk_key: String) -> SocketCallback {
        let sdk_key = server_sdk_key;
        trace!("socket_on_connect: {:?}", sdk_key);
        async move {
            if let Err(e) = socket
                .emit("register", serde_json::json!({ "key": sdk_key }))
                .await
            {
                tracing::error!("register error: {:?}", e);
            }
        }
        .boxed()
    }

    #[cfg(feature = "realtime")]
    fn socket_on_update(slf: Self, payload: Option<socketio_rs::Payload>) -> SocketCallback {
        trace!("socket_on_update: {:?}", payload);
        async move {
            if let Some(syncer) = &slf.syncer {
                syncer.sync_now(SyncType::Realtime);
            } else {
                warn!("socket receive update event, but no synchronizer");
            }
        }
        .boxed()
    }

    fn flush_events(&mut self) {
        trace!("flush_events");
        let events_url = self.config.events_url.clone();
        let flush_interval = self.config.refresh_interval;
        let auth = SdkAuthorization(self.config.server_sdk_key.clone()).encode();
        let should_stop = self.should_stop.clone();
        let event_recorder = EventRecorder::new(
            events_url,
            auth,
            (*crate::USER_AGENT).clone(),
            flush_interval,
            100,
            should_stop,
        );
        self.event_recorder = Some(event_recorder);
    }

    #[cfg(feature = "internal")]
    pub fn repo(&self) -> Arc<RwLock<Repository>> {
        self.repo.clone()
    }
}

fn record_event(
    recorder: EventRecorder,
    track_access_events: bool,
    toggle: &str,
    user: &FPUser,
    detail: Option<EvalDetail<Value>>,
    debug_until_time: Option<u64>,
) {
    let toggle = toggle.to_owned();
    let user = user.key();
    let user_detail = serde_json::to_value(user.clone()).unwrap_or_default();

    tokio::spawn(async move {
        let ts = unix_timestamp();
        record_access(
            &recorder,
            &toggle,
            user.clone(),
            track_access_events,
            &detail,
            ts,
        );
        record_debug(
            &recorder,
            &toggle,
            user,
            user_detail,
            debug_until_time,
            &detail,
            ts,
        );
    });
}

fn record_access(
    recorder: &EventRecorder,
    toggle: &str,
    user: String,
    track_access_events: bool,
    detail: &Option<EvalDetail<Value>>,
    ts: u128,
) -> Option<()> {
    let detail = detail.as_ref()?;
    let value = detail.value.as_ref()?;
    let event = AccessEvent {
        kind: "access".to_string(),
        time: ts,
        key: toggle.to_owned(),
        user,
        value: value.clone(),
        variation_index: detail.variation_index?,
        version: detail.version,
        rule_index: detail.rule_index,
        track_access_events,
    };
    recorder.record_event(Event::AccessEvent(event));
    None
}

#[allow(clippy::too_many_arguments)]
fn record_debug(
    recorder: &EventRecorder,
    toggle: &str,
    user: String,
    user_detail: Value,
    debug_until_time: Option<u64>,
    detail: &Option<EvalDetail<Value>>,
    ts: u128,
) -> Option<()> {
    let detail = detail.as_ref()?;
    let value = detail.value.as_ref()?;
    if let Some(debug_until_time) = debug_until_time {
        if debug_until_time as u128 >= ts {
            let debug = DebugEvent {
                kind: "debug".to_string(),
                time: ts,
                key: toggle.to_owned(),
                user,
                user_detail,
                value: value.clone(),
                variation_index: detail.variation_index?,
                version: detail.version,
                rule_index: detail.rule_index,
                reason: Some(detail.reason.to_string()),
            };
            recorder.record_event(Event::DebugEvent(debug));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::FPError;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn test_feature_probe_bool() {
        let json = load_local_json("resources/fixtures/repo.json");
        let fp = FeatureProbe::new_with("secret key".to_string(), json.unwrap());
        let u = FPUser::new().with("name", "bob").with("city", "1");

        assert!(fp.bool_value("bool_toggle", &u, false));
        assert!(fp.bool_detail("bool_toggle", &u, false).value);
    }

    #[test]
    fn test_feature_probe_number() {
        let json = load_local_json("resources/fixtures/repo.json");
        let fp = FeatureProbe::new_with("secret key".to_string(), json.unwrap());
        let u = FPUser::new().with("name", "bob").with("city", "1");

        assert_eq!(fp.number_value("number_toggle", &u, 0.0), 1.0);
        assert_eq!(fp.number_detail("number_toggle", &u, 0.0).value, 1.0);
    }

    #[test]
    fn test_feature_probe_string() {
        let json = load_local_json("resources/fixtures/repo.json");
        let fp = FeatureProbe::new_with("secret key".to_string(), json.unwrap());
        let u = FPUser::new().with("name", "bob").with("city", "1");

        assert_eq!(
            fp.string_value("string_toggle", &u, "".to_string()),
            "1".to_owned()
        );
        assert_eq!(
            fp.string_detail("string_toggle", &u, "".to_owned()).value,
            "1".to_owned()
        );
    }

    #[test]
    fn test_feature_probe_json() {
        let json = load_local_json("resources/fixtures/repo.json");
        let fp = FeatureProbe::new_with("secret key".to_string(), json.unwrap());
        let u = FPUser::new().with("name", "bob").with("city", "1");

        assert!(fp
            .json_value("json_toggle", &u, json!(""))
            .get("variation_0")
            .is_some());
        assert!(fp
            .json_detail("json_toggle", &u, json!(""))
            .value
            .get("variation_0")
            .is_some());
    }

    #[test]
    fn test_feature_probe_none_exist_toggle() {
        let json = load_local_json("resources/fixtures/repo.json");
        let fp = FeatureProbe::new_with("secret key".to_string(), json.unwrap());
        let u = FPUser::new();

        assert!(fp.bool_value("none_exist_toggle", &u, true));
        let d = fp.bool_detail("none_exist_toggle", &u, true);
        assert!(d.value);
        assert_eq!(d.rule_index, None);
    }

    #[test]
    fn test_for_ut() {
        let fp = FeatureProbe::new_for_test("toggle_1", Value::Bool(false));
        let u = FPUser::new();
        assert!(!fp.bool_value("toggle_1", &u, true));

        let mut toggles: HashMap<String, Value> = HashMap::new();
        toggles.insert("toggle_2".to_owned(), json!(12.5));
        toggles.insert("toggle_3".to_owned(), json!("value"));
        let fp = FeatureProbe::new_for_tests(toggles);
        assert_eq!(fp.number_value("toggle_2", &u, 20.0), 12.5);
        assert_eq!(fp.string_value("toggle_3", &u, "val".to_owned()), "value");
    }

    #[test]
    fn test_feature_probe_record_debug() {
        let json = load_local_json("resources/fixtures/repo.json");
        let mut repo = json.unwrap();
        repo.debug_until_time = Some(unix_timestamp() as u64 + 60 * 1000);
        let fp = FeatureProbe::new_with("secret key".to_string(), repo);
        let u = FPUser::new().with("name", "bob").with("city", "1");
        fp.bool_value("bool_toggle", &u, false);
    }

    fn load_local_json(file: &str) -> Result<Repository, FPError> {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push(file);
        let json_str = fs::read_to_string(path).unwrap();
        let repo = crate::evaluate::load_json(&json_str);
        assert!(repo.is_ok(), "err is {:?}", repo);
        repo
    }
}

#[cfg(test)]
mod server_sdk_contract_tests {
    use crate::{FPDetail, FPError, FPUser, FeatureProbe, Repository};
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use std::fmt::Debug;
    use std::fs;
    use std::path::PathBuf;
    use std::string::String;

    #[allow(dead_code)]
    pub(crate) fn load_tests_json(json_str: &str) -> Result<Tests, FPError> {
        serde_json::from_str::<Tests>(json_str)
            .map_err(|e| FPError::JsonError(json_str.to_owned(), e))
    }

    #[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
    #[serde(rename_all = "camelCase")]
    pub struct Tests {
        pub(crate) tests: Vec<Scenario>,
    }

    #[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
    pub struct Scenario {
        pub(crate) scenario: String,
        pub(crate) cases: Vec<Case>,
        pub(crate) fixture: Repository,
    }

    #[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct Case {
        pub(crate) name: String,
        pub(crate) user: User,
        pub(crate) function: Function,
        pub(crate) expect_result: ExpectResult,
    }

    #[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct User {
        pub(crate) key: String,
        pub(crate) custom_values: Vec<KeyValue>,
    }

    #[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
    pub struct KeyValue {
        pub(crate) key: String,
        pub(crate) value: String,
    }

    #[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
    pub struct Function {
        pub(crate) name: String,
        pub(crate) toggle: String,
        pub(crate) default: Value,
    }

    #[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ExpectResult {
        pub(crate) value: Value,
        pub(crate) reason: Option<String>,
        pub(crate) rule_index: Option<usize>,
        pub(crate) no_rule_index: Option<bool>,
        pub(crate) version: Option<u64>,
    }

    #[test]
    fn test_contract() {
        let root = load_test_json("resources/fixtures/spec/spec/toggle_simple_spec.json");
        assert!(root.is_ok());

        for scenario in root.unwrap().tests {
            println!("scenario: {}", scenario.scenario);
            assert!(!scenario.cases.is_empty());

            let fp = FeatureProbe::new_with("secret key".to_string(), scenario.fixture);

            for case in scenario.cases {
                println!("  case: {}", case.name);

                let mut user = FPUser::new().stable_rollout(case.user.key.clone());
                for custom_value in &case.user.custom_values {
                    user = user.with(custom_value.key.clone(), custom_value.value.clone());
                }

                macro_rules! validate_value {
                    ( $fun:ident, $default:expr, $expect:expr) => {
                        let ret = fp.$fun(case.function.toggle.as_str(), &user, $default);
                        assert_eq!(ret, $expect);
                    };
                }

                macro_rules! validate_detail {
                    ( $fun:ident, $default:expr, $expect:expr) => {
                        let ret = fp.$fun(case.function.toggle.as_str(), &user, $default);
                        assert_eq!(ret.value, $expect);
                        assert_detail(&case, ret);
                    };
                }

                match case.function.name.as_str() {
                    "bool_value" => {
                        validate_value!(
                            bool_value,
                            case.function.default.as_bool().unwrap(),
                            case.expect_result.value.as_bool().unwrap()
                        );
                    }
                    "string_value" => {
                        validate_value!(
                            string_value,
                            case.function.default.as_str().unwrap().to_string(),
                            case.expect_result.value.as_str().unwrap().to_string()
                        );
                    }
                    "number_value" => {
                        validate_value!(
                            number_value,
                            case.function.default.as_f64().unwrap(),
                            case.expect_result.value.as_f64().unwrap()
                        );
                    }
                    "json_value" => {
                        validate_value!(
                            json_value,
                            case.function.default,
                            case.expect_result.value
                        );
                    }
                    "bool_detail" => {
                        validate_detail!(
                            bool_detail,
                            case.function.default.as_bool().unwrap(),
                            case.expect_result.value
                        );
                    }
                    "string_detail" => {
                        validate_detail!(
                            string_detail,
                            case.function.default.as_str().unwrap().to_string(),
                            case.expect_result.value
                        );
                    }
                    "number_detail" => {
                        validate_detail!(
                            number_detail,
                            case.function.default.as_f64().unwrap(),
                            case.expect_result.value
                        );
                    }
                    "json_detail" => {
                        validate_detail!(
                            json_detail,
                            case.function.default.clone(),
                            case.expect_result.value
                        );
                    }
                    _ => panic!("function name {} not found.", case.function.name),
                }
            }
        }
    }

    fn assert_detail<T: Default + Debug>(case: &Case, ret: FPDetail<T>) {
        match &case.expect_result.reason {
            None => (),
            Some(r) => {
                assert!(
                    ret.reason.contains(r.as_str()),
                    "reason: \"{}\" does not contains \"{}\"",
                    ret.reason.as_str(),
                    r.as_str()
                );
            }
        };

        if case.expect_result.rule_index.is_some() {
            assert_eq!(
                case.expect_result.rule_index, ret.rule_index,
                "rule index not match"
            );
        }

        if case.expect_result.no_rule_index.is_some() {
            assert!(
                case.expect_result.rule_index.is_none(),
                "should not have rule index."
            );
        }

        if case.expect_result.version.is_some() {
            assert_eq!(case.expect_result.version, ret.version, "version not match");
        }
    }

    fn load_test_json(file: &str) -> Result<Tests, FPError> {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push(file);
        let mut json_str = fs::read_to_string(path.clone());
        if json_str.is_err() {
            use std::process::Command;
            Command::new("git")
                .args(["submodule", "init"])
                .status()
                .expect("init");
            Command::new("git")
                .args(["submodule", "update"])
                .status()
                .expect("update");
            json_str = fs::read_to_string(path);
        }
        assert!(json_str.is_ok(),
                "contract test resource not found, run `git submodule init && git submodule update` to fetch");
        let tests = load_tests_json(&json_str.unwrap());
        assert!(tests.is_ok(), "err is {:?}", tests);
        tests
    }
}
