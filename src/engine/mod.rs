//! Executing one method of one integration.
//!
//! A method is an ordered list of requests. Each sees the inbound input plus
//! `steps.<name>` for every request that already ran, so request *n* can
//! consume request *n-1*'s output. After the last one, [`ResultMapping`] turns
//! the accumulated scope into the reply.
//!
//! # Invariants the engine owns, not the spec
//!
//! These exist because a per-integration document is the wrong place to
//! re-decide them, and getting one wrong costs money:
//!
//! 1. **Uncertainty becomes `pending`, never `declined`.** A transport error,
//!    a 5xx, or an unreadable body all mean the gateway may have acted. See
//!    [`error::EngineError::is_uncertain`].
//! 2. **Every outbound call is logged**, auth calls included, because the
//!    engine opens the span — a spec cannot describe an unlogged request.
//! 3. **Failures are HTTP 200** with `{"result": false, …}`.
//! 4. **Secrets are redacted** from logs by value, driven by the settings
//!    schema's `secret` flags.

pub mod auth;
pub mod error;
pub mod http;
pub mod log;
pub mod scope;

use serde_json::{Map, Value};

use connect::{
    ConnectInput, ConnectResponse, InteractionLog, MethodKind, Status, TransactionResponse,
};

use crate::spec::request::OnError;
use crate::spec::{AuthId, AuthKind, Condition, Integration, RequestDef, ResultMapping};
use auth::TokenStore;
use error::{EngineError, Result};
use http::{prepare, send, Prepared, RawResponse};
use log::{InteractionSpan, Redactor};
use scope::{Env, Scope};

/// How deep an auth request may itself depend on another auth. Validation
/// rejects cycles outright; this is the runtime backstop for a document that
/// reached the engine unvalidated.
const MAX_AUTH_DEPTH: usize = 4;

/// Shared execution context: the HTTP client and the token cache.
pub struct EngineCx {
    pub client: reqwest::Client,
    pub tokens: TokenStore,
}

impl EngineCx {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            tokens: TokenStore::new(),
        }
    }
}

/// Per-invocation facts the spec sees under `env`.
#[derive(Debug, Clone)]
pub struct Runtime {
    /// Where the gateway should send its callback.
    pub callback_url: String,
    pub request_id: String,
}

pub async fn execute_method(
    cx: &EngineCx,
    integration: &Integration,
    kind: MethodKind,
    input: &ConnectInput,
    runtime: &Runtime,
) -> ConnectResponse {
    let mut logs = Vec::new();
    match run(cx, integration, kind, input, runtime, &mut logs).await {
        Ok(transaction) => ConnectResponse::success(transaction, logs),
        Err(error) => ConnectResponse::failure(error, logs),
    }
}

