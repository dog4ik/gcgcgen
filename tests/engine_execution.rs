//! End-to-end execution against a local stub. No real gateway is contacted.
//!
//! The bulk of this file is the error matrix, because "which failures may
//! report `declined`" is the one question this system cannot afford to get
//! wrong: a transport error, a 5xx, or an unreadable body all mean the gateway
//! may have taken the money, and must surface as `pending`.

use connect::{ConnectInput, ConnectResponse, MethodKind, Status};
use gcgcgen::engine::{execute_method, EngineCx, Runtime};
use gcgcgen::spec::{validate, Integration, Template};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn scripay_at(base: &str) -> Integration {
    let mut doc: Integration =
        serde_json::from_str(include_str!("../fixtures/scripay.json")).unwrap();
    validate::validate(&doc).expect("fixture validates");
    doc.base_url = Template::parse(base).unwrap();
    doc
}

fn input() -> ConnectInput {
    serde_json::from_value(json!({
        "payment": {
            "token": "ORD1", "gateway_amount": 10000, "gateway_currency": "KES",
            "product": "Invoice"
        },
        "params": { "customer": { "phone": "254700000000", "first_name": "John", "last_name": "Doe" } },
        "settings": {
            "sandbox": true, "client_id": "c", "client_secret": "sk_live_supersecret",
            "channel": "Mpesa", "wallet": "436418", "code": "2001"
        }
    }))
    .unwrap()
}

fn runtime() -> Runtime {
    Runtime {
        callback_url: "https://cb.example/callback".into(),
        request_id: "req-1".into(),
    }
}

fn cx() -> EngineCx {
    EngineCx::new(reqwest::Client::new())
}

async fn mock_auth(server: &MockServer, times: u64) {
    Mock::given(method("POST"))
        .and(path("/v1/auth/access-token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access_token": "TOK", "expires_in": 3600 })),
        )
        .expect(times)
        .mount(server)
        .await;
}

fn success(r: &ConnectResponse) -> &connect::TransactionResponse {
    match r {
        ConnectResponse::Success(s) => &s.transaction,
        ConnectResponse::Failure(f) => panic!("expected success, got failure: {}", f.error),
    }
}

fn failure(r: &ConnectResponse) -> &str {
    match r {
        ConnectResponse::Failure(f) => &f.error,
        ConnectResponse::Success(s) => {
            panic!("expected failure, got {:?}", s.transaction.status)
        }
    }
}

// ---------------------------------------------------------------------------
// happy path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pay_authenticates_then_collects_and_logs_both_calls() {
    let server = MockServer::start().await;
    mock_auth(&server, 1).await;
    Mock::given(method("POST"))
        .and(path("/v1/gateway/initiate/collection"))
        .and(header("authorization", "Bearer TOK"))
        .and(body_json(json!({
            "purpose": "payment", "order_id": "ORD1", "amount": 100,
            "callback_url": "https://cb.example/callback", "description": "Invoice",
            "wallet": "436418", "channel": "Mpesa",
            "data": { "phone_number": "254700000000", "account_name": "John Doe", "code": "2001" }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "rrn": "RRN-7", "amount": 100.0, "currency": "KES", "order_id": "ORD1" }),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let doc = scripay_at(&server.uri());
    let r = execute_method(&cx(), &doc, MethodKind::Pay, &input(), &runtime()).await;

    let t = success(&r);
    assert_eq!(t.status, Status::Pending);
    assert_eq!(t.gateway_token.as_deref(), Some("RRN-7"));
    assert_eq!(t.amount, Some(10000), "echoed back in minor units");
    assert_eq!(t.currency.as_deref(), Some("KES"));

    let logs = r.logs();
    assert_eq!(
        logs.len(),
        2,
        "the auth call is logged alongside the business call"
    );
    assert_eq!(logs[0].kind, "auth");
    assert_eq!(logs[1].kind, "collection");
    assert_eq!(logs[0].status, Some(200));
    assert!(logs.iter().all(|l| l.gateway == "scripay"));
}

#[tokio::test]
async fn secrets_never_reach_the_logs() {
    let server = MockServer::start().await;
    mock_auth(&server, 1).await;
    Mock::given(method("POST"))
        .and(path("/v1/gateway/initiate/collection"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "rrn": "R" })))
        .mount(&server)
        .await;

    let r = execute_method(
        &cx(),
        &scripay_at(&server.uri()),
        MethodKind::Pay,
        &input(),
        &runtime(),
    )
    .await;

    let dumped = serde_json::to_string(r.logs()).unwrap();
    assert!(
        !dumped.contains("sk_live_supersecret"),
        "the client_secret leaked into the audit trail: {dumped}"
    );
    assert!(
        dumped.contains("redacted"),
        "expected a redaction marker in {dumped}"
    );
    // A non-secret setting is still visible, so the log stays useful.
    assert!(dumped.contains("\"client_id\":\"c\""), "{dumped}");
}

