use crate::evalutate::{EvalDetail, Repository};
use crate::sync::Synchronizer;
use crate::user::FPUser;
use crate::{FPDetail, FPError, SdkAuthorization};
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
use parking_lot::RwLock;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;
use url::Url;

#[cfg(feature = "internal")]
use crate::evalutate::Segment;
#[cfg(feature = "internal")]
use crate::evalutate::Toggle;
#[cfg(feature = "use_tokio")]
use reqwest::Client;
#[cfg(feature = "internal")]
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct FeatureProbe {
    repo: Arc<RwLock<Repository>>,
    syncer: Option<Synchronizer>,
    #[cfg(any(feature = "event", feature = "event_tokio"))]
    event_recorder: Option<EventRecorder>,
    config: InnerConfig,
}

impl FeatureProbe {
    pub fn new(config: FPConfig) -> Result<Self, FPError> {
        let config = build_config(config);
        let mut slf = Self {
            config,
            ..Default::default()
        };

        match slf.start() {
            Ok(_) => Ok(slf),
            Err(e) => Err(e),
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
            config: InnerConfig {
                server_sdk_key: server_key,
                ..Default::default()
            },
            repo: Arc::new(RwLock::new(repo)),
            syncer: None,
            #[cfg(any(feature = "event", feature = "event_tokio"))]
            event_recorder: None,
        }
    }

    fn generic_detail<T: Default>(
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
            index: detail.rule_index,
            version: detail.version,
            reason: detail.reason.clone(),
        });
        None
    }

    fn start(&mut self) -> Result<(), FPError> {
        self.sync()?;
        #[cfg(any(feature = "event", feature = "event_tokio"))]
        self.flush_events()?;
        Ok(())
    }

    fn sync(&mut self) -> Result<(), FPError> {
        info!("sync url {}", &self.config.toggles_url);
        let toggles_url: Url = match Url::parse(&self.config.toggles_url) {
            Err(e) => return Err(FPError::UrlError(e.to_string())),
            Ok(url) => url,
        };
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
        syncer.sync(self.config.wait_first_resp);
        self.syncer = Some(syncer);
        Ok(())
    }

    #[cfg(any(feature = "event", feature = "event_tokio"))]
    fn flush_events(&mut self) -> Result<(), FPError> {
        info!("flush_events");
        let events_url: Url = match Url::parse(&self.config.events_url) {
            Err(e) => return Err(FPError::UrlError(e.to_string())),
            Ok(url) => url,
        };
        let flush_interval = self.config.refresh_interval;
        let auth = SdkAuthorization(self.config.server_sdk_key.clone()).encode();
        let event_recorder = EventRecorder::new(events_url, auth, flush_interval, 100);
        self.event_recorder = Some(event_recorder);
        Ok(())
    }

    #[cfg(feature = "internal")]
    pub fn update_toggles(&mut self, toggles: HashMap<String, Toggle>) {
        let mut repo = self.repo.write();
        repo.toggles.extend(toggles)
    }

    #[cfg(feature = "internal")]
    pub fn update_segments(&mut self, segments: HashMap<String, Segment>) {
        let mut repo = self.repo.write();
        repo.segments.extend(segments)
    }

    #[cfg(feature = "internal")]
    pub fn repo_string(&self) -> String {
        let repo = self.repo.read();
        serde_json::to_string(&*repo).expect("repo valid json format")
    }

    #[cfg(feature = "internal")]
    pub fn all_evaluated_string(&self, user: &FPUser) -> String {
        let repo = self.repo.read();
        let map: HashMap<String, EvalDetail<Value>> = repo
            .toggles
            .iter()
            .filter(|(_, t)| t.is_for_client())
            .map(|(key, toggle)| (key.to_owned(), toggle.eval_detail(user, &repo.segments)))
            .collect();
        serde_json::to_string(&map).expect("valid json format")
    }
}

