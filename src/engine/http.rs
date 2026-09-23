use serde_json::{Map, Value};

use super::error::{EngineError, Result};
use crate::spec::{Body, Envelope, HttpMethod, RequestDef};

/// A fully rendered request, ready to sign and send.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub method: HttpMethod,
    /// Path portion, as authored (used by signature canonical strings).
    pub path: String,
    pub base_url: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: PreparedBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PreparedBody {
    None,
    Json(Value),
    Form(Vec<(String, String)>),
}

impl Prepared {
    /// URL without the query string.
    fn url(&self) -> String {
        format!("{}{}", self.base_url, self.path)
    }

    /// URL including the query string, as it will be requested and logged.
    pub fn full_url(&self) -> String {
        if self.query.is_empty() {
            return self.url();
        }
        format!("{}?{}", self.url(), urlencode_pairs(&self.query))
    }

    /// The serialized body, for signing and for `req.body_raw`.
    pub fn body_raw(&self) -> String {
        match &self.body {
            PreparedBody::None => String::new(),
            PreparedBody::Json(v) => serde_json::to_string(v).unwrap_or_default(),
            PreparedBody::Form(fields) => urlencode_pairs(fields),
        }
    }

    pub fn body_value(&self) -> Value {
        match &self.body {
            PreparedBody::None => Value::Null,
            PreparedBody::Json(v) => v.clone(),
            PreparedBody::Form(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect(),
            ),
        }
    }

    /// What lands in the interaction log's `request.params`.
    pub fn log_params(&self) -> Value {
        let mut m = Map::new();
        m.insert(
            "method".into(),
            Value::String(self.method.as_str().to_string()),
        );
        if !self.headers.is_empty() {
            m.insert("headers".into(), pairs(&self.headers));
        }
        if !self.query.is_empty() {
            m.insert("query".into(), pairs(&self.query));
        }
        if !matches!(self.body, PreparedBody::None) {
            m.insert("body".into(), self.body_value());
        }
        Value::Object(m)
    }

    /// The `req` root a signature's canonical string reads.
    pub fn req_scope(&self) -> Value {
        let mut m = Map::new();
        m.insert(
            "method".into(),
            Value::String(self.method.as_str().to_string()),
        );
        m.insert("path".into(), Value::String(self.path.clone()));
        m.insert("url".into(), Value::String(self.full_url()));
        m.insert("query".into(), pairs(&self.query));
        m.insert("headers".into(), pairs(&self.headers));
        m.insert("body".into(), self.body_value());
        m.insert("body_raw".into(), Value::String(self.body_raw()));
        Value::Object(m)
    }

    pub fn set_header(&mut self, name: &str, value: String) {
        match self
            .headers
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            Some(slot) => slot.1 = value,
            None => self.headers.push((name.to_string(), value)),
        }
    }

    pub fn set_query(&mut self, name: &str, value: String) {
        match self.query.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = value,
            None => self.query.push((name.to_string(), value)),
        }
    }

    pub fn set_body_field(&mut self, path: &str, value: Value) -> Result<()> {
        if let PreparedBody::Form(fields) = &mut self.body {
            let mut segs = path.split('.').filter(|s| !s.is_empty());
            let Some(first) = segs.next() else {
                return Err(EngineError::Config(
                    "signature body path must not be empty".into(),
                ));
            };
            let key = segs.fold(first.to_string(), |k, s| format!("{k}[{s}]"));
            let value = match value {
                Value::String(s) => s,
                other => other.to_string(),
            };
            match fields.iter_mut().find(|(n, _)| *n == key) {
                Some(slot) => slot.1 = value,
                None => fields.push((key, value)),
            }
            return Ok(());
        }
        let PreparedBody::Json(root) = &mut self.body else {
            return Err(EngineError::Config(
                "signature placement `body_field` requires a JSON or form body".into(),
            ));
        };
        if !root.is_object() {
            *root = Value::Object(Map::new());
        }
        let mut cur = root;
        let mut segs = path.split('.').peekable();
        while let Some(seg) = segs.next() {
            if segs.peek().is_none() {
                cur.as_object_mut()
                    .expect("ensured to be an object below")
                    .insert(seg.to_string(), value);
                return Ok(());
            }
            let obj = cur.as_object_mut().expect("ensured to be an object below");
            let next = obj
                .entry(seg.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if !next.is_object() {
                *next = Value::Object(Map::new());
            }
            cur = next;
        }
        Err(EngineError::Config(
            "signature body path must not be empty".into(),
        ))
    }
}

fn pairs(items: &[(String, String)]) -> Value {
    Value::Object(
        items
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect(),
    )
}