/// `Err` here is the `{"result": false}` envelope, not a declined payment.
/// A declined payment is `Ok(TransactionResponse { status: Declined, .. })`.
async fn run(
    cx: &EngineCx,
    integration: &Integration,
    kind: MethodKind,
    input: &ConnectInput,
    runtime: &Runtime,
    logs: &mut Vec<InteractionLog>,
) -> std::result::Result<TransactionResponse, String> {
    let method = integration.method(kind).ok_or_else(|| {
        format!(
            "`{kind}` is not enabled for integration `{}`",
            integration.key
        )
    })?;

    let now = time::OffsetDateTime::now_utc();
    let sandbox = truthy(input.setting("sandbox").unwrap_or(&Value::Null));

    // `base_url` may branch on settings, so it is rendered against a scope
    // whose own `env.base_url` is not yet known.
    let mut env = Env {
        integration_key: integration.key.clone(),
        base_url: String::new(),
        callback_url: runtime.callback_url.clone(),
        sandbox,
        now_rfc3339: now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        unix_now: now.unix_timestamp(),
        request_id: runtime.request_id.clone(),
    };
    let bootstrap = Scope::new(input, &env, kind);
    env.base_url = integration
        .base_url
        .render_string(&bootstrap.value())
        .map_err(|e| format!("base_url: {e}"))?
        .ok_or_else(|| "base_url rendered empty".to_string())?
        .trim_end_matches('/')
        .to_string();

    // Re-checked here rather than only at save time: `base_url` may be
    // templated, so this is the point at which the destination is actually
    // known, and it is the only thing standing between a bad expression and
    // credentials being posted somewhere unintended.
    if !(env.base_url.starts_with("https://") || env.base_url.starts_with("http://")) {
        return Err(format!(
            "base_url rendered `{}`, which is not an http(s) URL",
            env.base_url
        ));
    }

    let mut scope = Scope::new(input, &env, kind);
    let redactor = Redactor::new(scope.secret_values(&integration.settings));

    for req in &method.requests {
        if let Some(cond) = &req.run_if {
            let skip = cond
                .eval(&scope.value())
                .map(|v| !truthy(&v))
                .map_err(|e| format!("request `{}`: run_if: {e}", req.name))?;
            if skip {
                tracing::debug!(request = %req.name, "skipped by run_if");
                continue;
            }
        }

        let outcome = execute_request(cx, integration, req, &env, &scope, logs, &redactor, 0).await;

        match outcome {
            Ok(step) => scope.set_step(&req.name, step),
            Err((err, resp)) => {
                tracing::warn!(request = %req.name, error = %err, "request failed");
                let message = format!("{}: {err}", req.name);
                match req.response.error.on_error {
                    // The gateway may have acted, so pending — never declined.
                    OnError::Fail if err.is_uncertain() => {
                        return Ok(uncertain(input, format!("uncertain outcome: {message}")))
                    }
                    OnError::Fail => return Err(message),
                    OnError::Declined => {
                        return Ok(echo(input, Status::Declined).with_details(message))
                    }
                    OnError::Pending => {
                        return Ok(echo(input, Status::Pending).with_details(message))
                    }
                    OnError::Continue => {
                        let step = step_value(resp.as_ref(), false, None, Some(&err));
                        scope.set_step(&req.name, step);
                    }
                }
            }
        }
    }

    // Everything the gateway had to do has happened by now, so a mapping
    // failure here means we cannot *read* the outcome, not that there wasn't
    // one: report pending and let the platform's poller settle it.
    build_response(&method.result, &scope.value()).or_else(|e| {
        tracing::error!(integration = %integration.key, %kind, error = %e, "result mapping failed");
        Ok(uncertain(
            input,
            format!("could not interpret the gateway response: {e}"),
        ))
    })
}

/// Runs one request end to end. On failure the response is handed back too, so
/// [`OnError::Continue`] can still expose it as a step output.
#[allow(clippy::too_many_arguments)]
async fn execute_request(
    cx: &EngineCx,
    integration: &Integration,
    req: &RequestDef,
    env: &Env,
    scope: &Scope,
    logs: &mut Vec<InteractionLog>,
    redactor: &Redactor,
    depth: usize,
) -> std::result::Result<Value, (EngineError, Option<RawResponse>)> {
    let mut span = InteractionSpan::enter();

    let sent = async {
        let mut prepared = prepare(req, &env.base_url, &scope.value())?;
        if let Some(id) = &req.auth {
            resolve_auth(
                cx,
                integration,
                &mut prepared,
                id,
                env,
                scope,
                logs,
                redactor,
                depth,
            )
            .await?;
        }
        span.set_request(prepared.full_url(), prepared.log_params());
        let resp = send(&cx.client, &prepared).await?;
        Ok::<_, EngineError>((prepared, resp))
    }
    .await;

    let (_, resp) = match sent {
        Ok(pair) => pair,
        Err(e) => {
            // Log only if the request actually reached the wire. Failing
            // earlier means either a render error (nothing was attempted) or
            // an auth failure, which has already logged its own attempt — an
            // empty second entry would just obscure it.
            if span.has_request() {
                logs.push(span.finish(&integration.key, &req.name, redactor));
            }
            return Err((e, None));
        }
    };

    span.set_status(resp.status);
    span.set_response(resp.body.clone());
    logs.push(span.finish(&integration.key, &req.name, redactor));

    match interpret(req, &resp, scope) {
        Ok(step) => Ok(step),
        Err(e) => Err((e, Some(resp))),
    }
}

