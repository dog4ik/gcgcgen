//! The inbound request envelope.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The buckets reactivepay sends on every call.
///
/// The buckets stay untyped here on purpose: which keys appear in each is
/// per-integration configuration, declared by the integration's settings
/// schema and published to the platform as a [`crate::Manifest`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConnectInput {
    #[serde(default)]
    pub payment: Value,
    #[serde(default)]
    pub refund: Value,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub settings: Value,
}

impl ConnectInput {
    /// Convenience accessor for a settings key.
    pub fn setting(&self, name: &str) -> Option<&Value> {
        self.settings.get(name)
    }

    /// The platform's payment identifier
    pub fn token(&self) -> Option<&str> {
        self.payment.get("token")?.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_buckets_default_to_null() {
        let i: ConnectInput = serde_json::from_str(r#"{"payment":{"token":"t"}}"#).unwrap();
        assert_eq!(i.token(), Some("t"));
        assert_eq!(i.params, Value::Null);
        assert_eq!(i.settings, Value::Null);
        assert_eq!(i.setting("client_id"), None);
    }

    #[test]
    fn reads_settings() {
        let i: ConnectInput = serde_json::from_str(r#"{"settings":{"client_id":"cid"}}"#).unwrap();
        assert_eq!(i.setting("client_id"), Some(&json!("cid")));
    }
}