#[tokio::test]
async fn status_maps_the_gateway_vocabulary_onto_the_canonical_one() {
    for (gateway_status, expected) in [
        ("Success", Status::Approved),
        ("Completed", Status::Approved),
        ("Failed", Status::Declined),
        ("Cancelled", Status::Declined),
        ("Processing", Status::Pending),
        // An unrecognised value must fall through to pending, never declined.
        ("SomethingNew", Status::Pending),
    ] {
        let server = MockServer::start().await;
        mock_auth(&server, 1).await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway/transactions/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": gateway_status, "amount": 100.0, "currency": "KES",
                "narration": "ok"
            })))
            .mount(&server)
            .await;

        let mut i = input();
        i.payment
            .as_object_mut()
            .unwrap()
            .insert("gateway_token".into(), json!("RRN-7"));
        let r = execute_method(
            &cx(),
            &scripay_at(&server.uri()),
            MethodKind::Status,
            &i,
            &runtime(),
        )
        .await;
        assert_eq!(
            success(&r).status,
            expected,
            "gateway status `{gateway_status}`"
        );
    }
}

// ---------------------------------------------------------------------------
// the error matrix
// ---------------------------------------------------------------------------

async fn pay_against(status: u16, body: ResponseTemplate) -> ConnectResponse {
    let server = MockServer::start().await;
    mock_auth(&server, 1).await;
    Mock::given(method("POST"))
        .and(path("/v1/gateway/initiate/collection"))
        .respond_with(body)
        .mount(&server)
        .await;
    let _ = status;
    execute_method(
        &cx(),
        &scripay_at(&server.uri()),
        MethodKind::Pay,
        &input(),
        &runtime(),
    )
    .await
}

#[tokio::test]
async fn a_5xx_is_pending_because_the_gateway_may_have_charged() {
    for code in [500, 502, 503] {
        let r = pay_against(
            code,
            ResponseTemplate::new(code).set_body_json(json!({"message": "boom"})),
        )
        .await;
        let t = success(&r);
        assert_eq!(t.status, Status::Pending, "HTTP {code}");
        assert_eq!(
            t.gateway_token, None,
            "no token can be claimed for an unknown outcome"
        );
        assert!(
            t.details
                .as_deref()
                .unwrap_or_default()
                .starts_with("uncertain outcome"),
            "{:?}",
            t.details
        );
    }
}

#[tokio::test]
async fn a_4xx_is_a_certain_failure_and_surfaces_the_gateway_message() {
    let r = pay_against(
        400,
        ResponseTemplate::new(400).set_body_json(json!({"message": "bad wallet"})),
    )
    .await;
    let e = failure(&r);
    assert!(e.contains("bad wallet"), "{e}");
    assert!(e.contains("collection"), "the failing step is named: {e}");
    assert_eq!(r.logs().len(), 2, "logs survive a failure");
}

#[tokio::test]
async fn an_unreadable_body_is_pending_when_the_status_says_failure() {
    let r = pay_against(
        502,
        ResponseTemplate::new(502)
            .set_body_string("<html>bad gateway</html>")
            .insert_header("content-type", "text/html"),
    )
    .await;
    assert_eq!(success(&r).status, Status::Pending);
    // The HTML body is still captured for the audit trail.
    let logs = r.logs();
    assert_eq!(logs[1].response, Some(json!("<html>bad gateway</html>")));
}