/// Applies the response spec: is this a success, and what does it contribute?
fn interpret(req: &RequestDef, resp: &RawResponse, scope: &Scope) -> Result<Value> {
    let resp_scope = scope.with_resp(resp.to_scope());

    if !eval_condition(&req.response.success_when, &resp_scope, resp.status)? {
        let message = req
            .response
            .error
            .message
            .as_ref()
            .map(|e| e.eval(&resp_scope))
            .transpose()?
            .as_ref()
            .and_then(auth::as_text)
            .unwrap_or_else(|| default_error_message(&resp.body, resp.status));
        return Err(EngineError::Api {
            message,
            status: resp.status,
        });
    }

    let out = req
        .response
        .success
        .as_ref()
        .map(|t| t.render(&resp_scope))
        .transpose()?;
    Ok(step_value(Some(resp), true, out.as_ref(), None))
}

/// `steps.<name>` always carries `ok`, `status`, `headers` and `body`, so a
/// spec can read `steps.auth.body.access_token` with no `success` template at
/// all. A `success` template's fields are merged over the top, which is how
/// `steps.auth.access_token` becomes available when one is supplied.
fn step_value(
    resp: Option<&RawResponse>,
    ok: bool,
    out: Option<&Value>,
    error: Option<&EngineError>,
) -> Value {
    let mut m = Map::new();
    m.insert("ok".into(), Value::Bool(ok));
    if let Some(r) = resp {
        m.insert("status".into(), Value::Number(r.status.into()));
        m.insert("headers".into(), r.headers.clone());
        m.insert("body".into(), r.body.clone());
    }
    if let Some(Value::Object(fields)) = out {
        for (k, v) in fields {
            m.insert(k.clone(), v.clone());
        }
    }
    if let Some(e) = error {
        let mut em = Map::new();
        em.insert("message".into(), Value::String(e.to_string()));
        em.insert("uncertain".into(), Value::Bool(e.is_uncertain()));
        if let EngineError::Api { status, .. } = e {
            em.insert("status".into(), Value::Number((*status).into()));
        }
        m.insert("error".into(), Value::Object(em));
    }
    Value::Object(m)
}

fn eval_condition(cond: &Condition, scope: &Value, status: u16) -> Result<bool> {
    Ok(match cond {
        Condition::HttpOk => (200..300).contains(&status),
        Condition::HttpStatusIn { statuses } => statuses.contains(&status),
        Condition::Eq { left, right } => left.eval(scope)? == right.eval(scope)?,
        Condition::Truthy { expr } => truthy(&expr.eval(scope)?),
        Condition::All { of } => {
            for c in of {
                if !eval_condition(c, scope, status)? {
                    return Ok(false);
                }
            }
            true
        }
        Condition::Any { of } => {
            for c in of {
                if eval_condition(c, scope, status)? {
                    return Ok(true);
                }
            }
            false
        }
        Condition::Not { of } => !eval_condition(of, scope, status)?,
    })
}

/// Best-effort message when the spec supplies no `error.message`.
fn default_error_message(body: &Value, status: u16) -> String {
    for key in [
        "message",
        "error",
        "error_description",
        "detail",
        "description",
    ] {
        match body.get(key) {
            Some(Value::String(s)) if !s.is_empty() => return s.clone(),
            Some(Value::Array(items)) => {
                let joined: Vec<&str> = items.iter().filter_map(|v| v.as_str()).collect();
                if !joined.is_empty() {
                    return joined.join(" | ");
                }
            }
            _ => {}
        }
    }
    match body {
        Value::String(s) if !s.is_empty() => s.clone(),
        _ => format!("gateway returned HTTP {status}"),
    }
}

