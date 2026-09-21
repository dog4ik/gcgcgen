//! Executing one method of one integration.
//!
//! # Invariants the engine owns, not the spec
//!
//! A per-integration document is the wrong place to re-decide these, and
//! getting one wrong costs money:
//!
//! 1. **Uncertainty becomes `pending`, never `declined`.**
//! 2. **Every outbound call is logged**, auth calls included.
//! 3. **Failures are HTTP 200** with `{"result": false, …}`.
//! 4. **Secrets are redacted** from logs by value.

pub mod auth;
pub mod callback;
pub mod context;
pub mod error;
pub mod http;
pub mod log;
pub mod platform;
pub mod scope;

use serde_json::{Map, Value};

use connect::{
    ConnectInput, ConnectResponse, InteractionLog, MethodKind, Status, TransactionResponse,
};

use result::{build_response, echo, truthy, uncertain};

use crate::spec::request::OnError;
use crate::spec::{
    AuthId, AuthKind, Iframe, Integration, RedirectDef, RedirectRequest, RequestDef, ResultMapping,
};
use auth::TokenStore;
use context::{CallbackContext, ContextStore};
use error::{EngineError, Result};
use http::{prepare, send, Prepared, RawResponse};
use log::{InteractionSpan, Redactor};
use scope::{Env, Scope};

/// Runtime backstop for a document that reached the engine unvalidated;
/// validation rejects auth cycles outright.
const MAX_AUTH_DEPTH: usize = 4;

/// Shared execution context: the HTTP client, the token cache, and what
/// pending transactions left for their callbacks.
pub struct EngineCx {
    pub client: reqwest::Client,
    pub tokens: TokenStore,
    pub contexts: ContextStore,
}

impl EngineCx {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            tokens: TokenStore::new(),
            contexts: ContextStore::default(),
        }
    }

    pub fn with_contexts(mut self, contexts: ContextStore) -> Self {
        self.contexts = contexts;
        self
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
        Ok(transaction) => {
            if !transaction.status.is_final() && integration.stores_callback_context(kind) {
                keep_callback_context(cx, integration, input, &transaction);
            }
            ConnectResponse::success(transaction, logs)
        }
        Err(error) => ConnectResponse::failure(error, logs),
    }
}

/// Leaves what the callback will need, under both ids the gateway may echo.
fn keep_callback_context(
    cx: &EngineCx,
    integration: &Integration,
    input: &ConnectInput,
    transaction: &TransactionResponse,
) {
    let Some(token) = input.token() else {
        tracing::warn!(integration = %integration.key, "no payment.token; its callback cannot be matched");
        return;
    };
    let Some(merchant_private_key) = input
        .payment
        .get("merchant_private_key")
        .and_then(Value::as_str)
        .filter(|k| !k.is_empty())
    else {
        tracing::warn!(
            integration = %integration.key, %token,
            "no payment.merchant_private_key; its callback could not be signed"
        );
        return;
    };
    let ids = [Some(token), transaction.gateway_token.as_deref()];
    cx.contexts.insert(
        &integration.key,
        ids.into_iter().flatten(),
        CallbackContext {
            token: token.to_string(),
            merchant_private_key: merchant_private_key.to_string(),
            settings: input.settings.clone(),
        },
    );
}

/// `env` without its `base_url`, which may itself be computed from the scope.
fn new_env(integration: &Integration, settings: &Value, runtime: &Runtime) -> Env {
    let now = time::OffsetDateTime::now_utc();
    Env {
        integration_key: integration.key.clone(),
        base_url: String::new(),
        callback_url: runtime.callback_url.clone(),
        sandbox: truthy(settings.get("sandbox").unwrap_or(&Value::Null)),
        now_rfc3339: now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        unix_now: now.unix_timestamp(),
        request_id: runtime.request_id.clone(),
    }
}

