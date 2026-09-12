//! A single outbound HTTP request within a method's sequence.
//!
//! A method's requests run in order. Each one sees the initial input plus the
//! outputs of every previous request under the `steps` root, so request *n*
//! can consume request *n-1*'s response:
//!
//! ```text
//! steps.enquiry.body.reference
//! ```

use serde::{Deserialize, Serialize};

use super::auth::AuthId;
use super::expr::Expr;
use super::template::{JsonTemplate, Template};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestDef {
    /// Identifies this step's output: `steps.<name>`.
    pub name: String,
    #[serde(default)]
    pub method: HttpMethod,
    /// Appended to the integration's `base_url`.
    pub path: Template,
    #[serde(default)]
    pub auth: Option<AuthId>,
    #[serde(default)]
    pub headers: Vec<NameValue>,
    #[serde(default)]
    pub query: Vec<NameValue>,
    #[serde(default)]
    pub body: Body,
    /// Skip this request unless the expression is truthy.
    #[serde(default)]
    pub run_if: Option<Expr>,
    #[serde(default)]
    pub response: ResponseSpec,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl RequestDef {
    pub fn new(name: impl Into<String>, path: Template) -> Self {
        Self {
            name: name.into(),
            method: HttpMethod::Post,
            path,
            auth: None,
            headers: Vec::new(),
            query: Vec::new(),
            body: Body::None,
            run_if: None,
            response: ResponseSpec::default(),
            timeout_ms: None,
        }
    }

    /// Every template in the request, for validation and reference analysis.
    pub fn templates(&self) -> Vec<&Template> {
        let mut out = vec![&self.path];
        out.extend(self.headers.iter().map(|h| &h.value));
        out.extend(self.query.iter().map(|q| &q.value));
        match &self.body {
            Body::None => {}
            Body::Json { template } => out.extend(template.templates()),
            Body::Form { fields } => out.extend(fields.iter().map(|f| &f.value)),
            Body::Raw { text, .. } => out.push(text),
        }
        out
    }
}

/// One header or query parameter. A list rather than a map because the editor
/// renders it as an add/remove grid and because duplicate names are legal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NameValue {
    pub name: String,
    pub value: Template,
}

impl NameValue {
    pub fn new(name: impl Into<String>, value: Template) -> Self {
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
    Json {
        template: JsonTemplate,
    },
    Form {
        fields: Vec<NameValue>,
    },
    Raw {
        content_type: String,
        text: Template,
    },
}

/// How a response becomes either a step output or an error.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseSpec {
    #[serde(default)]
    pub success_when: Condition,
    /// Shapes `steps.<name>`. When omitted the raw
    /// `{status, headers, body}` object is stored instead, so a spec can
    /// simply read `steps.auth.body.access_token`.
    #[serde(default)]
    pub success: Option<JsonTemplate>,
    #[serde(default)]
    pub error: ErrorSpec,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
    /// HTTP 2xx.
    #[default]
    HttpOk,
    HttpStatusIn {
        statuses: Vec<u16>,
    },
    Eq {
        left: Expr,
        right: Expr,
    },
    Truthy {
        expr: Expr,
    },
    All {
        of: Vec<Condition>,
    },
    Any {
        of: Vec<Condition>,
    },
    Not {
        of: Box<Condition>,
    },
}

impl Condition {
    /// Walks the tree, yielding every expression for validation.
    pub fn exprs<'a>(&'a self, out: &mut Vec<&'a Expr>) {
        match self {
            Condition::HttpOk | Condition::HttpStatusIn { .. } => {}
            Condition::Eq { left, right } => {
                out.push(left);
                out.push(right);
            }
            Condition::Truthy { expr } => out.push(expr),
            Condition::All { of } | Condition::Any { of } => of.iter().for_each(|c| c.exprs(out)),
            Condition::Not { of } => of.exprs(out),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorSpec {
    /// Human-readable failure reason, surfaced to the platform.
    #[serde(default)]
    pub message: Option<Expr>,
    #[serde(default)]
    pub code: Option<Expr>,
    #[serde(default)]
    pub on_error: OnError,
}

/// What the enclosing method does when this request fails.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnError {
    /// Let the engine classify: a transport error, a 5xx, or an unparseable
    /// body is *uncertain* and becomes `pending`; anything else is `declined`.
    ///
    /// This is the default because reporting a possibly-charged payment as
    /// failed is the one unrecoverable mistake this system can make.
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
        "path": "/v1/gateway/initiate/collection",
        "auth": "oauth",
        "body": {
            "kind": "json",
            "template": {
                "purpose": "payment",
                "order_id": "{{ payment.token }}",
                "amount": "{{ payment.gateway_amount | minor_to_major }}"
            }
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
        assert_eq!(r.response.success_when, Condition::HttpOk);
        assert!(r.response.success.is_none());
        assert_eq!(r.response.error.on_error, OnError::Fail);
    }

    #[test]
    fn round_trips() {
        let r: RequestDef = serde_json::from_str(SCRIPAY_PAY).unwrap();
        let back: RequestDef = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn collects_every_template() {
        let r: RequestDef = serde_json::from_str(SCRIPAY_PAY).unwrap();
        // path + the three body leaves
        assert_eq!(r.templates().len(), 4);
    }

    #[test]
    fn nested_conditions_parse_and_expose_exprs() {
        let c: Condition = serde_json::from_str(
            r#"{"kind":"all","of":[
                 {"kind":"http_ok"},
                 {"kind":"eq","left":"resp.body.status","right":"'ok'"}
               ]}"#,
        )
        .unwrap();
        let mut exprs = Vec::new();
        c.exprs(&mut exprs);
        assert_eq!(exprs.len(), 2);
    }

    #[test]
    fn defaults_are_the_safe_ones() {
        let r: RequestDef = serde_json::from_str(r#"{"name":"x","path":"/p"}"#).unwrap();
        assert_eq!(r.method, HttpMethod::Post);
        assert_eq!(r.body, Body::None);
        assert_eq!(r.response.error.on_error, OnError::Fail);
    }
}
