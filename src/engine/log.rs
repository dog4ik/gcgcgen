use std::time::Instant;

use serde_json::{Map, Value};

pub use connect::{InteractionLog, LoggedRequest};

/// An in-flight interaction. Started before the request is built so that a
/// failure to build one is still timed and reported.
pub struct InteractionSpan {
    started: Instant,
    request: Option<LoggedRequest>,
    status: Option<u16>,
    response: Option<Value>,
}

impl InteractionSpan {
    pub fn enter() -> Self {
        Self {
            started: Instant::now(),
            request: None,
            status: None,
            response: None,
        }
    }

    /// Whether the request was built and about to be sent. A span without one
    /// never reached the wire and is not worth a log entry.
    pub fn has_request(&self) -> bool {
        self.request.is_some()
    }

    pub fn set_request(&mut self, url: String, params: Value) {
        self.request = Some(LoggedRequest { url, params });
    }

    pub fn set_status(&mut self, status: u16) {
        self.status = Some(status);
    }

    pub fn set_response(&mut self, response: Value) {
        self.response = Some(response);
    }

    pub fn finish(self, gateway: &str, kind: &str, redactor: &Redactor) -> InteractionLog {
        InteractionLog {
            gateway: gateway.to_string(),
            kind: kind.to_string(),
            request: self.request.map(|r| LoggedRequest {
                url: redactor.redact_str(&r.url),
                params: redactor.redact(&r.params),
            }),
            status: self.status,
            response: self.response.as_ref().map(|r| redactor.redact(r)),
            duration: self.started.elapsed().as_secs_f32(),
        }
    }
}

/// Replaces secret settings values wherever they appear in a log entry. Logs
/// travel to the platform and into storage. Matching is by value, which is why
/// the settings schema has to mark fields `secret`.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

const MASK: &str = "***redacted***";
/// Short values (a `sandbox` flag of `"1"`, a two-letter code) would match
/// everywhere and turn a log into noise, so they are left alone.
const MIN_SECRET_LEN: usize = 6;

impl Redactor {
    pub fn new(secrets: impl IntoIterator<Item = String>) -> Self {
        let mut secrets: Vec<String> = secrets
            .into_iter()
            .filter(|s| s.len() >= MIN_SECRET_LEN)
            .collect();
        // Longest first, so a secret that contains another is masked whole.
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        Self { secrets }
    }

    pub fn redact_str(&self, s: &str) -> String {
        let mut out = s.to_string();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), MASK);
            }
        }
        out
    }

    pub fn redact(&self, v: &Value) -> Value {
        if self.secrets.is_empty() {
            return v.clone();
        }
        match v {
            Value::String(s) => Value::String(self.redact_str(s)),
            Value::Array(items) => Value::Array(items.iter().map(|i| self.redact(i)).collect()),
            Value::Object(fields) => {
                let mut out = Map::new();
                for (k, val) in fields {
                    out.insert(k.clone(), self.redact(val));
                }
                Value::Object(out)
            }
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn masks_secrets_anywhere_in_the_payload() {
        let r = Redactor::new(["sk_live_abcdef".to_string()]);
        assert_eq!(
            r.redact(&json!({"a": "sk_live_abcdef", "b": ["Bearer sk_live_abcdef"], "c": 1})),
            json!({"a": MASK, "b": [format!("Bearer {MASK}")], "c": 1})
        );
    }

    #[test]
    fn leaves_short_values_alone() {
        // Masking a two-character code would redact half the document.
        let r = Redactor::new(["ke".to_string(), "true".to_string()]);
        assert_eq!(
            r.redact(&json!({"country": "ke"})),
            json!({"country": "ke"})
        );
    }

    #[test]
    fn masks_the_longest_match_first() {
        let r = Redactor::new([
            "secret_value".to_string(),
            "secret_value_extended".to_string(),
        ]);
        assert_eq!(r.redact_str("secret_value_extended"), MASK);
    }

    #[test]
    fn span_produces_one_log() {
        let mut span = InteractionSpan::enter();
        span.set_request(
            "https://x/y".into(),
            json!({"client_secret": "sk_live_abcdef"}),
        );
        span.set_status(200);
        span.set_response(json!({"ok": true}));
        let log = span.finish(
            "scripay",
            "auth",
            &Redactor::new(["sk_live_abcdef".to_string()]),
        );
        assert_eq!(log.kind, "auth");
        assert_eq!(log.status, Some(200));
        assert_eq!(log.request.unwrap().params, json!({"client_secret": MASK}));
        assert!(log.duration >= 0.0);
    }
}