/// Evaluates `base_url` against a scope whose own `env.base_url` is not yet
/// known, and refuses anything that is not an http(s) URL.
fn render_base_url(
    integration: &Integration,
    bootstrap: &Scope,
) -> std::result::Result<String, String> {
    let base_url = integration
        .base_url
        .eval_text(&bootstrap.value())
        .map_err(|e| format!("base_url: {e}"))?
        .ok_or_else(|| "base_url evaluated to nothing".to_string())?
        .trim_end_matches('/')
        .to_string();

    // The last thing standing between a bad expression and credentials posted
    // somewhere unintended.
    if !(base_url.starts_with("https://") || base_url.starts_with("http://")) {
        return Err(format!(
            "base_url evaluated to `{base_url}`, which is not an http(s) URL"
        ));
    }
    Ok(base_url)
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

    let mut env = new_env(integration, &input.settings, runtime);
    env.base_url = render_base_url(integration, &Scope::new(input, &env, kind))?;

    let mut scope = Scope::new(input, &env, kind);
    let redactor = Redactor::new(scope.secret_values(&integration.settings));

    let halt = run_requests(
        cx,
        integration,
        &method.requests,
        &env,
        &mut scope,
        logs,
        &redactor,
    )
    .await?;
    match halt {
        None => {}
        // The gateway may have acted, so pending — never declined.
        Some(Halt::Uncertain(message)) => {
            return Ok(uncertain(input, format!("uncertain outcome: {message}")))
        }
        Some(Halt::Fail(message)) => return Err(message),
        Some(Halt::Declined(message)) => {
            return Ok(echo(input, Status::Declined).with_details(message))
        }
        Some(Halt::Pending(message)) => {
            return Ok(echo(input, Status::Pending).with_details(message))
        }
    }

    // A mapping failure here means we cannot *read* the outcome, not that
    // there wasn't one: let the platform's poller settle it.
    build_response(&method.result, &scope.value()).or_else(|e| {
        tracing::error!(integration = %integration.key, %kind, error = %e, "result mapping failed");
        Ok(uncertain(
            input,
            format!("could not interpret the gateway response: {e}"),
        ))
    })
}

/// Why a request sequence stopped before its last request, per the failing
/// request's `on_error`. Each carries the `name: error` message.
#[derive(Debug)]
enum Halt {
    /// `fail`, and the gateway may have acted.
    Uncertain(String),
    /// `fail`, and the gateway certainly refused.
    Fail(String),
    Declined(String),
    Pending(String),
}

