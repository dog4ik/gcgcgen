//! A stand-in for a real payment gateway, for local development and manual
//! testing. **Nothing here talks to a real provider.**
//!
//! It speaks the shape used by `fixtures/scripay.json`, plus a set of
//! deliberately broken endpoints for exercising the failure paths that matter:
//! a 4xx (a certain decline), a 5xx and a non-JSON body (both uncertain, so
//! `pending`), and a hang for timeouts.
//!
//! ```text
//! cargo run --example stub_gateway --features ssr
//! ```
//!
//! Then point an integration's `base_url` at `http://127.0.0.1:8899`.

use std::time::Duration;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{any, post};
use axum::{Json, Router};
use serde_json::{json, Value};

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/v1/auth/access-token", post(access_token))
        .route("/v1/gateway/initiate/collection", post(initiate))
        .route("/v1/gateway/initiate/payout", post(initiate))
        .route("/v1/gateway/transactions/status", post(status))
        // Failure paths, addressed by name.
        .route("/fail/not-json", any(not_json))
        .route("/fail/hang", any(hang))
        .route("/fail/{code}", any(fail))
        .fallback(any(|| async {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "message": "no such endpoint" })),
            )
        }));

    let addr = std::env::var("STUB_ADDR").unwrap_or_else(|_| "127.0.0.1:8899".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    println!("stub gateway listening on http://{addr}");
    println!("  POST /v1/auth/access-token           -> a token");
    println!("  POST /v1/gateway/initiate/collection -> an rrn");
    println!("  POST /v1/gateway/initiate/payout     -> an rrn");
    println!("  POST /v1/gateway/transactions/status -> echoes the requested status");
    println!("  ANY  /fail/{{code}} | /fail/not-json | /fail/hang");
    axum::serve(listener, app).await.expect("serve");
}

async fn access_token(Json(body): Json<Value>) -> impl IntoResponse {
    if body
        .get("client_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .is_empty()
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "message": "client_id is required" })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({ "access_token": "stub-token", "expires_in": 3600 })),
    )
}

async fn initiate(Json(body): Json<Value>) -> impl IntoResponse {
    let order_id = body
        .get("order_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    (
        StatusCode::OK,
        Json(json!({
            "rrn": format!("RRN-{order_id}"),
            "order_id": order_id,
            "amount": body.get("amount").cloned().unwrap_or(json!(0)),
            "currency": "KES"
        })),
    )
}

/// Echoes whichever status the caller asks for, so every branch of a status
/// map can be exercised without waiting on a real transaction.
async fn status(Json(body): Json<Value>) -> impl IntoResponse {
    let state = body
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("Success");
    (
        StatusCode::OK,
        Json(json!({
            "rrn": body.get("rrn").cloned().unwrap_or(json!("unknown")),
            "status": state,
            "amount": 100.0,
            "currency": "KES",
            "narration": format!("stub says {state}")
        })),
    )
}

async fn fail(Path(code): Path<String>) -> impl IntoResponse {
    let code = code
        .parse::<u16>()
        .ok()
        .and_then(|c| StatusCode::from_u16(c).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        code,
        Json(json!({ "message": format!("stub failure {}", code.as_u16()) })),
    )
}

async fn not_json() -> impl IntoResponse {
    (
        StatusCode::BAD_GATEWAY,
        [("content-type", "text/html")],
        "<html><body>bad gateway</body></html>",
    )
}

async fn hang() -> impl IntoResponse {
    tokio::time::sleep(Duration::from_secs(120)).await;
    StatusCode::OK
}
