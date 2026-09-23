use serde_json::{Map, Value};

use connect::{CallbackPayload, CallbackStatus, InteractionLog, Status};

use super::context::CallbackContext;
use super::log::{InteractionSpan, Redactor};
use super::result::{build_response, minor_amount, opt_text, truthy};
use super::scope::{Env, Scope};
use super::{eval_base_url, new_env, run_requests, EngineCx, Halt, Runtime};
use crate::spec::{CallbackDef, Integration};

/// The callback as it arrived.
#[derive(Debug, Clone, Default)]
pub struct InboundCallback {
    pub method: String,
    pub headers: Map<String, Value>,
    pub query: Map<String, Value>,
    /// The body exactly as sent, for signature checks.
    pub raw: String,
}

impl InboundCallback {
    /// The `callback` root. `body` is JSON when it parses as JSON, an object
    /// when it is a form, and otherwise the text itself.
    pub fn to_value(&self) -> Value {
        let body = serde_json::from_str::<Value>(&self.raw)
            .ok()
            .or_else(|| parse_form(&self.raw).map(Value::Object))
            .unwrap_or_else(|| Value::String(self.raw.clone()));
        let mut m = Map::new();
        m.insert("method".into(), Value::String(self.method.clone()));
        m.insert("headers".into(), Value::Object(self.headers.clone()));
        m.insert("query".into(), Value::Object(self.query.clone()));
        m.insert("body".into(), body);
        m.insert("raw".into(), Value::String(self.raw.clone()));
        Value::Object(m)
    }
}

/// `a=1&b=two` as an object. `None` unless every pair has a `=`, so plain
/// text does not masquerade as a one-key form.
pub fn parse_form(raw: &str) -> Option<Map<String, Value>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let mut out = Map::new();
    for pair in raw.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=')?;
        out.insert(urldecode(k)?, Value::String(urldecode(v)?));
    }
    Some(out)
}

fn urldecode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// What to do with the callback.
#[derive(Debug)]
pub enum CallbackOutcome {
    /// No stored context: the payment is unknown here, expired, or from before
    /// a restart. Acknowledged, since a retry would not find it either.
    Unmatched {
        id: Option<String>,
    },
    /// `verify` was falsy.
    Rejected,
    Skipped {
        reason: String,
    },
    Forward(Forward),
    Failed(String),
}

#[derive(Debug)]
pub struct Forward {
    /// The platform's payment token, from the stored context.
    pub token: String,
    pub merchant_private_key: String,
    pub payload: CallbackPayload,
}

#[derive(Debug)]
pub struct CallbackReply {
    pub outcome: CallbackOutcome,
    pub ack_status: u16,
    pub ack_body: Option<Value>,
}

pub async fn execute_callback(
    cx: &EngineCx,
    integration: &Integration,
    cb: &CallbackDef,
    inbound: &InboundCallback,
    runtime: &Runtime,
) -> CallbackReply {
    let mut env = new_env(integration, &Value::Null, runtime);
    let mut scope = Scope::for_callback(inbound.to_value(), &env);
    let outcome = handle(cx, integration, cb, inbound, runtime, &mut env, &mut scope).await;

    let ack_body = cb.ack.body.as_ref().and_then(|e| {
        e.eval(&scope.value())
            .inspect_err(|e| tracing::warn!(integration = %integration.key, error = %e, "ack body did not evaluate"))
            .ok()
            .flatten()
    });
    CallbackReply {
        outcome,
        ack_status: cb.ack.status,
        ack_body,
    }
}

