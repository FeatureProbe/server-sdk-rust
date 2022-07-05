mod evalutate;
mod feature_probe;
mod sync;
mod user;

pub use crate::evalutate::{load_json, Repository, Segment, Toggle};
pub use crate::feature_probe::{FPConfig, FeatureProbe};
pub use crate::user::FPUser;
use headers::{Error, Header, HeaderName, HeaderValue};
use http::header::AUTHORIZATION;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use thiserror::Error;
pub use url::Url;

lazy_static! {
    pub(crate) static ref USER_AGENT: String = "Rust/".to_owned() + VERSION;
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize, Deserialize, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FPDetail<T: Default + Debug> {
    pub value: T,
    pub rule_index: Option<usize>,
    pub variation_index: Option<usize>,
    pub version: Option<u64>,
    pub reason: String,
}

#[non_exhaustive]
#[derive(Debug, Error)]
pub enum FPError {
    #[error("invalid json: {0}")]
    JsonError(String),
    #[error("invalid url: {0}")]
    UrlError(String),
    #[error("evaluation error")]
    EvalError,
    #[error("evaluation error: {0}")]
    EvalDetailError(String),
}

#[derive(Debug, Deserialize)]
pub struct SdkAuthorization(pub String);

impl SdkAuthorization {
    pub fn encode(&self) -> HeaderValue {
        HeaderValue::from_str(&self.0).expect("valid header value")
    }
}

impl Header for SdkAuthorization {
    fn name() -> &'static HeaderName {
        &AUTHORIZATION
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, Error>
    where
        Self: Sized,
        I: Iterator<Item = &'i HeaderValue>,
    {
        match values.next() {
            Some(v) => match v.to_str() {
                Ok(s) => Ok(SdkAuthorization(s.to_owned())),
                Err(_) => Err(Error::invalid()),
            },
            None => Err(Error::invalid()),
        }
    }

    fn encode<E: Extend<HeaderValue>>(&self, values: &mut E) {
        if let Ok(value) = HeaderValue::from_str(&self.0) {
            values.extend(std::iter::once(value))
        }
    }
}

pub fn unix_timestamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("Time went backwards!")
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    fn test_encode_panic() {
        let v: Vec<u8> = vec![21, 20, 19, 18]; // not visible string
        let s = String::from_utf8(v).unwrap();
        let auth = SdkAuthorization(s);
        let _ = auth.encode();
    }
}