#[tokio::test]
async fn a_transport_error_is_pending() {
    // Port 1 refuses connections immediately.
    let doc = scripay_at("http://127.0.0.1:1");
    let r = execute_method(&cx(), &doc, MethodKind::Pay, &input(), &runtime()).await;
    let t = success(&r);
    assert_eq!(t.status, Status::Pending);
    assert!(t
        .details
        .as_deref()
        .unwrap_or_default()
        .contains("uncertain outcome"));
    // Even a request that never left the process is logged as an attempt.
    assert_eq!(r.logs().len(), 1);
    assert_eq!(r.logs()[0].kind, "auth");
    assert_eq!(r.logs()[0].status, None);
}

#[tokio::test]
async fn a_failing_auth_call_fails_the_whole_method() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/access-token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"message": "bad client"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/gateway/initiate/collection"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let r = execute_method(
        &cx(),
        &scripay_at(&server.uri()),
        MethodKind::Pay,
        &input(),
        &runtime(),
    )
    .await;
    let e = failure(&r);
    assert!(e.contains("bad client"), "{e}");
    assert_eq!(r.logs().len(), 1, "only the auth attempt happened");
}

#[tokio::test]
async fn a_disabled_method_is_reported_rather_than_attempted() {
    let server = MockServer::start().await;
    let r = execute_method(
        &cx(),
        &scripay_at(&server.uri()),
        MethodKind::Refund,
        &input(),
        &runtime(),
    )
    .await;
    assert!(failure(&r).contains("refund"), "{}", failure(&r));
}

#[tokio::test]
async fn a_base_url_that_renders_to_a_non_url_is_refused_before_sending() {
    let mut doc = scripay_at("http://127.0.0.1:1");
    doc.base_url = Template::parse("{{ settings.wallet }}").unwrap();
    let r = execute_method(&cx(), &doc, MethodKind::Pay, &input(), &runtime()).await;
    assert!(
        failure(&r).contains("not an http(s) URL"),
        "{}",
        failure(&r)
    );
    assert!(r.logs().is_empty(), "nothing was attempted");
}

// ---------------------------------------------------------------------------
// auth caching
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_token_is_shared_across_methods() {
    let server = MockServer::start().await;
    // `expect(1)` is the assertion: pay and payout must not each fetch a token.
    mock_auth(&server, 1).await;
    Mock::given(method("POST"))
        .and(path("/v1/gateway/initiate/collection"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"rrn": "R1"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/gateway/initiate/payout"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"rrn": "R2"})))
        .mount(&server)
        .await;

    let cx = cx();
    let doc = scripay_at(&server.uri());
    execute_method(&cx, &doc, MethodKind::Pay, &input(), &runtime()).await;
    let r = execute_method(&cx, &doc, MethodKind::Payout, &input(), &runtime()).await;

    assert_eq!(success(&r).gateway_token.as_deref(), Some("R2"));
    assert_eq!(r.logs().len(), 1, "the second call reused the cached token");
    assert_eq!(r.logs()[0].kind, "payout");
}

#[tokio::test]
async fn two_merchants_never_share_a_token() {
    let server = MockServer::start().await;
    // Two distinct client_ids must produce two token fetches.
    mock_auth(&server, 2).await;
    Mock::given(method("POST"))
        .and(path("/v1/gateway/initiate/collection"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"rrn": "R"})))
        .mount(&server)
        .await;

    let cx = cx();
    let doc = scripay_at(&server.uri());
    execute_method(&cx, &doc, MethodKind::Pay, &input(), &runtime()).await;

    let mut other = input();
    other
        .settings
        .as_object_mut()
        .unwrap()
        .insert("client_id".into(), json!("other-merchant"));
    let r = execute_method(&cx, &doc, MethodKind::Pay, &other, &runtime()).await;

    assert_eq!(
        r.logs().len(),
        2,
        "the second merchant authenticated separately"
    );
    assert_eq!(cx.tokens.len(), 2);
}