async fn handle(
    cx: &EngineCx,
    integration: &Integration,
    cb: &CallbackDef,
    inbound: &InboundCallback,
    runtime: &Runtime,
    env: &mut Env,
    scope: &mut Scope,
) -> CallbackOutcome {
    let id = match cb.lookup.eval_text(&scope.for_lookup()) {
        Ok(id) => id,
        Err(e) => return CallbackOutcome::Failed(format!("lookup: {e}")),
    };
    let Some(context) = id
        .as_deref()
        .and_then(|id| cx.contexts.get(&integration.key, id))
    else {
        return CallbackOutcome::Unmatched { id };
    };

    scope.attach_cb_context(&context);
    env.sandbox = truthy(context.settings.get("sandbox").unwrap_or(&Value::Null));
    scope.env_mut().insert("sandbox".into(), env.sandbox.into());

    if !cb.requests.is_empty() {
        match eval_base_url(integration, scope) {
            Ok(url) => {
                scope
                    .env_mut()
                    .insert("base_url".into(), url.clone().into());
                env.base_url = url;
            }
            Err(e) => return CallbackOutcome::Failed(e),
        }
    }
    let redactor = Redactor::new(
        scope
            .secret_values(&integration.settings)
            .into_iter()
            .chain([context.merchant_private_key.clone()]),
        integration.redacted_key_list.clone(),
    );

    // The callback itself opens the interaction log.
    let mut logs = vec![inbound_log(integration, inbound, runtime, &redactor)];

    if let Some(verify) = &cb.verify {
        match verify.eval(&scope.value()) {
            Ok(Some(v)) if truthy(&v) => {}
            Ok(_) => return CallbackOutcome::Rejected,
            Err(e) => return CallbackOutcome::Failed(format!("verify: {e}")),
        }
    }

    let halt = run_requests(
        cx,
        integration,
        &cb.requests,
        env,
        scope,
        &mut logs,
        &redactor,
    )
    .await;

    let forced = match halt {
        Ok(None) => None,
        Err(e) => return CallbackOutcome::Failed(e),
        // Uncertain about what the gateway did: let it call again.
        Ok(Some(Halt::Uncertain(m))) => {
            return CallbackOutcome::Failed(format!("uncertain outcome: {m}"))
        }
        Ok(Some(Halt::Fail(m))) => return CallbackOutcome::Failed(m),
        Ok(Some(Halt::Pending(m))) => return CallbackOutcome::Skipped { reason: m },
        Ok(Some(Halt::Declined(m))) => Some(m),
    };

    match payload(cb, &scope.value(), forced, logs) {
        Ok(Some(payload)) => CallbackOutcome::Forward(forward(&context, payload)),
        Ok(None) => CallbackOutcome::Skipped {
            reason: "status is pending".into(),
        },
        Err(e) => CallbackOutcome::Failed(e),
    }
}

fn forward(context: &CallbackContext, payload: CallbackPayload) -> Forward {
    Forward {
        token: context.token.clone(),
        merchant_private_key: context.merchant_private_key.clone(),
        payload,
    }
}

fn payload(
    cb: &CallbackDef,
    scope: &Value,
    declined: Option<String>,
    logs: Vec<InteractionLog>,
) -> Result<Option<CallbackPayload>, String> {
    let result = &cb.result;
    let (status, details) = match declined {
        Some(reason) => (Status::Declined, Some(reason)),
        None => {
            let t = build_response(result, scope).map_err(|e| format!("result: {e}"))?;
            (t.status, t.details)
        }
    };
    let status = match status {
        Status::Pending => return Ok(None),
        Status::Approved => CallbackStatus::Approved,
        Status::Refunded => CallbackStatus::Refunded,
        Status::Declined => CallbackStatus::Declined {
            reason: details.unwrap_or_else(|| "unspecified reason".into()),
        },
    };
    // The payment bucket is not kept, so there is nothing to fall back to.
    let amount = minor_amount(result.amount.as_ref(), scope)
        .map_err(|e| format!("result.amount: {e}"))?
        .ok_or("result.amount produced no number")?;
    let currency = opt_text(result.currency.as_ref(), scope)
        .map_err(|e| format!("result.currency: {e}"))?
        .ok_or("result.currency produced nothing")?;
    Ok(Some(CallbackPayload {
        status,
        currency,
        amount,
        logs,
    }))
}

fn inbound_log(
    integration: &Integration,
    inbound: &InboundCallback,
    runtime: &Runtime,
    redactor: &Redactor,
) -> InteractionLog {
    let mut span = InteractionSpan::enter();
    let callback = inbound.to_value();
    let mut params = Map::new();
    params.insert("method".into(), Value::String(inbound.method.clone()));
    if !inbound.query.is_empty() {
        params.insert("query".into(), Value::Object(inbound.query.clone()));
    }
    params.insert("body".into(), callback["body"].clone());
    span.set_request(runtime.callback_url.clone(), Value::Object(params));
    span.finish(&integration.key, "callback", redactor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_body_is_json_then_a_form_then_text() {
        let json_body = InboundCallback {
            raw: r#"{"rrn":"R1"}"#.into(),
            ..Default::default()
        };
        assert_eq!(json_body.to_value()["body"], json!({"rrn": "R1"}));

        let form = InboundCallback {
            raw: "rrn=R1&note=paid+in%20full".into(),
            ..Default::default()
        };
        assert_eq!(
            form.to_value()["body"],
            json!({"rrn": "R1", "note": "paid in full"})
        );

        let text = InboundCallback {
            raw: "OK".into(),
            ..Default::default()
        };
        assert_eq!(text.to_value()["body"], json!("OK"));
        assert_eq!(text.to_value()["raw"], json!("OK"));
    }
}
