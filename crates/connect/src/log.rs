//! The interaction log returned inline with every reply.
//!
//! The platform stores these as the audit trail for a transaction, so one
//! entry per outbound call — including the auth call that fetched a token.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoggedRequest {
    pub url: String,
    /// Method, headers, query and body as actually sent, after redaction.
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractionLog {
    /// The integration key, e.g. `scripay`.
    pub gateway: String,
    /// Which call this was: the request's `name`, e.g. `auth`, `collection`.
    pub kind: String,
    pub request: Option<LoggedRequest>,
    pub status: Option<u16>,
    /// The raw response. A non-JSON body is kept as a string rather than
    /// dropped, so an HTML error page survives into the audit trail.
    pub response: Option<Value>,
    /// Seconds.
    pub duration: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialises_with_an_rfc3339_timestamp() {
        let log = InteractionLog {
            gateway: "scripay".into(),
            kind: "auth".into(),
            request: Some(LoggedRequest {
                url: "https://api.example.com/v1/auth".into(),
                params: json!({"method": "POST"}),
            }),
            status: Some(200),
            response: Some(json!({"access_token": "t"})),
            duration: 0.25,
        };
        let v: Value = serde_json::to_value(&log).unwrap();
        assert_eq!(v["created_at"], json!("1970-01-01T00:00:00Z"));
        assert_eq!(v["kind"], json!("auth"));
        assert_eq!(serde_json::from_value::<InteractionLog>(v).unwrap(), log);
    }
}
