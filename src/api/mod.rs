//! The reactivepay-facing HTTP surface.

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use axum::Router;
use serde_json::{Map, Value};

use connect::{ConnectInput, ConnectResponse, MethodKind};

use crate::engine::callback::{execute_callback, parse_form, CallbackOutcome, InboundCallback};
use crate::engine::log::Redactor;
use crate::engine::platform::send_callback;
use crate::engine::{execute_method, Runtime};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    let mut router = Router::new();
    for kind in MethodKind::ALL {
        router = router.route(
            &format!("/{{key}}/{}", kind.as_str()),
            post(move |state, path, body| handle(kind, state, path, body)),
        );
    }
    router.route("/{key}/callback", any(callback))
}

async fn handle(
    kind: MethodKind,
    State(state): State<AppState>,
    Path(key): Path<String>,
    body: Bytes,
) -> Response {
    let raw: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(%key, %kind, error = %e, "malformed request body");
            return reply(ConnectResponse::failure(
                format!("malformed request body: {e}"),
                vec![],
            ));
        }
    };

    let redactor = Redactor::new([], ["pan".into(), "cvv".into()]);
    tracing::debug!(%key, %kind, body = %&redactor.redact(&raw), "inbound request");

    let input: ConnectInput = match serde_json::from_value(raw) {
        Ok(v) => v,
        Err(e) => {
            return reply(ConnectResponse::failure(
                format!("unexpected request shape: {e}"),
                vec![],
            ))
        }
    };

    let integration = match state.repo.try_load(&key).await {
        Ok(Some(doc)) => doc,
        Ok(None) => {
            return reply(ConnectResponse::failure(
                format!("unknown integration `{key}`"),
                vec![],
            ))
        }
        Err(e) => {
            tracing::error!(%key, error = %e, "could not load integration");
            // The document is unreadable, so we never contacted the gateway
            // and the failure is certain.
            return reply(ConnectResponse::failure(
                format!("could not load integration `{key}`: {e}"),
                vec![],
            ));
        }
    };

    let runtime = Runtime {
        callback_url: state.config.callback_url(&key),
        request_id: input.token().unwrap_or("unknown_tx_token").into(),
    };

    let response = execute_method(&state.engine, &integration, kind, &input, &runtime).await;
    reply(response)
}

#[tracing::instrument(skip_all, fields(%key))]
async fn callback(
    State(state): State<AppState>,
    Path(key): Path<String>,
    method: Method,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    let raw = String::from_utf8_lossy(&body).into_owned();
    tracing::debug!(body = %raw, "inbound callback");

    let integration = match state.repo.try_load(&key).await {
        Ok(Some(doc)) => doc,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "could not load integration");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let Some(cb) = integration.callback() else {
        tracing::warn!("callback received, but the integration defines none");
        return StatusCode::NOT_FOUND.into_response();
    };

    let inbound = InboundCallback {
        method: method.to_string(),
        headers: headers
            .iter()
            .filter_map(|(k, v)| {
                Some((
                    k.as_str().to_string(),
                    Value::String(v.to_str().ok()?.to_string()),
                ))
            })
            .collect(),
        query: query
            .as_deref()
            .and_then(parse_form)
            .unwrap_or_else(Map::new),
        raw,
    };
    let runtime = Runtime {
        callback_url: state.config.callback_url(&key),
        request_id: "callback".into(),
    };

    let reply = execute_callback(&state.engine, &integration, cb, &inbound, &runtime).await;
    let ack = || {
        let status = StatusCode::from_u16(reply.ack_status).unwrap_or(StatusCode::OK);
        match &reply.ack_body {
            Some(body) => (status, axum::Json(body.clone())).into_response(),
            None => status.into_response(),
        }
    };

    match &reply.outcome {
        CallbackOutcome::Unmatched { id } => {
            tracing::warn!(?id, "no stored context for this callback; dropping it");
            ack()
        }
        CallbackOutcome::Rejected => {
            tracing::warn!("callback failed `verify`");
            StatusCode::UNAUTHORIZED.into_response()
        }
        CallbackOutcome::Skipped { reason } => {
            tracing::debug!(%reason, "callback not forwarded");
            ack()
        }
        CallbackOutcome::Failed(e) => {
            tracing::error!(error = %e, "callback could not be handled");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        CallbackOutcome::Forward(fwd) => {
            let Some(platform) = &state.config.platform else {
                tracing::error!("SIGN_KEY is not set; cannot forward the callback");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };
            match send_callback(
                &state.engine.client,
                platform,
                &fwd.token,
                &fwd.merchant_private_key,
                &fwd.payload,
            )
            .await
            {
                Ok(()) => {
                    tracing::info!(token = %fwd.token, "callback forwarded");
                    ack()
                }
                Err(e) => {
                    tracing::error!(token = %fwd.token, error = %e, "callback forward failed");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
    }
}

fn reply(response: ConnectResponse) -> Response {
    // Always 200 (see the module docs).
    (axum::http::StatusCode::OK, axum::Json(response)).into_response()
}
