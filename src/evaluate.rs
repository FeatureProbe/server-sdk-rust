use crate::user::FPUser;
use crate::FPError;
use crate::{unix_timestamp, PrerequisiteError};
use byteorder::{BigEndian, ReadBytesExt};
use regex::Regex;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::Digest;
use std::string::String;
use std::{collections::HashMap, str::FromStr};
use tracing::{info, warn};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase")]
pub enum Serve {
    Select(usize),
    Split(Distribution),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Variation {
    pub value: Value,
    pub index: usize,
}

impl Serve {
    pub fn select_variation(&self, eval_param: &EvalParams) -> Result<Variation, FPError> {
        let variations = eval_param.variations;
        let index = match self {
            Serve::Select(i) => *i,
            Serve::Split(distribution) => distribution.find_index(eval_param)?,
        };

        match variations.get(index) {
            None if eval_param.is_detail => Err(FPError::EvalDetailError(format!(
                "index {} overflow, variations count is {}",
                index,
                variations.len()
            ))),
            None => Err(FPError::EvalError),
            Some(v) => Ok(Variation {
                value: v.clone(),
                index,
            }),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
struct BucketRange((u32, u32));

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Distribution {
    distribution: Vec<Vec<BucketRange>>,
    bucket_by: Option<String>,
    salt: Option<String>,
}

impl Distribution {
    pub fn find_index(&self, eval_param: &EvalParams) -> Result<usize, FPError> {
        let user = eval_param.user;

        let hash_key = match &self.bucket_by {
            None => user.key(),
            Some(custom_key) => match user.get(custom_key) {
                None if eval_param.is_detail => {
                    return Err(FPError::EvalDetailError(format!(
                        "User with key:{:?} does not have attribute named: [{}]",
                        user.key(),
                        custom_key
                    )));
                }
                None => return Err(FPError::EvalError),
                Some(value) => value.to_owned(),
            },
        };

        let salt = match &self.salt {
            Some(s) if !s.is_empty() => s,
            _ => eval_param.key,
        };

        let bucket_index = salt_hash(&hash_key, salt, 10000);

        let variation = self.distribution.iter().position(|ranges| {
            ranges.iter().any(|pair| {
                let (lower, upper) = pair.0;
                lower <= bucket_index && bucket_index < upper
            })
        });

        match variation {
            None if eval_param.is_detail => Err(FPError::EvalDetailError(
                "not find hash_bucket in distribution.".to_string(),
            )),
            None => Err(FPError::EvalError),
            Some(index) => Ok(index),
        }
    }
}

fn salt_hash(key: &str, salt: &str, bucket_size: u64) -> u32 {
    let size = 4;
    let mut hasher = sha1::Sha1::new();
    let data = format!("{key}{salt}");
    hasher.update(data);
    let hax_value = hasher.finalize();
    let mut v = Vec::with_capacity(size);
    for i in (hax_value.len() - size)..hax_value.len() {
        v.push(hax_value[i]);
    }
    let mut v = v.as_slice();
    let value = v.read_u32::<BigEndian>().expect("can not be here");
    value % bucket_size as u32
}

pub struct EvalParams<'a> {
    key: &'a str,
    is_detail: bool,
    user: &'a FPUser,
    variations: &'a [Value],
    segment_repo: &'a HashMap<String, Segment>,
    toggle_repo: &'a HashMap<String, Toggle>,
    debug_until_time: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EvalDetail<T> {
    pub value: Option<T>,
    pub rule_index: Option<usize>,
    pub track_access_events: Option<bool>,
    pub debug_until_time: Option<u64>,
    pub last_modified: Option<u64>,
    pub variation_index: Option<usize>,
    pub version: Option<u64>,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Prerequisites {
    pub key: String,
    pub value: Value,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Toggle {
    key: String,
    enabled: bool,
    track_access_events: Option<bool>,
    last_modified: Option<u64>,
    version: u64,
    for_client: bool,
    disabled_serve: Serve,
    default_serve: Serve,
    rules: Vec<Rule>,
    variations: Vec<Value>,
    prerequisites: Option<Vec<Prerequisites>>,
}

impl Toggle {
    pub fn eval(
        &self,
        user: &FPUser,
        segment_repo: &HashMap<String, Segment>,
        toggle_repo: &HashMap<String, Toggle>,
        is_detail: bool,
        deep: u8,
        debug_until_time: Option<u64>,
    ) -> EvalDetail<Value> {
        let eval_param = EvalParams {
            user,
            segment_repo,
            toggle_repo,
            key: &self.key,
            is_detail,
            variations: &self.variations,
            debug_until_time,
        };

        match self.do_eval(&eval_param, deep) {
            Ok(eval) => eval,
            Err(e) => self.disabled_variation(&eval_param, Some(e.to_string())),
        }
    }

    fn do_eval(
        &self,
        eval_param: &EvalParams,
        max_depth: u8,
    ) -> Result<EvalDetail<Value>, PrerequisiteError> {
        if !self.enabled {
            return Ok(self.disabled_variation(eval_param, None))
        }

        if !self.meet_prerequisite(eval_param, max_depth)? {
            return Ok(self.disabled_variation(eval_param, Some(
                "Prerequisite not match".to_owned())));
        }

        for (i, rule) in self.rules.iter().enumerate() {
            match rule.serve_variation(eval_param) {
                Ok(v) => {
                    if v.is_some() {
                        return Ok(self.serve_variation(
                            v,
                            format!("rule {i}"),
                            Some(i),
                            eval_param.debug_until_time,
                        ));
                    }
                }
                Err(e) => {
                    return Ok(self.serve_variation(
                        None,
                        format!("{e:?}"),
                        Some(i),
                        eval_param.debug_until_time,
                    ));
                }
            }
        }

        Ok(self.default_variation(eval_param, None))
    }

    fn meet_prerequisite(
        &self,
        eval_param: &EvalParams,
        deep: u8,
    ) -> Result<bool, PrerequisiteError> {
        if deep == 0 {
            return Err(PrerequisiteError::DepthOverflow);
        }

        if let Some(ref prerequisites) = self.prerequisites {
            for pre in prerequisites {
                let eval = match eval_param.toggle_repo.get(&pre.key) {
                    None => {
                        return Err(PrerequisiteError::NotExist(pre.key.to_string()));
                    }
                    Some(t) => t.do_eval(
                        &EvalParams {
                            key: &t.key,
                            variations: &t.variations,
                            is_detail: eval_param.is_detail,
                            user: eval_param.user,
                            segment_repo: eval_param.segment_repo,
                            toggle_repo: eval_param.toggle_repo,
                            debug_until_time: eval_param.debug_until_time,
                        },
                        deep - 1,
                    )?,
                };

                match eval.value {
                    Some(v) if v == pre.value => continue,
                    _ => return Ok(false),
                }
            }
            return Ok(true);
        }
        Ok(true)
    }

    fn serve_variation(
        &self,
        v: Option<Variation>,
        reason: String,
        rule_index: Option<usize>,
        debug_until_time: Option<u64>,
    ) -> EvalDetail<Value> {
        EvalDetail {
            variation_index: v.as_ref().map(|v| v.index),
            value: v.map(|v| v.value),
            version: Some(self.version),
            track_access_events: self.track_access_events,
            debug_until_time,
            last_modified: self.last_modified,
            rule_index,
            reason,
        }
    }

    fn default_variation(
        &self,
        eval_param: &EvalParams,
        reason: Option<String>,
    ) -> EvalDetail<Value> {
        return self.fixed_variation(
            &self.default_serve,
            eval_param,
            "default.".to_owned(),
            reason,
        );
    }

    fn disabled_variation(
        &self,
        eval_param: &EvalParams,
        reason: Option<String>,
    ) -> EvalDetail<Value> {
        return self.fixed_variation(
            &self.disabled_serve,
            eval_param,
            "disabled.".to_owned(),
            reason,
        );
    }

    fn fixed_variation(
        &self,
        serve: &Serve,
        eval_param: &EvalParams,
        default_reason: String,
        reason: Option<String>,
    ) -> EvalDetail<Value> {
        match serve.select_variation(eval_param) {
            Ok(v) => self.serve_variation(
                Some(v),
                concat_reason(default_reason, reason),
                None,
                eval_param.debug_until_time,
            ),
            Err(e) => self.serve_variation(
                None,
                concat_reason(format!("{e:?}"), reason),
                None,
                eval_param.debug_until_time,
            ),
        }
    }

    pub fn track_access_events(&self) -> bool {
        self.track_access_events.unwrap_or(false)
    }

    #[cfg(feature = "internal")]
    pub fn is_for_client(&self) -> bool {
        self.for_client
    }

    #[cfg(feature = "internal")]
    pub fn all_segment_ids(&self) -> Vec<&str> {
        let mut sids: Vec<&str> = Vec::new();
        for r in &self.rules {
            for c in &r.conditions {
                if c.r#type == ConditionType::Segment {
                    sids.push(&c.subject)
                }
            }
        }
        sids
    }

    pub fn new_for_test(key: String, val: Value) -> Self {
        Self {
            key,
            enabled: true,
            track_access_events: None,
            last_modified: None,
            default_serve: Serve::Select(0),
            disabled_serve: Serve::Select(0),
            variations: vec![val],
            version: 0,
            for_client: false,
            rules: vec![],
            prerequisites: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
struct SegmentRule {
    conditions: Vec<Condition>,
}

impl SegmentRule {
    pub fn allow(&self, user: &FPUser) -> bool {
        for c in &self.conditions {
            if c.meet(user, None) {
                return true;
            }
        }
        false
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct DefaultRule {
    pub serve: Serve,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
struct Rule {
    serve: Serve,
    conditions: Vec<Condition>,
}

impl Rule {
    pub fn serve_variation(&self, eval_param: &EvalParams) -> Result<Option<Variation>, FPError> {
        let user = eval_param.user;
        let segment_repo = eval_param.segment_repo;
        match self
            .conditions
            .iter()
            .all(|c| c.meet(user, Some(segment_repo)))
        {
            true => Ok(Some(self.serve.select_variation(eval_param)?)),
            false => Ok(None),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase")]
enum ConditionType {
    String,
    Segment,
    Datetime,
    Number,
    Semver,
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
struct Condition {
    r#type: ConditionType,
    #[serde(default)]
    subject: String,
    predicate: String,
    objects: Vec<String>,
}

impl Condition {
    pub fn meet(&self, user: &FPUser, segment_repo: Option<&HashMap<String, Segment>>) -> bool {
        match &self.r#type {
            ConditionType::String => self.match_string(user, &self.predicate),
            ConditionType::Segment => self.match_segment(user, &self.predicate, segment_repo),
            ConditionType::Number => self.match_ordering::<f64>(user, &self.predicate),
            ConditionType::Semver => self.match_ordering::<Version>(user, &self.predicate),
            ConditionType::Datetime => self.match_timestamp(user, &self.predicate),
            _ => false,
        }
    }

    fn match_segment(
        &self,
        user: &FPUser,
        predicate: &str,
        segment_repo: Option<&HashMap<String, Segment>>,
    ) -> bool {
        match segment_repo {
            None => false,
            Some(repo) => match predicate {
                "is in" => self.user_in_segments(user, repo),
                "is not in" => !self.user_in_segments(user, repo),
                _ => false,
            },
        }
    }

    fn match_string(&self, user: &FPUser, predicate: &str) -> bool {
        if let Some(c) = user.get(&self.subject) {
            return match predicate {
                "is one of" => self.do_match::<String>(c, |c, o| c.eq(o)),
                "ends with" => self.do_match::<String>(c, |c, o| c.ends_with(o)),
                "starts with" => self.do_match::<String>(c, |c, o| c.starts_with(o)),
                "contains" => self.do_match::<String>(c, |c, o| c.contains(o)),
                "matches regex" => {
                    self.do_match::<String>(c, |c, o| match Regex::new(o) {
                        Ok(re) => re.is_match(c),
                        Err(_) => false, // invalid regex should be checked when load config
                    })
                }
                "is not any of" => !self.match_string(user, "is one of"),
                "does not end with" => !self.match_string(user, "ends with"),
                "does not start with" => !self.match_string(user, "starts with"),
                "does not contain" => !self.match_string(user, "contains"),
                "does not match regex" => !self.match_string(user, "matches regex"),
                _ => {
                    info!("unknown predicate {}", predicate);
                    false
                }
            };
        }
        info!("user attr missing: {}", self.subject);
        false
    }

    fn match_ordering<T: FromStr + PartialOrd>(&self, user: &FPUser, predicate: &str) -> bool {
        if let Some(c) = user.get(&self.subject) {
            let c: T = match c.parse() {
                Ok(v) => v,
                Err(_) => return false,
            };
            return match predicate {
                "=" => self.do_match::<T>(&c, |c, o| c.eq(o)),
                "!=" => !self.match_ordering::<T>(user, "="),
                ">" => self.do_match::<T>(&c, |c, o| c.gt(o)),
                ">=" => self.do_match::<T>(&c, |c, o| c.ge(o)),
                "<" => self.do_match::<T>(&c, |c, o| c.lt(o)),
                "<=" => self.do_match::<T>(&c, |c, o| c.le(o)),
                _ => {
                    info!("unknown predicate {}", predicate);
                    false
                }
            };
        }
        info!("user attr missing: {}", self.subject);
        false
    }

    fn match_timestamp(&self, user: &FPUser, predicate: &str) -> bool {
        let c: u128 = match user.get(&self.subject) {
            Some(v) => match v.parse() {
                Ok(v) => v,
                Err(_) => return false,
            },
            None => unix_timestamp() / 1000,
        };
        match predicate {
            "after" => self.do_match::<u128>(&c, |c, o| c.ge(o)),
            "before" => self.do_match::<u128>(&c, |c, o| c.lt(o)),
            _ => {
                info!("unknown predicate {}", predicate);
                false
            }
        }
    }

    fn do_match<T: FromStr>(&self, t: &T, f: fn(&T, &T) -> bool) -> bool {
        self.objects
            .iter()
            .map(|o| match o.parse::<T>() {
                Ok(o) => f(t, &o),
                Err(_) => false,
            })
            .any(|x| x)
    }

    fn user_in_segments(&self, user: &FPUser, repo: &HashMap<String, Segment>) -> bool {
        for segment_key in &self.objects {
            match repo.get(segment_key) {
                Some(segment) => {
                    if segment.contains(user) {
                        return true;
                    }
                }
                None => warn!("segment not found {}", segment_key),
            }
        }
        false
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    unique_id: String,
    version: u64,
    rules: Vec<SegmentRule>,
}

impl Segment {
    pub fn contains(&self, user: &FPUser) -> bool {
        for rule in &self.rules {
            if rule.allow(user) {
                return true;
            }
        }
        false
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub segments: HashMap<String, Segment>,
    pub toggles: HashMap<String, Toggle>,
    pub events: Option<Value>,
    // TODO: remove option next release
    pub version: Option<u128>,
    pub debug_until_time: Option<u64>,
}

impl Default for Repository {
    fn default() -> Self {
        Repository {
            segments: Default::default(),
            toggles: Default::default(),
            events: Default::default(),
            version: Some(0),
            debug_until_time: None,
        }
    }
}

fn validate_toggle(_toggle: &Toggle) -> Result<(), FPError> {
    //TODO: validate toggle segment unique id exists
    //TODO: validate serve index and buckets size less than variations length
    //TODO: validate rules list last one if default rule (no condition just serve)
    //TODO: validate bucket is full range
    Ok(())
}

#[allow(dead_code)]
pub fn load_json(json_str: &str) -> Result<Repository, FPError> {
    let repo = serde_json::from_str::<Repository>(json_str)
        .map_err(|e| FPError::JsonError(json_str.to_owned(), e));
    if let Ok(repo) = &repo {
        for t in repo.toggles.values() {
            validate_toggle(t)?
        }
    }
    repo
}

fn concat_reason(reason1: String, reason2: Option<String>) -> String {
    if let Some(reason2) = reason2 {
        return format!("{reason1}. {reason2}.");
    }
    format!("{reason1}.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::{self, assert_relative_eq};
    use std::fs;
    use std::path::PathBuf;

    const MAX_DEEP: u8 = 20;

    #[test]
    fn test_load() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/fixtures/repo.json");
        let json_str = fs::read_to_string(path).unwrap();
        let repo = load_json(&json_str);
        assert!(repo.is_ok());
    }

    #[test]
    fn test_load_invalid_json() {
        let json_str = "{invalid_json}";
        let repo = load_json(json_str);
        assert!(repo.is_err());
    }

    #[test]
    fn test_salt_hash() {
        let bucket = salt_hash("key", "salt", 10000);
        assert_eq!(2647, bucket);
    }

    #[test]
    fn test_segment_condition() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/fixtures/repo.json");
        let json_str = fs::read_to_string(path).unwrap();
        let repo = load_json(&json_str);
        assert!(repo.is_ok());
        let repo = repo.unwrap();

        let user = FPUser::new().with("city", "4");
        let toggle = repo.toggles.get("json_toggle").unwrap();
        let r = toggle.eval(&user, &repo.segments, &repo.toggles, false, MAX_DEEP, None);
        let r = r.value.unwrap();
        let r = r.as_object().unwrap();
        assert!(r.get("variation_1").is_some());
    }

    #[test]
    fn test_not_in_segment_condition() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/fixtures/repo.json");
        let json_str = fs::read_to_string(path).unwrap();
        let repo = load_json(&json_str);
        assert!(repo.is_ok());
        let repo = repo.unwrap();

        let user = FPUser::new().with("city", "100");
        let toggle = repo.toggles.get("not_in_segment").unwrap();
        let r = toggle.eval(&user, &repo.segments, &repo.toggles, false, MAX_DEEP, None);
        let r = r.value.unwrap();
        let r = r.as_object().unwrap();
        assert!(r.get("not_in").is_some());
    }

    #[test]
    fn test_multi_condition() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/fixtures/repo.json");
        let json_str = fs::read_to_string(path).unwrap();
        let repo = load_json(&json_str);
        assert!(repo.is_ok());
        let repo = repo.unwrap();

        let user = FPUser::new().with("city", "1").with("os", "linux");
        let toggle = repo.toggles.get("multi_condition_toggle").unwrap();
        let r = toggle.eval(&user, &repo.segments, &repo.toggles, false, MAX_DEEP, None);
        let r = r.value.unwrap();
        let r = r.as_object().unwrap();
        assert!(r.get("variation_0").is_some());

        let user = FPUser::new().with("os", "linux");
        let toggle = repo.toggles.get("multi_condition_toggle").unwrap();
        let r = toggle.eval(&user, &repo.segments, &repo.toggles, false, MAX_DEEP, None);
        assert!(r.reason.starts_with("default"));

        let user = FPUser::new().with("city", "1");
        let toggle = repo.toggles.get("multi_condition_toggle").unwrap();
        let r = toggle.eval(&user, &repo.segments, &repo.toggles, false, MAX_DEEP, None);
        assert!(r.reason.starts_with("default"));
    }

    #[test]
    fn test_distribution_condition() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/fixtures/repo.json");
        let json_str = fs::read_to_string(path).unwrap();
        let repo = load_json(&json_str);
        assert!(repo.is_ok());
        let repo = repo.unwrap();

        let total = 10000;
        let users = gen_users(total, false);
        let toggle = repo.toggles.get("json_toggle").unwrap();
        let mut variation_0 = 0;
        let mut variation_1 = 0;
        let mut variation_2 = 0;
        for user in &users {
            let r = toggle.eval(&user, &repo.segments, &repo.toggles, false, MAX_DEEP, None);
            let r = r.value.unwrap();
            let r = r.as_object().unwrap();
            if r.get("variation_0").is_some() {
                variation_0 += 1;
            } else if r.get("variation_1").is_some() {
                variation_1 += 1;
            } else if r.get("variation_2").is_some() {
                variation_2 += 1;
            }
        }

        let rate0 = variation_0 as f64 / total as f64;
        assert_relative_eq!(0.3333, rate0, max_relative = 0.05);
        let rate1 = variation_1 as f64 / total as f64;
        assert_relative_eq!(0.3333, rate1, max_relative = 0.05);
        let rate2 = variation_2 as f64 / total as f64;
        assert_relative_eq!(0.3333, rate2, max_relative = 0.05);
    }

    #[test]
    fn test_disabled_toggle() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/fixtures/repo.json");
        let json_str = fs::read_to_string(path).unwrap();
        let repo = load_json(&json_str);
        assert!(repo.is_ok());
        let repo = repo.unwrap();

        let user = FPUser::new().with("city", "100");
        let toggle = repo.toggles.get("disabled_toggle").unwrap();
        let r = toggle.eval(&user, &repo.segments, &repo.toggles, false, MAX_DEEP, None);
        assert!(r
            .value
            .unwrap()
            .as_object()
            .unwrap()
            .get("disabled_key")
            .is_some());
    }

    #[test]
    fn test_prerequisite_toggle() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/fixtures/repo.json");
        let json_str = fs::read_to_string(path).unwrap();
        let repo = load_json(&json_str);
        assert!(repo.is_ok());
        let repo = repo.unwrap();

        let user = FPUser::new().with("city", "4");

        let toggle = repo.toggles.get("prerequisite_toggle").unwrap();
        let r = toggle.eval(&user, &repo.segments, &repo.toggles, false, MAX_DEEP, None);

        assert!(r.value.unwrap().as_object().unwrap().get("2").is_some());
    }

    #[test]
    fn test_prerequisite_not_exist_should_return_disabled_variation() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/fixtures/repo.json");
        let json_str = fs::read_to_string(path).unwrap();
        let repo = load_json(&json_str);
        assert!(repo.is_ok());
        let repo = repo.unwrap();

        let user = FPUser::new().with("city", "4");

        let toggle = repo.toggles.get("prerequisite_toggle_not_exist").unwrap();
        let r = toggle.eval(&user, &repo.segments, &repo.toggles, false, MAX_DEEP, None);

        assert!(r.value.unwrap().as_object().unwrap().get("0").is_some());
        assert!(r.reason.contains("not exist"));
    }

    #[test]
    fn test_prerequisite_not_match_should_return_disabled_variation() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/fixtures/repo.json");
        let json_str = fs::read_to_string(path).unwrap();
        let repo = load_json(&json_str);
        assert!(repo.is_ok());
        let repo = repo.unwrap();

        let user = FPUser::new().with("city", "4");

        let toggle = repo.toggles.get("prerequisite_toggle_not_match").unwrap();
        let r = toggle.eval(&user, &repo.segments, &repo.toggles, false, MAX_DEEP, None);

        assert!(r.value.unwrap().as_object().unwrap().get("0").is_some());
        assert!(r.reason.contains("disabled."));
    }

    #[test]
    fn test_prerequisite_depth_overflow_should_return_disabled_variation() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/fixtures/repo.json");
        let json_str = fs::read_to_string(path).unwrap();
        let repo = load_json(&json_str);
        assert!(repo.is_ok());
        let repo = repo.unwrap();

        let user = FPUser::new().with("city", "4");

        let toggle = repo.toggles.get("prerequisite_toggle").unwrap();
        let r = toggle.eval(&user, &repo.segments, &repo.toggles, false, 1, None);

        assert!(r.value.unwrap().as_object().unwrap().get("0").is_some());
        assert!(r.reason.contains("depth overflow"));
    }

    fn gen_users(num: usize, random: bool) -> Vec<FPUser> {
        let mut users = Vec::with_capacity(num);
        for i in 0..num {
            let key: u64 = if random { rand::random() } else { i as u64 };
            let u = FPUser::new()
                .with("city", "100")
                .stable_rollout(format!("{}", key));
            users.push(u);
        }
        users
    }
}

#[cfg(test)]
mod distribution_tests {
    use super::*;

    #[test]
    fn test_distribution_in_exact_bucket() {
        let distribution = Distribution {
            distribution: vec![
                vec![BucketRange((0, 2647))],
                vec![BucketRange((2647, 2648))],
                vec![BucketRange((2648, 10000))],
            ],
            bucket_by: Some("name".to_string()),
            salt: Some("salt".to_string()),
        };

        let user_bucket_by_name = FPUser::new().with("name", "key");

        let params = EvalParams {
            key: "not care",
            is_detail: true,
            user: &user_bucket_by_name,
            variations: &[],
            segment_repo: &Default::default(),
            toggle_repo: &Default::default(),
            debug_until_time: None,
        };
        let result = distribution.find_index(&params);

        assert_eq!(1, result.unwrap_or_default());
    }

    #[test]
    fn test_distribution_in_none_bucket() {
        let distribution = Distribution {
            distribution: vec![
                vec![BucketRange((0, 2647))],
                vec![BucketRange((2648, 10000))],
            ],
            bucket_by: Some("name".to_string()),
            salt: Some("salt".to_string()),
        };

        let user_bucket_by_name = FPUser::new().with("name", "key");

        let params = EvalParams {
            key: "not care",
            is_detail: true,
            user: &user_bucket_by_name,
            variations: &[],
            segment_repo: &Default::default(),
            toggle_repo: &Default::default(),
            debug_until_time: None,
        };
        let result = distribution.find_index(&params);

        assert!(format!("{:?}", result.expect_err("error")).contains("not find hash_bucket"));

        let params_no_detail = EvalParams {
            key: "not care",
            is_detail: false,
            user: &user_bucket_by_name,
            variations: &[],
            segment_repo: &Default::default(),
            toggle_repo: &Default::default(),
            debug_until_time: None,
        };
        let result = distribution.find_index(&params_no_detail);
        assert!(result.is_err());
    }

    #[test]
    fn test_select_variation_fail() {
        let distribution = Distribution {
            distribution: vec![
                vec![BucketRange((0, 5000))],
                vec![BucketRange((5000, 10000))],
            ],
            bucket_by: Some("name".to_string()),
            salt: Some("salt".to_string()),
        };
        let serve = Serve::Split(distribution);

        let user_with_no_name = FPUser::new();

        let params = EvalParams {
            key: "",
            is_detail: true,
            user: &user_with_no_name,
            variations: &[
                Value::String("a".to_string()),
                Value::String("b".to_string()),
            ],
            segment_repo: &Default::default(),
            toggle_repo: &Default::default(),
            debug_until_time: None,
        };

        let result = serve.select_variation(&params).expect_err("e");

        assert!(format!("{:?}", result).contains("does not have attribute"));
    }
}

#[cfg(test)]
mod condition_tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    const MAX_DEEP: u8 = 20;

    #[test]
    fn test_unknown_condition() {
        let json_str = r#"
        {
            "type": "new_type",
            "subject": "new_subject",
            "predicate": ">",
            "objects": []
        }
        "#;

        let condition = serde_json::from_str::<Condition>(json_str);
        assert!(condition.is_ok());
        let condition = condition.unwrap();
        assert_eq!(condition.r#type, ConditionType::Unknown);
    }

    #[test]
    fn test_match_is_one_of() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "is one of".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "world");
        assert!(condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_not_match_is_one_of() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "is one of".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "not_in");

        assert!(!condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_user_miss_key_is_not_one_of() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "is not one of".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new();

        assert!(!condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_match_is_not_any_of() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "is not any of".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "welcome");
        assert!(condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_not_match_is_not_any_of() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "is not any of".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "not_in");

        assert!(condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_match_ends_with() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "ends with".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "bob world");

        assert!(condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_dont_match_ends_with() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "ends with".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "bob");

        assert!(!condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_match_does_not_end_with() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "does not end with".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "bob");

        assert!(condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_not_match_does_not_end_with() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "does not end with".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "bob world");

        assert!(!condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_match_starts_with() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "starts with".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "world bob");

        assert!(condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_not_match_starts_with() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "ends with".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "bob");

        assert!(!condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_match_does_not_start_with() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "does not start with".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "bob");

        assert!(condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_not_match_does_not_start_with() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "does not start with".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "world bob");

        assert!(!condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_match_contains() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "contains".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "alice world bob");

        assert!(condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_not_match_contains() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "contains".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "alice bob");

        assert!(!condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_match_not_contains() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "does not contain".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "alice bob");

        assert!(condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_not_match_not_contains() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "does not contain".to_string(),
            objects: vec![String::from("hello"), String::from("world")],
        };

        let user = FPUser::new().with("name", "alice world bob");

        assert!(!condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_match_regex() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "matches regex".to_string(),
            objects: vec![String::from("hello"), String::from("world.*")],
        };

        let user = FPUser::new().with("name", "alice world bob");

        assert!(condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_match_regex_first_object() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "matches regex".to_string(),
            objects: vec![String::from(r"hello\d"), String::from("world.*")],
        };

        let user = FPUser::new().with("name", "alice orld bob hello3");

        assert!(condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_not_match_regex() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "matches regex".to_string(),
            objects: vec![String::from(r"hello\d"), String::from("world.*")],
        };

        let user = FPUser::new().with("name", "alice orld bob hello");

        assert!(!condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_match_not_match_regex() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "does not match regex".to_string(),
            objects: vec![String::from(r"hello\d"), String::from("world.*")],
        };

        let user = FPUser::new().with("name", "alice orld bob hello");

        assert!(condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_invalid_regex_condition() {
        let condition = Condition {
            r#type: ConditionType::String,
            subject: "name".to_string(),
            predicate: "matches regex".to_string(),
            objects: vec![String::from("\\\\\\")],
        };

        let user = FPUser::new().with("name", "\\\\\\");

        assert!(!condition.match_string(&user, &condition.predicate));
    }

    #[test]
    fn test_match_equal_string() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/fixtures/repo.json");
        let json_str = fs::read_to_string(path).unwrap();
        let repo = load_json(&json_str);
        assert!(repo.is_ok());
        let repo = repo.unwrap();

        let user = FPUser::new().with("city", "1");
        let toggle = repo.toggles.get("json_toggle").unwrap();
        let r = toggle.eval(&user, &repo.segments, &repo.toggles, false, MAX_DEEP, None);
        let r = r.value.unwrap();
        let r = r.as_object().unwrap();
        assert!(r.get("variation_0").is_some());
    }

    #[test]
    fn test_segment_deserialize() {
        let json_str = r#"
        {
            "type":"segment",
            "predicate":"is in",
            "objects":[ "segment1","segment2"]
        }
        "#;

        let segment = serde_json::from_str::<Condition>(json_str)
            .map_err(|e| FPError::JsonError(json_str.to_owned(), e));
        assert!(segment.is_ok())
    }

    #[test]
    fn test_semver_condition() {
        let mut condition = Condition {
            r#type: ConditionType::Semver,
            subject: "version".to_owned(),
            objects: vec!["1.0.0".to_owned(), "2.0.0".to_owned()],
            predicate: "=".to_owned(),
        };

        let user = FPUser::new().with("version".to_owned(), "1.0.0".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("version".to_owned(), "2.0.0".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("version".to_owned(), "3.0.0".to_owned());
        assert!(!condition.meet(&user, None));

        condition.predicate = "!=".to_owned();
        let user = FPUser::new().with("version".to_owned(), "1.0.0".to_owned());
        assert!(!condition.meet(&user, None));
        let user = FPUser::new().with("version".to_owned(), "2.0.0".to_owned());
        assert!(!condition.meet(&user, None));
        let user = FPUser::new().with("version".to_owned(), "0.1.0".to_owned());
        assert!(condition.meet(&user, None));

        condition.predicate = ">".to_owned();
        let user = FPUser::new().with("version".to_owned(), "2.0.0".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("version".to_owned(), "3.0.0".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("version".to_owned(), "0.1.0".to_owned());
        assert!(!condition.meet(&user, None));

        condition.predicate = ">=".to_owned();
        let user = FPUser::new().with("version".to_owned(), "1.0.0".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("version".to_owned(), "2.0.0".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("version".to_owned(), "3.0.0".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("version".to_owned(), "0.1.0".to_owned());
        assert!(!condition.meet(&user, None));

        condition.predicate = "<".to_owned();
        let user = FPUser::new().with("version".to_owned(), "1.0.0".to_owned()); // < 2.0.0
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("version".to_owned(), "2.0.0".to_owned());
        assert!(!condition.meet(&user, None));
        let user = FPUser::new().with("version".to_owned(), "3.0.0".to_owned());
        assert!(!condition.meet(&user, None));

        condition.predicate = "<=".to_owned();
        let user = FPUser::new().with("version".to_owned(), "1.0.0".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("version".to_owned(), "2.0.0".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("version".to_owned(), "0.1.0".to_owned());
        assert!(condition.meet(&user, None));

        let user = FPUser::new().with("version".to_owned(), "a".to_owned());
        assert!(!condition.meet(&user, None));
    }

    #[test]
    fn test_number_condition() {
        let mut condition = Condition {
            r#type: ConditionType::Number,
            subject: "price".to_owned(),
            objects: vec!["10".to_owned(), "100".to_owned()],
            predicate: "=".to_owned(),
        };

        let user = FPUser::new().with("price".to_owned(), "10".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("price".to_owned(), "100".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("price".to_owned(), "0".to_owned());
        assert!(!condition.meet(&user, None));

        condition.predicate = "!=".to_owned();
        let user = FPUser::new().with("price".to_owned(), "10".to_owned());
        assert!(!condition.meet(&user, None));
        let user = FPUser::new().with("price".to_owned(), "100".to_owned());
        assert!(!condition.meet(&user, None));
        let user = FPUser::new().with("price".to_owned(), "0".to_owned());
        assert!(condition.meet(&user, None));

        condition.predicate = ">".to_owned();
        let user = FPUser::new().with("price".to_owned(), "11".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("price".to_owned(), "10".to_owned());
        assert!(!condition.meet(&user, None));

        condition.predicate = ">=".to_owned();
        let user = FPUser::new().with("price".to_owned(), "10".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("price".to_owned(), "11".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("price".to_owned(), "100".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("price".to_owned(), "0".to_owned());
        assert!(!condition.meet(&user, None));

        condition.predicate = "<".to_owned();
        let user = FPUser::new().with("price".to_owned(), "1".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("price".to_owned(), "10".to_owned()); // < 100
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("price".to_owned(), "100".to_owned()); // < 100
        assert!(!condition.meet(&user, None));

        condition.predicate = "<=".to_owned();
        let user = FPUser::new().with("price".to_owned(), "1".to_owned());
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("price".to_owned(), "10".to_owned()); // < 100
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("price".to_owned(), "100".to_owned()); // < 100
        assert!(condition.meet(&user, None));

        let user = FPUser::new().with("price".to_owned(), "a".to_owned());
        assert!(!condition.meet(&user, None));
    }

    #[test]
    fn test_datetime_condition() {
        let now_ts = unix_timestamp() / 1000;
        let mut condition = Condition {
            r#type: ConditionType::Datetime,
            subject: "ts".to_owned(),
            objects: vec![format!("{}", now_ts)],
            predicate: "after".to_owned(),
        };

        let user = FPUser::new();
        assert!(condition.meet(&user, None));
        let user = FPUser::new().with("ts".to_owned(), format!("{}", now_ts));
        assert!(condition.meet(&user, None));

        condition.predicate = "before".to_owned();
        condition.objects = vec![format!("{}", now_ts + 2)];
        assert!(condition.meet(&user, None));

        let user = FPUser::new().with("ts".to_owned(), "a".to_owned());
        assert!(!condition.meet(&user, None));
    }
}