// ---------------------------------------------------------------------------
// multi-step methods
// ---------------------------------------------------------------------------

/// A payout that must resolve an account reference before transferring — the
/// case a single request/response mapping cannot express.
fn two_step_payout(base: &str) -> Integration {
    let doc: Integration = serde_json::from_value(json!({
        "key": "twostep",
        "name": "Two step",
        "base_url": base,
        "settings": { "fields": [{ "name": "api_key", "secret": true }] },
        "auths": [{ "id": "k", "label": "API key", "kind": "header",
                    "name": "X-Api-Key", "value": "{{ settings.api_key }}" }],
        "methods": { "payout": {
            "requests": [
                { "name": "enquiry", "path": "/enquiry", "auth": "k",
                  "body": { "kind": "json", "template": { "phone": "{{ params.phone }}" } },
                  "response": { "error": { "message": "resp.body.message" } } },
                { "name": "transfer", "path": "/transfer", "auth": "k",
                  "body": { "kind": "json", "template": {
                      "reference": "{{ steps.enquiry.body.reference }}",
                      "name": "{{ steps.enquiry.body.account_name }}",
                      "amount": "{{ payment.gateway_amount | minor_to_major }}" } },
                  "response": { "error": { "message": "resp.body.message" } } }
            ],
            "result": {
                "status": "steps.transfer.body.state | map({ done: 'approved', _default: 'pending' })",
                "gateway_token": "steps.transfer.body.id"
            } } }
    }))
    .unwrap();
    validate::validate(&doc).expect("two-step fixture validates");
    doc
}

fn two_step_input() -> ConnectInput {
    serde_json::from_value(json!({
        "payment": { "token": "T1", "gateway_amount": 5000 },
        "params": { "phone": "254700000000" },
        "settings": { "api_key": "key_abcdef123" }
    }))
    .unwrap()
}

#[tokio::test]
async fn the_second_request_consumes_the_first_ones_output() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/enquiry"))
        .and(header("x-api-key", "key_abcdef123"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "reference": "ACC-42", "account_name": "John Doe" })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/transfer"))
        .and(body_json(
            json!({ "reference": "ACC-42", "name": "John Doe", "amount": 50 }),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "id": "TX-9", "state": "done" })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let r = execute_method(
        &cx(),
        &two_step_payout(&server.uri()),
        MethodKind::Payout,
        &two_step_input(),
        &runtime(),
    )
    .await;

    let t = success(&r);
    assert_eq!(t.status, Status::Approved);
    assert_eq!(t.gateway_token.as_deref(), Some("TX-9"));
    assert_eq!(r.logs().len(), 2);
    assert_eq!(r.logs()[0].kind, "enquiry");
    assert_eq!(r.logs()[1].kind, "transfer");
}

#[tokio::test]
async fn a_certain_failure_in_step_one_stops_the_sequence() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/enquiry"))
        .respond_with(
            ResponseTemplate::new(422).set_body_json(json!({"message": "no such account"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/transfer"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0) // the transfer must never be attempted
        .mount(&server)
        .await;

    let r = execute_method(
        &cx(),
        &two_step_payout(&server.uri()),
        MethodKind::Payout,
        &two_step_input(),
        &runtime(),
    )
    .await;

    let e = failure(&r);
    assert!(e.contains("no such account"), "{e}");
    assert!(e.starts_with("enquiry:"), "the failing step is named: {e}");
    assert_eq!(r.logs().len(), 1);
}

#[tokio::test]
async fn an_uncertain_failure_in_step_two_is_pending() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/enquiry"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "reference": "ACC-42" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/transfer"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"message": "down"})))
        .mount(&server)
        .await;

    let r = execute_method(
        &cx(),
        &two_step_payout(&server.uri()),
        MethodKind::Payout,
        &two_step_input(),
        &runtime(),
    )
    .await;

    let t = success(&r);
    assert_eq!(
        t.status,
        Status::Pending,
        "the transfer may have gone through"
    );
    assert_eq!(t.gateway_token, None);
    assert_eq!(r.logs().len(), 2);
}
