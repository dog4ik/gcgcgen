use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::MethodKind;

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
    #[serde(default)]
    pub method_name: MethodKind,
    #[serde(default)]
    pub processing_url: String,
}

impl ConnectInput {
    pub fn token(&self) -> Option<&str> {
        self.payment.get("token")?.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_buckets_default_to_null() {
        let i: ConnectInput = serde_json::from_str(r#"{"payment":{"token":"t"}}"#).unwrap();
        assert_eq!(i.token(), Some("t"));
        assert_eq!(i.params, Value::Null);
        assert_eq!(i.settings, Value::Null);
    }
}