/// Renders one request without sending anything.
///
/// This is what the editor's dry-run panel shows and what the golden mapping
/// tests assert against. Inline auth (bearer, basic, header, query, signature)
/// is applied because it is pure; a token request is skipped, since resolving
/// one would mean calling the gateway.
pub fn preview_request(
    integration: &Integration,
    kind: MethodKind,
    request_name: &str,
    input: &ConnectInput,
    runtime: &Runtime,
    steps: &Map<String, Value>,
) -> std::result::Result<Prepared, String> {
    let method = integration
        .methods
        .get(&kind)
        .ok_or_else(|| format!("`{kind}` is not defined"))?;
    let req = method
        .request(request_name)
        .ok_or_else(|| format!("`{kind}` has no request named `{request_name}`"))?;

    let now = time::OffsetDateTime::now_utc();
    let mut env = Env {
        integration_key: integration.key.clone(),
        base_url: String::new(),
        callback_url: runtime.callback_url.clone(),
        sandbox: truthy(input.setting("sandbox").unwrap_or(&Value::Null)),
        now_rfc3339: now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        unix_now: now.unix_timestamp(),
        request_id: runtime.request_id.clone(),
    };
    env.base_url = integration
        .base_url
        .render_string(&Scope::new(input, &env, kind).value())
        .map_err(|e| format!("base_url: {e}"))?
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string();

    let mut scope = Scope::new(input, &env, kind);
    for (name, value) in steps {
        scope.set_step(name, value.clone());
    }

    let mut prepared = prepare(req, &env.base_url, &scope.value()).map_err(|e| e.to_string())?;
    if let Some(def) = req.auth.as_ref().and_then(|id| integration.auth(id)) {
        if !matches!(def.kind, AuthKind::TokenRequest(_)) {
            auth::apply_inline(&mut prepared, &def.kind, &scope).map_err(|e| e.to_string())?;
        }
    }
    Ok(prepared)
}

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn resolve_auth(
    cx: &EngineCx,
    integration: &Integration,
    prepared: &mut Prepared,
    id: &AuthId,
    env: &Env,
    scope: &Scope,
    logs: &mut Vec<InteractionLog>,
    redactor: &Redactor,
    depth: usize,
) -> Result<()> {
    if depth >= MAX_AUTH_DEPTH {
        return Err(EngineError::Config(format!(
            "auth `{id}` nests more than {MAX_AUTH_DEPTH} levels deep — check for a cycle"
        )));
    }
    let def = integration
        .auth(id)
        .ok_or_else(|| EngineError::Config(format!("unknown auth `{id}`")))?;

    let AuthKind::TokenRequest(tr) = &def.kind else {
        return auth::apply_inline(prepared, &def.kind, scope);
    };

    let parts = tr
        .cache_key
        .iter()
        .map(|e| e.eval(&scope.value()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let key = auth::cache_key(&integration.key, id, &parts);

    if let Some(token) = cx.tokens.cached(&key) {
        auth::place_token(prepared, &tr.placement, &token);
        return Ok(());
    }

    // The auth request is an ordinary request: same renderer, same sender,
    // same log entry.
    let step = Box::pin(execute_request(
        cx,
        integration,
        &tr.request,
        env,
        scope,
        logs,
        redactor,
        depth + 1,
    ))
    .await
    // Name the auth definition, or the failure reads as though the business
    // request itself could not reach the gateway.
    .map_err(|(e, _)| match e {
        EngineError::Api { message, status } => EngineError::Api {
            message: format!("auth `{id}`: {message}"),
            status,
        },
        EngineError::Transport(m) => EngineError::Transport(format!("auth `{id}`: {m}")),
        EngineError::Decode(m) => EngineError::Decode(format!("auth `{id}`: {m}")),
        other => other,
    })?;

    // `token` and `expires_in` are written against `resp`, so re-expose the
    // auth response under that root.
    let token_scope = scope.with_resp(step.clone());
    let token = auth::as_text(&tr.token.eval(&token_scope)?).ok_or_else(|| {
        EngineError::Decode(format!(
            "auth `{id}`: `{}` did not yield a token",
            tr.token.src()
        ))
    })?;

    let ttl = tr
        .expires_in
        .as_ref()
        .map(|e| e.eval(&token_scope))
        .transpose()?
        .and_then(|v| v.as_f64().filter(|s| *s > 0.0).map(|s| s as u64))
        .unwrap_or(tr.default_ttl_secs);

    cx.tokens.insert(
        key,
        token.clone(),
        std::time::Duration::from_secs(ttl),
        std::time::Duration::from_secs(tr.refresh_buffer_secs),
    );
    auth::place_token(prepared, &tr.placement, &token);
    Ok(())
}

// ---------------------------------------------------------------------------
// result
// ---------------------------------------------------------------------------

fn build_response(mapping: &ResultMapping, scope: &Value) -> Result<TransactionResponse> {
    let raw = mapping.status.eval(scope)?;
    let text = auth::as_text(&raw).unwrap_or_default();
    let status = Status::parse(&text).ok_or_else(|| {
        EngineError::Decode(format!(
            "`{}` produced `{text}`, which is not one of {}",
            mapping.status.src(),
            Status::ALL.map(|s| s.as_str()).join(", ")
        ))
    })?;

    Ok(TransactionResponse {
        status,
        gateway_token: opt_text(mapping.gateway_token.as_ref(), scope)?,
        amount: mapping
            .amount
            .as_ref()
            .map(|e| e.eval(scope))
            .transpose()?
            .and_then(|v| v.as_f64())
            .map(|n| n.round().max(0.0) as u64),
        currency: opt_text(mapping.currency.as_ref(), scope)?,
        details: opt_text(mapping.details.as_ref(), scope)?,
    })
}

fn opt_text(e: Option<&crate::spec::Expr>, scope: &Value) -> Result<Option<String>> {
    Ok(e.map(|e| e.eval(scope))
        .transpose()?
        .as_ref()
        .and_then(auth::as_text))
}

/// Echoes the requested amount and currency, which is all we can honestly
/// report when the call's outcome is unknown.
fn echo(input: &ConnectInput, status: Status) -> TransactionResponse {
    TransactionResponse {
        status,
        gateway_token: None,
        amount: input
            .payment
            .get("gateway_amount")
            .and_then(|v| v.as_f64())
            .map(|n| n.round().max(0.0) as u64),
        currency: input
            .payment
            .get("gateway_currency")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        details: None,
    }
}

fn uncertain(input: &ConnectInput, details: String) -> TransactionResponse {
    echo(input, Status::Pending).with_details(details)
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !matches!(s.to_ascii_lowercase().as_str(), "" | "false" | "0" | "no"),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truthiness_matches_the_platform_sentinels() {
        assert!(!truthy(&json!(null)) && !truthy(&json!(false)) && !truthy(&json!(0)));
        assert!(!truthy(&json!("")) && !truthy(&json!("false")) && !truthy(&json!("no")));
        assert!(truthy(&json!(true)) && truthy(&json!(1)) && truthy(&json!("yes")));
    }

    #[test]
    fn default_error_message_digs_through_common_shapes() {
        assert_eq!(
            default_error_message(&json!({"message": "nope"}), 400),
            "nope"
        );
        assert_eq!(
            default_error_message(&json!({"message": ["a", "b"]}), 400),
            "a | b"
        );
        assert_eq!(default_error_message(&json!({"detail": "d"}), 400), "d");
        assert_eq!(default_error_message(&json!("<html/>"), 502), "<html/>");
        assert_eq!(
            default_error_message(&json!({}), 418),
            "gateway returned HTTP 418"
        );
    }

    #[test]
    fn step_value_exposes_the_raw_response_by_default() {
        let resp = RawResponse {
            status: 200,
            headers: json!({"x": "1"}),
            body: json!({"access_token": "T"}),
        };
        let v = step_value(Some(&resp), true, None, None);
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["body"]["access_token"], json!("T"));
    }

    #[test]
    fn a_success_template_merges_over_the_raw_response() {
        let resp = RawResponse {
            status: 200,
            headers: json!({}),
            body: json!({"rrn": "R"}),
        };
        let out = json!({"token": "R", "ok": true});
        let v = step_value(Some(&resp), true, Some(&out), None);
        assert_eq!(v["token"], json!("R"));
        assert_eq!(v["body"]["rrn"], json!("R"), "raw response stays reachable");
    }

    #[test]
    fn step_value_records_whether_a_failure_was_uncertain() {
        let e = EngineError::Api {
            message: "boom".into(),
            status: 503,
        };
        let v = step_value(None, false, None, Some(&e));
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["error"]["uncertain"], json!(true));
        assert_eq!(v["error"]["status"], json!(503));
    }

    #[test]
    fn conditions_compose() {
        let scope = json!({"resp": {"body": {"status": "ok"}}});
        let c: Condition = serde_json::from_str(
            r#"{"kind":"all","of":[{"kind":"http_ok"},
                 {"kind":"eq","left":"resp.body.status","right":"'ok'"}]}"#,
        )
        .unwrap();
        assert!(eval_condition(&c, &scope, 200).unwrap());
        assert!(!eval_condition(&c, &scope, 500).unwrap());

        let n: Condition =
            serde_json::from_str(r#"{"kind":"not","of":{"kind":"http_ok"}}"#).unwrap();
        assert!(eval_condition(&n, &scope, 500).unwrap());
    }

    #[test]
    fn result_mapping_rejects_a_status_outside_the_vocabulary() {
        let m: ResultMapping = serde_json::from_str(r#"{"status":"steps.c.body.state"}"#).unwrap();
        let scope = json!({"steps": {"c": {"body": {"state": "SETTLED"}}}});
        let err = build_response(&m, &scope).unwrap_err();
        assert!(err.to_string().contains("not one of"), "{err}");
    }

    #[test]
    fn result_mapping_builds_the_transaction() {
        let m: ResultMapping = serde_json::from_str(
            r#"{"status": "steps.c.body.state | map({SETTLED:'approved', _default:'pending'})",
                "gateway_token": "steps.c.body.rrn",
                "amount": "steps.c.body.amount | major_to_minor",
                "currency": "steps.c.body.currency"}"#,
        )
        .unwrap();
        let scope = json!({"steps": {"c": {"body":
            {"state": "SETTLED", "rrn": "R1", "amount": 10.5, "currency": "KES"}}}});
        let t = build_response(&m, &scope).unwrap();
        assert_eq!(t.status, Status::Approved);
        assert_eq!(t.gateway_token.as_deref(), Some("R1"));
        assert_eq!(t.amount, Some(1050));
        assert_eq!(t.currency.as_deref(), Some("KES"));
    }

    #[test]
    fn echo_reports_only_what_the_caller_told_us() {
        let input = ConnectInput {
            payment: json!({"gateway_amount": 1000, "gateway_currency": "KES"}),
            ..Default::default()
        };
        let t = uncertain(&input, "uncertain outcome: reset".into());
        assert_eq!(t.status, Status::Pending);
        assert_eq!(t.amount, Some(1000));
        assert_eq!(
            t.gateway_token, None,
            "no token can be claimed for an unknown outcome"
        );
        assert!(t.details.unwrap().starts_with("uncertain outcome"));
    }
}
