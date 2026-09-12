//! The reactivepay-facing HTTP surface.
//!
//! `POST /gw/{key}/{pay|payout|refund|status}` with the three-bucket envelope.
//!
//! Every reply is **HTTP 200**, including failures. The platform treats a
//! non-200 as a transport problem and retries it, so an integration error has
//! to arrive as `{"result": false, "error": …}` with a 200 status — including
//! the case where the request body itself is malformed, which is why this
//! module parses the body by hand instead of using `axum::Json`.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;

use connect::{ConnectInput, ConnectResponse, MethodKind};

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
    router
}

async fn handle(
    kind: MethodKind,
    State(state): State<AppState>,
    Path(key): Path<String>,
    body: Bytes,
) -> Response {
    // Parsed to `Value` first so the raw body can be logged before typing it,
    // matching how the hand-written adapters behave.
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
        request_id: request_id(&input),
    };

    let response = execute_method(&state.engine, &integration, kind, &input, &runtime).await;
    reply(response)
}

/// Correlates logs for one platform payment; the platform's own token is the
/// most useful handle, so prefer it over inventing an id.
fn request_id(input: &ConnectInput) -> String {
    input.token().unwrap_or("unknown").to_string()
}

fn reply(response: ConnectResponse) -> Response {
    // Always 200 — see the module docs.
    (axum::http::StatusCode::OK, axum::Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Repo;
    use crate::engine::EngineCx;
    use crate::spec::{Integration, Template};
    use crate::state::Config;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tower::ServiceExt;
    use wiremock::matchers::{method as m, path as p};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn app(base_url: &str) -> Router {
        let repo = Repo::new(crate::db::test_pool().await);
        let mut doc: Integration =
            serde_json::from_str(include_str!("../../fixtures/scripay.json")).unwrap();
        doc.base_url = Template::parse(base_url).unwrap();
        repo.save(&doc, None).await.unwrap();

        let state = AppState {
            leptos_options: leptos::prelude::LeptosOptions::builder()
                .output_name("test")
                .build(),
            repo,
            engine: Arc::new(EngineCx::new(reqwest::Client::new())),
            config: Arc::new(Config {
                callback_base: "https://cb.example".into(),
            }),
        };
        Router::new().nest("/gw", router()).with_state(state)
    }

    fn body() -> Body {
        Body::from(
            json!({
                "payment": { "token": "ORD1", "gateway_amount": 10000, "gateway_currency": "KES" },
                "params": { "phone": "254700000000" },
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

    #[tokio::test]
    async fn pay_runs_the_integration_end_to_end() {
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

        let (status, v) = post_to(app(&server.uri()).await, "/gw/scripay/pay", body()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["result"], json!(true));
        assert_eq!(v["status"], json!("pending"));
        assert_eq!(v["gateway_token"], json!("R1"));
        assert_eq!(v["logs"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_malformed_body_is_a_200_with_result_false() {
        let (status, v) = post_to(
            app("http://127.0.0.1:1").await,
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

    #[tokio::test]
    async fn an_unknown_integration_is_a_200_with_result_false() {
        let (status, v) = post_to(app("http://127.0.0.1:1").await, "/gw/nope/pay", body()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["result"], json!(false));
        assert!(v["error"]
            .as_str()
            .unwrap()
            .contains("unknown integration `nope`"));
    }

    #[tokio::test]
    async fn a_method_the_integration_does_not_define_is_reported() {
        let (_, v) = post_to(
            app("http://127.0.0.1:1").await,
            "/gw/scripay/refund",
            body(),
        )
        .await;
        assert_eq!(v["result"], json!(false));
        assert!(v["error"].as_str().unwrap().contains("refund"));
    }

    #[tokio::test]
    async fn all_four_methods_are_routed() {
        for kind in MethodKind::ALL {
            let uri = format!("/gw/scripay/{kind}");
            let (status, _) = post_to(app("http://127.0.0.1:1").await, &uri, body()).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
        }
    }

    #[tokio::test]
    async fn an_unroutable_path_is_a_real_404() {
        let res = app("http://127.0.0.1:1")
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
}