fn urlencode_pairs(pairs: &[(String, String)]) -> String {
    form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

/// Evaluates a request definition against a scope.
pub fn prepare(req: &RequestDef, base_url: &str, scope: &Value) -> Result<Prepared> {
    let path = req.path.eval_text(scope)?.ok_or_else(|| {
        EngineError::Config(format!("request `{}`: path evaluated to nothing", req.name))
    })?;

    let mut headers = Vec::new();
    for h in &req.headers {
        if let Some(v) = h.value.eval_text(scope)? {
            headers.push((h.name.clone(), v));
        }
    }

    // Evaluating to nothing sends nothing.
    let body = match &req.body {
        Body::None => PreparedBody::None,
        Body::Json { expr } => match expr.eval(scope)? {
            None => PreparedBody::None,
            Some(value) => match req.envelope {
                Envelope::Json => PreparedBody::Json(value),
                Envelope::Form => PreparedBody::Form(form_pairs(&req.name, value)?),
            },
        },
    };

    Ok(Prepared {
        method: req.method,
        path,
        base_url: base_url.trim_end_matches('/').to_string(),
        query: Vec::new(),
        headers,
        body,
    })
}

/// Flattens an evaluated JSON body into form pairs: nested objects become
/// `a[b]`, arrays `a[0]`, scalars their plain text. Nulls are dropped, as an
/// absent value already has been.
fn form_pairs(name: &str, value: Value) -> Result<Vec<(String, String)>> {
    fn walk(key: String, value: Value, out: &mut Vec<(String, String)>) {
        match value {
            Value::Null => {}
            Value::String(s) => out.push((key, s)),
            Value::Bool(_) | Value::Number(_) => out.push((key, value.to_string())),
            Value::Array(items) => {
                for (i, v) in items.into_iter().enumerate() {
                    walk(format!("{key}[{i}]"), v, out);
                }
            }
            Value::Object(m) => {
                for (k, v) in m {
                    walk(format!("{key}[{k}]"), v, out);
                }
            }
        }
    }

    let Value::Object(m) = value else {
        return Err(EngineError::Config(format!(
            "request `{name}`: a form envelope needs the body to evaluate to an object"
        )));
    };
    let mut out = Vec::new();
    for (k, v) in m {
        walk(k, v, &mut out);
    }
    Ok(out)
}

/// What came back. The body is always captured, even when it is not JSON, so
/// the interaction log never loses an error page.
#[derive(Debug, Clone)]
pub struct RawResponse {
    pub status: u16,
    pub headers: Value,
    pub body: Value,
}

impl RawResponse {
    /// The `resp` root a response spec reads.
    pub fn to_scope(&self) -> Value {
        let mut m = Map::new();
        m.insert("status".into(), Value::Number(self.status.into()));
        m.insert("headers".into(), self.headers.clone());
        m.insert("body".into(), self.body.clone());
        m.insert("ok".into(), Value::Bool((200..300).contains(&self.status)));
        Value::Object(m)
    }
}

pub async fn send(client: &reqwest::Client, prepared: &Prepared) -> Result<RawResponse> {
    let method = reqwest::Method::from_bytes(prepared.method.as_str().as_bytes())
        .map_err(|e| EngineError::Config(format!("bad HTTP method: {e}")))?;

    let mut builder = client.request(method, prepared.full_url());
    for (name, value) in &prepared.headers {
        builder = builder.header(name, value);
    }
    match &prepared.body {
        PreparedBody::None => {}
        PreparedBody::Json(v) => builder = builder.json(v),
        PreparedBody::Form(fields) => {
            builder = builder
                .header("content-type", "application/x-www-form-urlencoded")
                .body(prepared.body_raw());
            let _ = fields;
        }
    }
    let resp = builder
        .send()
        .await
        .map_err(|e| EngineError::Transport(e.to_string()))?;
    let status = resp.status().as_u16();

    let headers = Value::Object(
        resp.headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|v| (k.as_str().to_string(), Value::String(v.to_string())))
            })
            .collect(),
    );

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| EngineError::Transport(e.to_string()))?;
    // Kept verbatim, so an HTML error page still reaches the log.
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));

    Ok(RawResponse {
        status,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(src: &str) -> RequestDef {
        serde_json::from_str(src).unwrap()
    }

    fn scope() -> Value {
        json!({
            "payment": {"token": "tok 1", "gateway_amount": 1000},
            "settings": {"wallet": "w1", "key": "k"},
            "params": {}, "steps": {}, "env": {}, "method": {"kind": "pay"}
        })
    }

    #[test]
    fn evaluates_path_headers_and_json_body() {
        let r = req(r#"{
            "name": "c", "method": "post", "path": "'/v1/pay/' + payment.token",
            "headers": [{"name": "X-Key", "value": "settings.key"},
                        {"name": "X-Skip", "value": "payment.nope"}],
            "body": {"kind": "json", "expr":
                "{\"amount\": payment.gateway_amount | minor_to_major, \"drop\": payment.nope}"}
        }"#);
        let p = prepare(&r, "https://api.example.com", &scope()).unwrap();

        assert_eq!(p.path, "/v1/pay/tok 1");
        // The header whose value is absent is gone, not empty.
        assert_eq!(p.headers, vec![("X-Key".to_string(), "k".to_string())]);
        assert_eq!(p.body, PreparedBody::Json(json!({"amount": 10})));
        assert_eq!(p.full_url(), "https://api.example.com/v1/pay/tok 1");
        assert_eq!(p.body_raw(), r#"{"amount":10}"#);
    }

    #[test]
    fn form_bodies_are_urlencoded() {
        let r = req(
            r#"{"name":"c","path":"'/f'","envelope":"form","body":{"kind":"json","expr":
                "{\"a\": 'x y', \"b\": settings.wallet}"}}"#,
        );
        let p = prepare(&r, "https://api.example.com", &scope()).unwrap();
        assert_eq!(p.body_raw(), "a=x+y&b=w1");
        assert_eq!(p.body_value(), json!({"a": "x y", "b": "w1"}));
    }

    #[test]
    fn form_envelope_flattens_a_json_body() {
        let r = req(
            r#"{"name":"c","path":"'/f'","envelope":"form","body":{"kind":"json","expr":
            "{\"amount\": payment.gateway_amount | minor_to_major, \"wallet\": settings.wallet, \"drop\": payment.nope, \"meta\": {\"tags\": ['a b', true]}}"}}"#,
        );
        let p = prepare(&r, "https://api.example.com", &scope()).unwrap();
        // Keys sort; `drop` is absent and never reaches the wire.
        assert_eq!(
            p.body_raw(),
            "amount=10&meta%5Btags%5D%5B0%5D=a+b&meta%5Btags%5D%5B1%5D=true&wallet=w1"
        );
    }

    #[test]
    fn form_envelope_rejects_a_non_object_body() {
        let r = req(
            r#"{"name":"c","path":"'/f'","envelope":"form","body":{"kind":"json","expr":"[1]"}}"#,
        );
        assert!(prepare(&r, "https://api.example.com", &scope()).is_err());
    }

    #[test]
    fn set_body_field_on_a_form_body_uses_bracket_keys() {
        let r = req(
            r#"{"name":"c","path":"'/p'","envelope":"form","body":{"kind":"json","expr":"{\"a\": 1}"}}"#,
        );
        let mut p = prepare(&r, "https://api.example.com", &scope()).unwrap();
        p.set_body_field("auth.sig", json!("deadbeef")).unwrap();
        assert_eq!(p.body_raw(), "a=1&auth%5Bsig%5D=deadbeef");
    }

    #[test]
    fn log_params_capture_the_whole_request() {
        let r = req(
            r#"{"name":"c","path":"'/p'","headers":[{"name":"H","value":"'v'"}],
                        "body":{"kind":"json","expr":"{\"a\": 1}"}}"#,
        );
        let p = prepare(&r, "https://api.example.com", &scope()).unwrap();
        assert_eq!(
            p.log_params(),
            json!({"method": "POST", "headers": {"H": "v"}, "body": {"a": 1}})
        );
    }

    #[test]
    fn req_scope_exposes_the_signable_surface() {
        let r = req(r#"{"name":"c","path":"'/p'","body":{"kind":"json","expr":"{\"a\": 1}"}}"#);
        let mut p = prepare(&r, "https://api.example.com", &scope()).unwrap();
        // Only an auth placement puts anything in the query string now.
        p.set_query("q", "1".into());
        let s = p.req_scope();
        assert_eq!(s["method"], json!("POST"));
        assert_eq!(s["path"], json!("/p"));
        assert_eq!(s["url"], json!("https://api.example.com/p?q=1"));
        assert_eq!(s["body_raw"], json!(r#"{"a":1}"#));
    }

    #[test]
    fn set_body_field_creates_nested_objects() {
        let r = req(r#"{"name":"c","path":"'/p'","body":{"kind":"json","expr":"{\"a\": 1}"}}"#);
        let mut p = prepare(&r, "https://api.example.com", &scope()).unwrap();
        p.set_body_field("auth.signature", json!("deadbeef"))
            .unwrap();
        assert_eq!(
            p.body_value(),
            json!({"a": 1, "auth": {"signature": "deadbeef"}})
        );
    }

    #[test]
    fn set_body_field_rejects_a_non_json_body() {
        let r = req(r#"{"name":"c","path":"'/p'"}"#);
        let mut p = prepare(&r, "https://api.example.com", &scope()).unwrap();
        assert!(p.set_body_field("sig", json!("x")).is_err());
    }

    #[test]
    fn setting_a_header_is_case_insensitive() {
        let r = req(
            r#"{"name":"c","path":"'/p'","headers":[{"name":"Authorization","value":"'old'"}]}"#,
        );
        let mut p = prepare(&r, "https://api.example.com", &scope()).unwrap();
        p.set_header("authorization", "new".into());
        assert_eq!(
            p.headers,
            vec![("Authorization".to_string(), "new".to_string())]
        );
    }

    #[test]
    fn response_scope_reports_ok_and_keeps_the_body() {
        let r = RawResponse {
            status: 502,
            headers: json!({}),
            body: json!("<html>oops</html>"),
        };
        let s = r.to_scope();
        assert_eq!(s["ok"], json!(false));
        assert_eq!(s["status"], json!(502));
        assert_eq!(s["body"], json!("<html>oops</html>"));
    }
}
