use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::trace;

use crate::user::FPUser;
use crate::{
    config::Config,
    evalutate::{EvalDetail, Repository},
};
use crate::{sync::Synchronizer, FPConfig};
use crate::{FPDetail, SdkAuthorization, Toggle};
#[cfg(feature = "event")]
use feature_probe_event_std::event::AccessEvent;
#[cfg(feature = "event")]
use feature_probe_event_std::recorder::unix_timestamp;
#[cfg(feature = "event")]
use feature_probe_event_std::recorder::EventRecorder;
#[cfg(feature = "event_tokio")]
use feature_probe_event_tokio::event::AccessEvent;
#[cfg(feature = "event_tokio")]
use feature_probe_event_tokio::recorder::unix_timestamp;
#[cfg(feature = "event_tokio")]
use feature_probe_event_tokio::recorder::EventRecorder;

#[derive(Debug, Default, Clone)]
pub struct FeatureProbe {
    repo: Arc<RwLock<Repository>>,
    syncer: Option<Synchronizer>,
    #[cfg(any(feature = "event", feature = "event_tokio"))]
    event_recorder: Option<EventRecorder>,
    config: Config,
    should_stop: Arc<RwLock<bool>>,
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
        self.generic_detail(toggle, user, default, |v| v.as_bool())
            .value
    }

    pub fn string_value(&self, toggle: &str, user: &FPUser, default: String) -> String {
        self.generic_detail(toggle, user, default, |v| v.as_str().map(|s| s.to_owned()))
            .value
    }

    pub fn number_value(&self, toggle: &str, user: &FPUser, default: f64) -> f64 {
        self.generic_detail(toggle, user, default, |v| v.as_f64())
            .value
    }

    pub fn json_value(&self, toggle: &str, user: &FPUser, default: Value) -> Value {
        self.generic_detail(toggle, user, default, Some).value
    }

    pub fn bool_detail(&self, toggle: &str, user: &FPUser, default: bool) -> FPDetail<bool> {
        self.generic_detail(toggle, user, default, |v| v.as_bool())
    }

    pub fn string_detail(&self, toggle: &str, user: &FPUser, default: String) -> FPDetail<String> {
        self.generic_detail(toggle, user, default, |v| v.as_str().map(|x| x.to_owned()))
    }

    pub fn number_detail(&self, toggle: &str, user: &FPUser, default: f64) -> FPDetail<f64> {
        self.generic_detail(toggle, user, default, |v| v.as_f64())
    }

    pub fn json_detail(&self, toggle: &str, user: &FPUser, default: Value) -> FPDetail<Value> {
        self.generic_detail(toggle, user, default, Some)
    }

    pub fn new_with(server_key: String, repo: Repository) -> Self {
        Self {
            config: Config {
                server_sdk_key: server_key,
                ..Default::default()
            },
            repo: Arc::new(RwLock::new(repo)),
            syncer: None,
            #[cfg(any(feature = "event", feature = "event_tokio"))]
            event_recorder: None,
            should_stop: Arc::new(RwLock::new(false)),
        }
    }

    pub fn close(&self) {
        trace!("closing featureprobe client");
        #[cfg(any(feature = "event", feature = "event_tokio"))]
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

    fn generic_detail<T: Default + Debug>(
        &self,
        toggle: &str,
        user: &FPUser,
        default: T,
        transform: fn(Value) -> Option<T>,
    ) -> FPDetail<T> {
        let (value, reason, detail) = match self.eval_detail(toggle, user) {
            None => (
                default,
                Some(format!("Toggle:[{}] not exist", toggle)),
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

    fn eval_detail(&self, toggle: &str, user: &FPUser) -> Option<EvalDetail<Value>> {
        let repo = self.repo.read();
        let detail = repo
            .toggles
            .get(toggle)
            .map(|toggle| toggle.eval_detail(user, &repo.segments));

        #[cfg(any(feature = "event", feature = "event_tokio"))]
        self.record_detail(toggle, &detail);

        detail
    }

    #[cfg(any(feature = "event", feature = "event_tokio"))]
    fn record_detail(&self, toggle: &str, detail: &Option<EvalDetail<Value>>) -> Option<()> {
        let recorder = self.event_recorder.as_ref()?;
        let detail = detail.as_ref()?;
        let value = detail.value.as_ref()?;
        recorder.record_access(AccessEvent {
            time: unix_timestamp(),
            key: toggle.to_owned(),
            value: value.clone(),
            index: detail.variation_index,
            version: detail.version,
            reason: detail.reason.clone(),
        });
        None
    }

    fn start(&mut self) {
        self.sync();
        #[cfg(any(feature = "event", feature = "event_tokio"))]
        self.flush_events();
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
            #[cfg(feature = "use_tokio")]
            self.config.http_client.clone(),
            repo,
        );
        self.syncer = Some(syncer.clone());
        syncer.sync(self.config.start_wait, self.should_stop.clone());
    }

    #[cfg(any(feature = "event", feature = "event_tokio"))]
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
        assert_eq!(d.value, true);
        assert_eq!(d.rule_index, None);
    }

    #[test]
    fn test_for_ut() {
        let fp = FeatureProbe::new_for_test("toggle_1", Value::Bool(false));
        let u = FPUser::new();
        assert_eq!(fp.bool_value("toggle_1", &u, true), false);

        let mut toggles: HashMap<String, Value> = HashMap::new();
        toggles.insert("toggle_2".to_owned(), json!(12.5));
        toggles.insert("toggle_3".to_owned(), json!("value"));
        let fp = FeatureProbe::new_for_tests(toggles);
        assert_eq!(fp.number_value("toggle_2", &u, 20.0), 12.5);
        assert_eq!(fp.string_value("toggle_3", &u, "val".to_owned()), "value");
    }

    fn load_local_json(file: &str) -> Result<Repository, FPError> {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push(file);
        let json_str = fs::read_to_string(path).unwrap();
        let repo = crate::evalutate::load_json(&json_str);
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
        serde_json::from_str::<Tests>(json_str).map_err(FPError::JsonError)
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

    #[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
    #[serde(rename_all = "camelCase")]
    pub struct Case {
        pub(crate) name: String,
        pub(crate) user: User,
        pub(crate) function: Function,
        pub(crate) expect_result: ExpectResult,
    }

    #[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
    #[serde(rename_all = "camelCase")]
    pub struct User {
        pub(crate) key: String,
        pub(crate) custom_values: Vec<KeyValue>,
    }

    #[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
    pub struct KeyValue {
        pub(crate) key: String,
        pub(crate) value: String,
    }

    #[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
    pub struct Function {
        pub(crate) name: String,
        pub(crate) toggle: String,
        pub(crate) default: Value,
    }

    #[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
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
                    _ => assert!(false, "function name {} not found.", case.function.name),
                }
            }
        }
    }

    fn assert_detail<T: std::default::Default + Debug>(case: &Case, ret: FPDetail<T>) {
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