/// Runs a request sequence, recording each output under `steps.<name>`.
/// `Ok(None)` means every request ran (or was skipped by `run_if`).
async fn run_requests(
    cx: &EngineCx,
    integration: &Integration,
    requests: &[RequestDef],
    env: &Env,
    scope: &mut Scope,
    logs: &mut Vec<InteractionLog>,
    redactor: &Redactor,
) -> std::result::Result<Option<Halt>, String> {
    for req in requests {
        if let Some(cond) = &req.run_if {
            let skip = cond
                .eval(&scope.value())
                .map(|v| !v.is_some_and(|v| truthy(&v)))
                .map_err(|e| format!("request `{}`: run_if: {e}", req.name))?;
            if skip {
                tracing::debug!(request = %req.name, "skipped by run_if");
                continue;
            }
        }

        let outcome = execute_request(cx, integration, req, env, scope, logs, redactor, 0).await;

        match outcome {
            Ok(step) => scope.set_step(&req.name, step),
            Err((err, resp)) => {
                tracing::warn!(request = %req.name, error = %err, "request failed");
                let message = format!("{}: {err}", req.name);
                match req.response.error.on_error {
                    OnError::Fail if err.is_uncertain() => {
                        return Ok(Some(Halt::Uncertain(message)))
                    }
                    OnError::Fail => return Ok(Some(Halt::Fail(message))),
                    OnError::Declined => return Ok(Some(Halt::Declined(message))),
                    OnError::Pending => return Ok(Some(Halt::Pending(message))),
                    OnError::Continue => {
                        let step = step_value(resp.as_ref(), false, None, Some(&err));
                        scope.set_step(&req.name, step);
                    }
                }
            }
        }
    }
    Ok(None)
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
            // Only if it reached the wire: failing earlier either attempted
            // nothing or was an auth failure, which logged its own attempt.
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

    if !succeeded(req, &resp_scope, resp.status)? {
        let message = req
            .response
            .error
            .message
            .as_ref()
            // A message expression that cannot read this body falls back
            // rather than replacing the gateway's failure with its own.
            .and_then(|e| e.eval_text(&resp_scope).ok())
            .flatten()
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
        .map(|e| e.eval(&resp_scope))
        .transpose()?
        .flatten();
    Ok(step_value(Some(resp), true, out.as_ref(), None))
}

/// `steps.<name>` always carries `ok`, `status`, `headers` and `body`; a
/// `success` value's fields are merged over the top.
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

/// Omitted means `resp.ok` — HTTP 2xx.
fn succeeded(req: &RequestDef, scope: &Value, status: u16) -> Result<bool> {
    match &req.response.success_when {
        Some(e) => Ok(e.eval(scope)?.is_some_and(|v| truthy(&v))),
        None => Ok((200..300).contains(&status)),
    }
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

/// Renders one request without sending anything, for the editor's dry-run
/// panel and the golden mapping tests. Inline auth is applied because it is
/// pure; a token request is skipped, since resolving one means calling the
/// gateway.
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

    let mut env = new_env(integration, &input.settings, runtime);
    env.base_url = integration
        .base_url
        .eval_text(&Scope::new(input, &env, kind).value())
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
        .map(|e| Ok(e.eval(&scope.value())?.unwrap_or(Value::Null)))
        .collect::<std::result::Result<Vec<_>, EngineError>>()?;
    let key = auth::cache_key(&integration.key, id, &parts);

    if let Some(token) = cx.tokens.cached(&key) {
        auth::place_token(prepared, &tr.placement, &token);
        return Ok(());
    }

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
    // Name the auth, or this reads as the business request failing.
    .map_err(|(e, _)| match e {
        EngineError::Api { message, status } => EngineError::Api {
            message: format!("auth `{id}`: {message}"),
            status,
        },
        EngineError::Transport(m) => EngineError::Transport(format!("auth `{id}`: {m}")),
        EngineError::Decode(m) => EngineError::Decode(format!("auth `{id}`: {m}")),
        other => other,
    })?;

    // `token` and `expires_in` are written against `resp`.
    let token_scope = scope.with_resp(step.clone());
    let token = tr.token.eval_text(&token_scope)?.ok_or_else(|| {
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
        .flatten()
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

mod result {
    use super::*;

    pub(super) fn build_response(
        mapping: &ResultMapping,
        scope: &Value,
    ) -> Result<TransactionResponse> {
        let text = mapping.status.eval_text(scope)?.unwrap_or_default();
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
            amount: minor_amount(mapping.amount.as_ref(), scope)?,
            currency: opt_text(mapping.currency.as_ref(), scope)?,
            details: opt_text(mapping.details.as_ref(), scope)?,
            redirect_request: mapping
                .redirect_request
                .as_ref()
                .map(|r| build_redirect(r, scope))
                .transpose()?
                .flatten(),
            requisites: mapping
                .requisites
                .as_ref()
                .map(|e| eval_object(e, scope, "requisites"))
                .transpose()?
                .filter(|m| !m.is_empty()),
        })
    }

    /// `Ok(None)` when the destination is absent: no handover this time.
    pub(super) fn build_redirect(
        def: &RedirectDef,
        scope: &Value,
    ) -> Result<Option<RedirectRequest>> {
        Ok(Some(match def {
            RedirectDef::Post { url, params } => {
                let Some(url) = redirect_text(url, scope, "redirect_request.url")? else {
                    return Ok(None);
                };
                RedirectRequest::Post {
                    url,
                    params: eval_object(params, scope, "redirect_request.params")?,
                }
            }
            RedirectDef::Get { url } => {
                let Some(url) = redirect_text(url, scope, "redirect_request.url")? else {
                    return Ok(None);
                };
                RedirectRequest::Get { url }
            }
            RedirectDef::GetWithProcessing { url } => {
                let Some(url) = redirect_text(url, scope, "redirect_request.url")? else {
                    return Ok(None);
                };
                RedirectRequest::GetWithProcessing { url }
            }
            RedirectDef::RedirectHtml { html } => {
                let Some(html) = redirect_text(html, scope, "redirect_request.html")? else {
                    return Ok(None);
                };
                RedirectRequest::RedirectHtml { html }
            }
            RedirectDef::PostIframes { iframes } => {
                let mut out = Vec::with_capacity(iframes.len());
                for (i, frame) in iframes.iter().enumerate() {
                    let loc = format!("redirect_request.iframes[{i}]");
                    let Some(url) = redirect_text(&frame.url, scope, &format!("{loc}.url"))? else {
                        continue;
                    };
                    out.push(Iframe {
                        url,
                        data: eval_object(&frame.data, scope, &format!("{loc}.data"))?,
                    });
                }
                if out.is_empty() {
                    return Ok(None);
                }
                RedirectRequest::PostIframes { iframes: out }
            }
        }))
    }

    /// `Ok(None)` for an absent or blank one: never hand the shopper a dead link.
    pub(super) fn redirect_text(
        e: &crate::spec::Expr,
        scope: &Value,
        loc: &str,
    ) -> Result<Option<String>> {
        let text = e
            .eval_text(scope)
            .map_err(|e| EngineError::Decode(format!("{loc}: {e}")))?;
        Ok(text.filter(|s| !s.trim().is_empty()))
    }

    pub(super) fn eval_object(
        e: &crate::spec::Expr,
        scope: &Value,
        loc: &str,
    ) -> Result<Map<String, Value>> {
        match e
            .eval(scope)
            .map_err(|e| EngineError::Decode(format!("{loc}: {e}")))?
        {
            Some(Value::Object(m)) => Ok(m),
            None | Some(Value::Null) => Ok(Map::new()),
            Some(other) => Err(EngineError::Decode(format!(
                "{loc} evaluated to {}, but it must be an object",
                match other {
                    Value::Array(_) => "an array",
                    Value::String(_) => "a string",
                    Value::Bool(_) => "a boolean",
                    _ => "a number",
                }
            ))),
        }
    }

    /// An amount expression's value in minor units. Anything but a number is
    /// treated as absent.
    pub(super) fn minor_amount(
        e: Option<&crate::spec::Expr>,
        scope: &Value,
    ) -> Result<Option<u64>> {
        Ok(e.map(|e| e.eval(scope))
            .transpose()?
            .flatten()
            .and_then(|v| v.as_f64())
            .map(|n| n.round().max(0.0) as u64))
    }

    pub(super) fn opt_text(e: Option<&crate::spec::Expr>, scope: &Value) -> Result<Option<String>> {
        Ok(e.map(|e| e.eval_text(scope)).transpose()?.flatten())
    }

    /// Echoes the requested amount and currency, which is all we can honestly
    /// report when the call's outcome is unknown.
    pub(super) fn echo(input: &ConnectInput, status: Status) -> TransactionResponse {
        TransactionResponse {
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
            ..TransactionResponse::new(status)
        }
    }

    pub(super) fn uncertain(input: &ConnectInput, details: String) -> TransactionResponse {
        echo(input, Status::Pending).with_details(details)
    }

    pub(super) fn truthy(v: &Value) -> bool {
        match v {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
            Value::String(s) => {
                !matches!(s.to_ascii_lowercase().as_str(), "" | "false" | "0" | "no")
            }
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
        }
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
    fn a_success_expression_merges_over_the_raw_response() {
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
    fn success_is_one_expression_over_the_response() {
        let req: RequestDef = serde_json::from_str(
            r#"{"name":"c","path":"'/c'","response":
                 {"success_when":"resp.ok && resp.body.status == \"ok\""}}"#,
        )
        .unwrap();
        let ok = json!({"resp": {"ok": true, "body": {"status": "ok"}}});
        let wrong_body = json!({"resp": {"ok": true, "body": {"status": "nope"}}});
        let bad_status = json!({"resp": {"ok": false, "body": {"status": "ok"}}});

        assert!(succeeded(&req, &ok, 200).unwrap());
        assert!(!succeeded(&req, &wrong_body, 200).unwrap());
        assert!(
            !succeeded(&req, &bad_status, 500).unwrap(),
            "`resp.ok` is the 2xx check, so the expression can spend it as it likes"
        );
    }

    #[test]
    fn an_omitted_success_when_is_http_2xx() {
        let req: RequestDef = serde_json::from_str(r#"{"name":"c","path":"'/c'"}"#).unwrap();
        let scope = json!({"resp": {"ok": true, "body": {}}});
        assert!(succeeded(&req, &scope, 204).unwrap());
        assert!(!succeeded(&req, &scope, 400).unwrap());
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
            r#"{"status": "steps.c.body.state | map({\"SETTLED\":\"approved\", \"_default\":\"pending\"})",
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
    fn result_mapping_builds_a_redirect_and_requisites() {
        let m: ResultMapping = serde_json::from_str(
            r#"{"status": "\"pending\"",
                "redirect_request": {
                    "type": "post",
                    "url": "steps.c.body.pay_url",
                    "params": "{\"order\": steps.c.body.rrn, \"amount\": 10}"
                },
                "requisites": "{\"account\": steps.c.body.account, \"bank\": steps.c.body.missing}"}"#,
        )
        .unwrap();
        let scope = json!({"steps": {"c": {"body":
            {"pay_url": "https://pay.example/h/1", "rrn": "R1", "account": "0011"}}}});

        let t = build_response(&m, &scope).unwrap();
        assert_eq!(
            t.redirect_request,
            Some(RedirectRequest::Post {
                url: "https://pay.example/h/1".into(),
                params: json!({"order": "R1", "amount": 10})
                    .as_object()
                    .unwrap()
                    .clone(),
            })
        );
        assert_eq!(
            t.requisites,
            Some(json!({"account": "0011"}).as_object().unwrap().clone()),
            "an absent value leaves the key out rather than nulling it"
        );
    }

    #[test]
    fn an_absent_redirect_url_drops_the_redirect() {
        let m: ResultMapping = serde_json::from_str(
            r#"{"status": "\"approved\"",
                "redirect_request": { "type": "get", "url": "steps.c.body.pay_url" },
                "requisites": "{}"}"#,
        )
        .unwrap();
        let t = build_response(&m, &json!({"steps": {"c": {"body": {}}}})).unwrap();
        assert_eq!(t.status, Status::Approved);
        assert_eq!(t.redirect_request, None);
        assert_eq!(t.requisites, None, "an empty object is not worth sending");
    }

    #[test]
    fn iframes_evaluate_each_frames_own_data() {
        let m: ResultMapping = serde_json::from_str(
            r#"{"status": "\"pending\"",
                "redirect_request": { "type": "post_iframes", "iframes": [
                    { "url": "steps.c.body.frame", "data": "{\"t\": steps.c.body.rrn}" },
                    { "url": "steps.c.body.gone" }
                ]}}"#,
        )
        .unwrap();
        let scope = json!({"steps": {"c": {"body": {"frame": "https://f.example", "rrn": "R1"}}}});
        let t = build_response(&m, &scope).unwrap();
        assert_eq!(
            t.redirect_request,
            Some(RedirectRequest::PostIframes {
                iframes: vec![Iframe {
                    url: "https://f.example".into(),
                    data: json!({"t": "R1"}).as_object().unwrap().clone(),
                }]
            })
        );
    }

    #[test]
    fn requisites_must_evaluate_to_an_object() {
        let m: ResultMapping =
            serde_json::from_str(r#"{"status": "\"pending\"", "requisites": "steps.c"}"#).unwrap();
        let err = build_response(&m, &json!({"steps": {"c": "nope"}})).unwrap_err();
        assert!(err.to_string().contains("must be an object"), "{err}");
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
