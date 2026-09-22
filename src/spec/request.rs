//! A single outbound HTTP request

use serde::{Deserialize, Serialize};

use super::auth::AuthId;
use super::expr::Expr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestDef {
    /// Identifies this step
    pub name: String,
    #[serde(default)]
    pub method: HttpMethod,
    pub path: Expr,
    #[serde(default)]
    pub auth: Option<AuthId>,
    #[serde(default)]
    pub headers: Vec<NameValue>,
    #[serde(default)]
    pub body: Body,
    #[serde(default)]
    pub envelope: Envelope,
    #[serde(default)]
    pub run_if: Option<Expr>,
    #[serde(default)]
    pub response: ResponseSpec,
}

impl RequestDef {
    pub fn new(name: impl Into<String>, path: Expr) -> Self {
        Self {
            name: name.into(),
            method: HttpMethod::Post,
            path,
            auth: None,
            headers: Vec::new(),
            body: Body::None,
            envelope: Envelope::Json,
            run_if: None,
            response: ResponseSpec::default(),
        }
    }

    /// Every expression in this request
    pub fn exprs(&self) -> Vec<&Expr> {
        let mut out = vec![&self.path];
        out.extend(self.headers.iter().map(|h| &h.value));
        match &self.body {
            Body::None => {}
            Body::Json { expr } => out.push(expr),
        }
        out
    }
}

/// One header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NameValue {
    pub name: String,
    pub value: Expr,
}

impl NameValue {
    pub fn new(name: impl Into<String>, value: Expr) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpMethod {
    Get,
    #[default]
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Body {
    #[default]
    None,
    /// Normally an object literal: `{"order_id": payment.token}`.
    Json { expr: Expr },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Envelope {
    #[default]
    Json,
    Form,
}

impl Envelope {
    pub fn as_str(self) -> &'static str {
        match self {
            Envelope::Json => "json",
            Envelope::Form => "form",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseSpec {
    #[serde(default)]
    pub success_when: Option<Expr>,
    #[serde(default)]
    pub error: ErrorSpec,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorSpec {
    #[serde(default)]
    pub message: Option<Expr>,
    #[serde(default)]
    pub on_error: OnError,
}

/// What to do on an error, transport or gateway.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnError {
    /// Classify it: a transport error is pending, a gateway error declines.
    #[default]
    Fail,
    Declined,
    Pending,
    /// Record the failure and run the next request anyway.
    Continue,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPAY_PAY: &str = r#"{
        "name": "collection",
        "method": "post",
        "path": "'/v1/gateway/initiate/collection'",
        "auth": "oauth",
        "body": {
            "kind": "json",
            "expr": "{\"purpose\": 'payment', \"order_id\": payment.token, \"amount\": payment.gateway_amount | minor_to_major}"
        },
        "response": {
            "error": { "message": "resp.body.message", "on_error": "fail" }
        }
    }"#;

    #[test]
    fn parses_a_request() {
        let r: RequestDef = serde_json::from_str(SCRIPAY_PAY).unwrap();
        assert_eq!(r.name, "collection");
        assert_eq!(r.method, HttpMethod::Post);
        assert_eq!(r.auth.as_ref().unwrap().as_str(), "oauth");
        assert_eq!(r.response.success_when, None);
        assert_eq!(r.response.error.on_error, OnError::Fail);
    }

    #[test]
    fn round_trips() {
        let r: RequestDef = serde_json::from_str(SCRIPAY_PAY).unwrap();
        let back: RequestDef = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn collects_every_expression() {
        let r: RequestDef = serde_json::from_str(SCRIPAY_PAY).unwrap();
        assert_eq!(r.exprs().len(), 2);
    }

    #[test]
    fn success_when_is_one_expression_over_the_response() {
        let r: RequestDef = serde_json::from_str(
            r#"{"name":"x","path":"'/p'","response":
                 {"success_when":"resp.ok && resp.body.status == \"Success\""}}"#,
        )
        .unwrap();
        let e = r.response.success_when.unwrap();
        assert_eq!(e.roots(), ["resp"]);
    }

    #[test]
    fn defaults_are_the_safe_ones() {
        let r: RequestDef = serde_json::from_str(r#"{"name":"x","path":"'/p'"}"#).unwrap();
        assert_eq!(r.method, HttpMethod::Post);
        assert_eq!(r.body, Body::None);
        assert_eq!(r.envelope, Envelope::Json);
        assert_eq!(r.response.error.on_error, OnError::Fail);
    }
}