#[derive(Debug, Default, Clone)]
pub struct FPConfig {
    pub remote_url: String,
    pub toggles_url: Option<String>,
    pub events_url: Option<String>,
    pub server_sdk_key: String,
    pub refresh_interval: Duration,
    #[cfg(feature = "use_tokio")]
    pub http_client: Option<Client>,
    pub wait_first_resp: bool,
}

#[derive(Debug, Default, Clone)]
pub struct InnerConfig {
    pub toggles_url: String,
    pub events_url: String,
    pub server_sdk_key: String,
    pub refresh_interval: Duration,
    #[cfg(feature = "use_tokio")]
    pub http_client: Option<Client>,
    pub wait_first_resp: bool,
}

fn build_config(mut config: FPConfig) -> InnerConfig {
    info!("build_config from {:?}", config);
    if !config.remote_url.ends_with('/') {
        config.remote_url += "/";
    }
    if config.toggles_url.is_none() {
        config.toggles_url = Some(config.remote_url.clone() + "api/server-sdk/toggles")
    }
    if config.events_url.is_none() {
        config.events_url = Some(config.remote_url + "api/events")
    }

    InnerConfig {
        toggles_url: config.toggles_url.expect("not none"),
        events_url: config.events_url.expect("not none"),
        server_sdk_key: config.server_sdk_key,
        refresh_interval: config.refresh_interval,
        wait_first_resp: config.wait_first_resp,
        #[cfg(feature = "use_tokio")]
        http_client: config.http_client,
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
        let u = FPUser::new("key").with("name", "bob").with("city", "1");

        assert!(fp.bool_value("bool_toggle", &u, false));
        assert!(fp.bool_detail("bool_toggle", &u, false).value);
    }

    #[test]
    fn test_feature_probe_number() {
        let json = load_local_json("resources/fixtures/repo.json");
        let fp = FeatureProbe::new_with("secret key".to_string(), json.unwrap());
        let u = FPUser::new("key").with("name", "bob").with("city", "1");

        assert_eq!(fp.number_value("number_toggle", &u, 0.0), 1.0);
        assert_eq!(fp.number_detail("number_toggle", &u, 0.0).value, 1.0);
    }

    #[test]
    fn test_feature_probe_string() {
        let json = load_local_json("resources/fixtures/repo.json");
        let fp = FeatureProbe::new_with("secret key".to_string(), json.unwrap());
        let u = FPUser::new("key").with("name", "bob").with("city", "1");

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
        let u = FPUser::new("key").with("name", "bob").with("city", "1");

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

    #[cfg(feature = "internal")]
    #[test]
    fn test_feature_probe_evaluate_all() {
        let json = load_local_json("resources/fixtures/repo.json");
        let fp = FeatureProbe::new_with("secret key".to_string(), json.unwrap());
        let u = FPUser::new("key").with("name", "bob").with("city", "1");

        let s = fp.all_evaluated_string(&u);
        assert!(s.len() > 10);
        assert!(!s.contains("server_toggle"))
    }

    #[test]
    fn test_feature_probe_none_exist_toggle() {
        let json = load_local_json("resources/fixtures/repo.json");
        let fp = FeatureProbe::new_with("secret key".to_string(), json.unwrap());
        let u = FPUser::new("key");

        assert!(fp.bool_value("none_exist_toggle", &u, true));
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
    use std::fs;
    use std::path::PathBuf;
    use std::string::String;

    #[allow(dead_code)]
    pub(crate) fn load_tests_json(json_str: &str) -> Result<Tests, FPError> {
        serde_json::from_str::<Tests>(json_str).map_err(|e| FPError::JsonError(e.to_string()))
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
    pub struct ExpectResult {
        pub(crate) value: Value,
        pub(crate) reason: Option<String>,
        pub(crate) rule_index: Option<usize>,
        pub(crate) condition_index: Option<usize>,
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

                let mut user = FPUser::new(case.user.key.clone());
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

    fn assert_detail<T: std::default::Default>(case: &Case, ret: FPDetail<T>) {
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
