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
    tracing::debug!(%key, %kind, body = %raw, "inbound request");

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Repo;
    use crate::engine::EngineCx;
    use crate::spec::{Expr, Integration};
    use crate::state::Config;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{json, Value};
    use sqlx::SqlitePool;
    use std::sync::Arc;
    use tower::ServiceExt;
    use wiremock::matchers::{method as m, path as p};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn app(base_url: &str, pool: SqlitePool) -> Router {
        let repo = Repo::new(pool);
        let mut doc: Integration =
            serde_json::from_str(include_str!("../../fixtures/scripay.json")).unwrap();
        doc.base_url = Expr::literal(base_url);
        repo.save(&doc, None).await.unwrap();

        let state = AppState {
            leptos_options: leptos::prelude::LeptosOptions::builder()
                .output_name("test")
                .build(),
            repo,
            engine: Arc::new(EngineCx::new(reqwest::Client::new())),
            config: Arc::new(Config {
                callback_base: "https://cb.example".into(),
                platform: None,
            }),
        };
        Router::new().nest("/gw", router()).with_state(state)
    }

    fn body() -> Body {
        Body::from(
            json!({
                "payment": { "token": "ORD1", "gateway_amount": 10000, "gateway_currency": "KES" },
                "params": { "phone": "88005553535" },
                "settings": { "sandbox": true, "client_id": "c", "client_secret": "s",
                              "wallet": "w", "code": "2001" }
            })
            .to_string(),
        )
    }

    async fn post_to(app: Router, uri: &str, body: Body) -> (StatusCode, Value) {
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[sqlx::test]
    async fn pay_runs_the_integration_end_to_end(pool: SqlitePool) {
        let server = MockServer::start().await;
        Mock::given(m("POST"))
            .and(p("/v1/auth/access-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token": "T"})))
            .mount(&server)
            .await;
        Mock::given(m("POST"))
            .and(p("/v1/gateway/initiate/collection"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"rrn": "R1", "amount": 100.0, "currency": "KES"})),
            )
            .mount(&server)
            .await;

        let (status, v) = post_to(app(&server.uri(), pool).await, "/gw/scripay/pay", body()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["result"], json!(true));
        assert_eq!(v["status"], json!("pending"));
        assert_eq!(v["gateway_token"], json!("R1"));
        assert_eq!(v["logs"].as_array().unwrap().len(), 2);
    }

    #[sqlx::test]
    async fn a_malformed_body_is_a_200_with_result_false(pool: SqlitePool) {
        let (status, v) = post_to(
            app("http://127.0.0.1:1", pool.clone()).await,
            "/gw/scripay/pay",
            Body::from("{oops"),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a 4xx would make the platform retry"
        );
        assert_eq!(v["result"], json!(false));
        assert!(v["error"].as_str().unwrap().contains("malformed"));
        assert_eq!(v["logs"], json!([]));
    }

    #[sqlx::test]
    async fn an_unknown_integration_is_a_200_with_result_false(pool: SqlitePool) {
        let (status, v) = post_to(
            app("http://127.0.0.1:1", pool.clone()).await,
            "/gw/nope/pay",
            body(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["result"], json!(false));
        assert!(v["error"]
            .as_str()
            .unwrap()
            .contains("unknown integration `nope`"));
    }

    #[sqlx::test]
    async fn a_method_the_integration_does_not_define_is_reported(pool: SqlitePool) {
        let (_, v) = post_to(
            app("http://127.0.0.1:1", pool.clone()).await,
            "/gw/scripay/refund",
            body(),
        )
        .await;
        assert_eq!(v["result"], json!(false));
        assert!(v["error"].as_str().unwrap().contains("refund"));
    }

    #[sqlx::test]
    async fn all_four_methods_are_routed(pool: SqlitePool) {
        for kind in MethodKind::ALL {
            let uri = format!("/gw/scripay/{kind}");
            let (status, _) =
                post_to(app("http://127.0.0.1:1", pool.clone()).await, &uri, body()).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
        }
    }

    #[sqlx::test]
    async fn an_unroutable_path_is_a_real_404(pool: SqlitePool) {
        let res = app("http://127.0.0.1:1", pool)
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/gw/scripay/capture")
                    .body(body())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn the_callback_route_answers_the_gateway_with_real_status_codes(pool: SqlitePool) {
        let repo = Repo::new(pool);
        let doc: Integration = serde_json::from_value(json!({
            "key": "cbgw", "name": "G", "base_url": "'https://x.example'",
            "methods": { "pay": { "requests": [{ "name": "c", "path": "'/c'" }],
                                  "result": { "status": "\"pending\"" } } },
            "callback": { "lookup": "callback.body.id",
                          "result": { "status": "\"approved\"", "amount": "callback.body.a",
                                      "currency": "callback.body.c" },
                          "ack": { "status": 202, "body": "{\"ok\": true}" } }
        }))
        .unwrap();
        repo.save(&doc, None).await.unwrap();
        let state = AppState {
            leptos_options: leptos::prelude::LeptosOptions::builder()
                .output_name("test")
                .build(),
            repo,
            engine: Arc::new(EngineCx::new(reqwest::Client::new())),
            config: Arc::new(Config {
                callback_base: "https://cb.example".into(),
                platform: None,
            }),
        };
        let app = Router::new().nest("/gw", router()).with_state(state);

        // Nothing is waiting on this id: acknowledged, not retried.
        let (status, v) = post_to(
            app.clone(),
            "/gw/cbgw/callback",
            Body::from(json!({"id": "unknown"}).to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(v, json!({"ok": true}));

        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/gw/nope/callback")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}
