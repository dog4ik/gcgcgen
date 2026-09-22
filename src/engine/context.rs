use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

const MAX_ENTRIES_BEFORE_CLEANUP: usize = 256;
const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq)]
pub struct CallbackContext {
    /// The platform's payment token: the forward's URL path segment.
    pub token: String,
    /// Encrypted into the forward's JWT.
    pub merchant_private_key: String,
    /// The merchant's credentials, for the callback's own requests.
    pub settings: Value,
}

#[derive(Debug)]
struct Entry {
    context: Arc<CallbackContext>,
    stored_at: Instant,
}

#[derive(Debug)]
pub struct ContextStore {
    inner: Mutex<HashMap<String, Entry>>,
    ttl: Duration,
}

impl Default for ContextStore {
    fn default() -> Self {
        Self::new(DEFAULT_TTL)
    }
}

impl ContextStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Stores one context under every id the gateway might call back with.
    /// Empty ids are skipped.
    pub fn insert<'a>(
        &self,
        integration_key: &str,
        ids: impl IntoIterator<Item = &'a str>,
        context: CallbackContext,
    ) {
        let context = Arc::new(context);
        let now = Instant::now();
        let mut map = self.inner.lock().expect("context store poisoned");
        if map.len() >= MAX_ENTRIES_BEFORE_CLEANUP {
            map.retain(|_, e| now.duration_since(e.stored_at) < self.ttl);
        }
        for id in ids.into_iter().filter(|id| !id.is_empty()) {
            map.insert(
                key(integration_key, id),
                Entry {
                    context: context.clone(),
                    stored_at: now,
                },
            );
        }
    }

    pub fn get(&self, integration_key: &str, id: &str) -> Option<Arc<CallbackContext>> {
        let map = self.inner.lock().expect("context store poisoned");
        let entry = map.get(&key(integration_key, id))?;
        (entry.stored_at.elapsed() < self.ttl).then(|| entry.context.clone())
    }

    /// Only the sweep tests can see the map's size from outside.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().expect("context store poisoned").len()
    }
}

fn key(integration_key: &str, id: &str) -> String {
    format!("{integration_key}:{id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(token: &str) -> CallbackContext {
        CallbackContext {
            token: token.into(),
            merchant_private_key: "mpk".into(),
            settings: json!({"client_id": "c"}),
        }
    }

    #[test]
    fn found_by_either_id_and_only_within_its_integration() {
        let s = ContextStore::default();
        s.insert("scripay", ["ORD1", "RRN1"], ctx("ORD1"));
        assert_eq!(s.get("scripay", "ORD1").unwrap().token, "ORD1");
        assert_eq!(s.get("scripay", "RRN1").unwrap().token, "ORD1");
        assert!(s.get("other", "ORD1").is_none());
        assert!(s.get("scripay", "missing").is_none());
    }

    #[test]
    fn empty_ids_are_not_stored() {
        let s = ContextStore::default();
        s.insert("g", ["ORD1", ""], ctx("ORD1"));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn expired_entries_are_invisible_and_swept_over_capacity() {
        let s = ContextStore::new(Duration::ZERO);
        s.insert("g", ["ORD1"], ctx("ORD1"));
        assert!(s.get("g", "ORD1").is_none(), "past its TTL");

        // With `ORD1`, this fills the map exactly to capacity.
        for i in 1..MAX_ENTRIES_BEFORE_CLEANUP {
            s.insert("g", [format!("o{i}").as_str()], ctx("x"));
        }
        s.insert("g", ["fresh"], ctx("fresh"));
        assert_eq!(s.len(), 1, "the sweep drops everything stale");
    }
}
