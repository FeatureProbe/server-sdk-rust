use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct FPUser {
    key: RwLock<Option<String>>,
    attrs: HashMap<String, String>,
}

impl FPUser {
    pub fn new() -> Self {
        let key = RwLock::new(None);
        FPUser {
            key,
            ..Default::default()
        }
    }

    pub fn stable_rollout(mut self, key: String) -> Self {
        self.key = RwLock::new(Some(key));
        self
    }

    pub fn with<T: Into<String>>(mut self, k: T, v: T) -> Self {
        self.attrs.insert(k.into(), v.into());
        self
    }

    pub fn with_attrs(mut self, attrs: impl Iterator<Item = (String, String)>) -> Self {
        self.attrs.extend(attrs);
        self
    }

    pub fn get(&self, k: &str) -> Option<&String> {
        self.attrs.get(k)
    }

    pub fn get_all(&self) -> &HashMap<String, String> {
        &self.attrs
    }

    pub fn key(&self) -> String {
        let key = {
            let key = self.key.read();
            (*key).clone()
        };
        match key {
            Some(key) => key,
            None => {
                let mut guard = self.key.write();
                let key = generate_key();
                *guard = Some(key.clone());
                key
            }
        }
    }
}

fn generate_key() -> String {
    let start = SystemTime::now();
    let since_the_epoch = start
        .duration_since(UNIX_EPOCH)
        .expect("Time went before epoch");
    format!("{}", since_the_epoch.as_micros())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_with() {
        let u = FPUser::new().with("name", "bob").with("phone", "123");
        assert_eq!(u.get("name"), Some(&"bob".to_owned()));
        assert_eq!(u.get("phone"), Some(&"123".to_owned()));
        assert_eq!(u.get_all().len(), 2);
        // test generated key not change
        assert_eq!(u.key(), u.key());
    }

    #[test]
    fn test_user_with_attrs() {
        let mut attrs: HashMap<String, String> = Default::default();
        attrs.insert("name".to_owned(), "bob".to_owned());
        attrs.insert("phone".to_owned(), "123".to_owned());
        let u = FPUser::new().with_attrs(attrs.into_iter());
        assert_eq!(u.get_all().len(), 2);
    }
}
