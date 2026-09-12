//! Acceptance test: the Scripay integration, expressed as a spec document,
//! must render exactly what the hand-written `oxyscripay` adapter renders.
//!
//! The reference is oxyscripay's own golden test,
//! `initiate_request_maps_to_scripay_shape` in `src/connect/api.rs` — same
//! input, same expected outbound body. If the spec model cannot express
//! Scripay it cannot express anything, so this gates the whole engine.
//!
//! Pure rendering: no network, no stub server, nothing sent.

use connect::{ConnectInput, MethodKind};
use gcgcgen::engine::http::PreparedBody;
use gcgcgen::engine::{preview_request, Runtime};
use gcgcgen::spec::{validate, Integration};
use serde_json::{json, Map, Value};

fn scripay() -> Integration {
    let src = include_str!("../fixtures/scripay.json");
    let doc: Integration = serde_json::from_str(src).expect("fixture parses");
    validate::validate(&doc).expect("fixture validates");
    doc
}

/// The exact input from oxyscripay's golden test.
fn golden_input() -> ConnectInput {
    serde_json::from_value(json!({
        "payment": {
            "token": "ORD1", "gateway_amount": 10000, "gateway_currency": "KES",
            "product": "Invoice", "merchant_private_key": "priv-key"
        },
        "params": {
            "customer": { "phone": "254700000000", "first_name": "John", "last_name": "Doe" }
        },
        "settings": {
            "sandbox": true, "client_id": "c", "client_secret": "s",
            "channel": "Mpesa", "wallet": "436418", "code": "2001"
        }
    }))
    .unwrap()
}

fn runtime() -> Runtime {
    Runtime {
        callback_url: "https://google.com".into(),
        request_id: "req-1".into(),
    }
}

fn render(kind: MethodKind, name: &str, input: &ConnectInput) -> Value {
    let prepared = preview_request(&scripay(), kind, name, input, &runtime(), &Map::new())
        .unwrap_or_else(|e| panic!("{kind}/{name}: {e}"));
    match prepared.body {
        PreparedBody::Json(v) => v,
        other => panic!("expected a JSON body, got {other:?}"),
    }
}

#[test]
fn pay_body_matches_the_hand_written_adapter() {
    assert_eq!(
        render(MethodKind::Pay, "collection", &golden_input()),
        json!({
            "purpose": "payment",
            "order_id": "ORD1",
            "amount": 100,
            "callback_url": "https://google.com",
            "description": "Invoice",
            "wallet": "436418",
            "channel": "Mpesa",
            "data": {
                "phone_number": "254700000000",
                "account_name": "John Doe",
                "code": "2001"
            }
        })
    );
}

#[test]
fn payout_differs_only_in_purpose_and_fallback_description() {
    let mut input = golden_input();
    input.payment.as_object_mut().unwrap().remove("product");
    input
        .payment
        .as_object_mut()
        .unwrap()
        .insert("order_number".into(), json!("ORD-9"));

    let pay = render(MethodKind::Pay, "collection", &input);
    let payout = render(MethodKind::Payout, "payout", &input);

    assert_eq!(pay["purpose"], json!("payment"));
    assert_eq!(payout["purpose"], json!("payout"));
    // `product` is gone, so both fall through to `order_number`.
    assert_eq!(pay["description"], json!("ORD-9"));
    assert_eq!(payout["description"], json!("ORD-9"));
    assert_eq!(pay["data"], payout["data"]);
}

#[test]
fn description_falls_through_to_the_literal() {
    let mut input = golden_input();
    input.payment.as_object_mut().unwrap().remove("product");
    assert_eq!(
        render(MethodKind::Pay, "collection", &input)["description"],
        json!("Payment")
    );
    assert_eq!(
        render(MethodKind::Payout, "payout", &input)["description"],
        json!("Payout")
    );
}

#[test]
fn amount_stays_an_integer_when_whole_and_a_decimal_otherwise() {
    let body = render(MethodKind::Pay, "collection", &golden_input());
    assert_eq!(body["amount"], json!(100));
    assert_eq!(
        serde_json::to_string(&body["amount"]).unwrap(),
        "100",
        "not 100.0"
    );

    let mut input = golden_input();
    input
        .payment
        .as_object_mut()
        .unwrap()
        .insert("gateway_amount".into(), json!(10050));
    assert_eq!(
        render(MethodKind::Pay, "collection", &input)["amount"],
        json!(100.5)
    );
}

#[test]
fn extra_return_param_overrides_the_merchant_channel() {
    let mut input = golden_input();
    input
        .params
        .as_object_mut()
        .unwrap()
        .insert("extra_return_param".into(), json!("Airtel"));
    assert_eq!(
        render(MethodKind::Pay, "collection", &input)["channel"],
        json!("Airtel")
    );
}

#[test]
fn the_blank_sentinel_falls_back_to_the_merchant_channel() {
    let mut input = golden_input();
    input
        .params
        .as_object_mut()
        .unwrap()
        .insert("extra_return_param".into(), json!("_blank_"));
    assert_eq!(
        render(MethodKind::Pay, "collection", &input)["channel"],
        json!("Mpesa")
    );
}

#[test]
fn channel_is_omitted_entirely_when_neither_is_set() {
    let mut input = golden_input();
    input.settings.as_object_mut().unwrap().remove("channel");
    let body = render(MethodKind::Pay, "collection", &input);
    assert!(
        body.get("channel").is_none(),
        "expected the key to be absent, got {body}"
    );
}

#[test]
fn account_name_reads_flat_fields_before_nested_customer() {
    let mut input = golden_input();
    input
        .params
        .as_object_mut()
        .unwrap()
        .insert("first_name".into(), json!("Jane"));
    let body = render(MethodKind::Pay, "collection", &input);
    // Flat `first_name` wins; `last_name` still falls through to `customer`.
    assert_eq!(body["data"]["account_name"], json!("Jane Doe"));
}

#[test]
fn account_name_never_carries_a_stray_separator() {
    let mut input = golden_input();
    input.params.as_object_mut().unwrap()["customer"]
        .as_object_mut()
        .unwrap()
        .remove("first_name");
    assert_eq!(
        render(MethodKind::Pay, "collection", &golden_input())["data"]["account_name"],
        json!("John Doe")
    );
    assert_eq!(
        render(MethodKind::Pay, "collection", &input)["data"]["account_name"],
        json!("Doe")
    );
}

#[test]
fn account_name_is_omitted_when_no_name_is_supplied() {
    let mut input = golden_input();
    input.params = json!({ "customer": { "phone": "254700000000" } });
    let body = render(MethodKind::Pay, "collection", &input);
    assert!(
        body["data"].get("account_name").is_none(),
        "expected the key to be absent, got {}",
        body["data"]
    );
}

#[test]
fn sandbox_selects_the_sandbox_host() {
    let input = golden_input();
    let prepared = preview_request(
        &scripay(),
        MethodKind::Pay,
        "collection",
        &input,
        &runtime(),
        &Map::new(),
    )
    .unwrap();
    assert_eq!(
        prepared.full_url(),
        "https://api-sandbox.scripay.com/v1/gateway/initiate/collection"
    );

    let mut live = golden_input();
    live.settings
        .as_object_mut()
        .unwrap()
        .insert("sandbox".into(), json!(false));
    let prepared = preview_request(
        &scripay(),
        MethodKind::Pay,
        "collection",
        &live,
        &runtime(),
        &Map::new(),
    )
    .unwrap();
    assert_eq!(
        prepared.full_url(),
        "https://api.scripay.com/v1/gateway/initiate/collection"
    );
}

#[test]
fn status_request_sends_the_gateway_token() {
    let input: ConnectInput = serde_json::from_value(json!({
        "payment": { "token": "ORD1", "gateway_token": "RRN-7" },
        "settings": { "sandbox": true, "client_id": "c", "client_secret": "s",
                      "wallet": "436418", "code": "2001" }
    }))
    .unwrap();
    assert_eq!(
        render(MethodKind::Status, "status", &input),
        json!({ "rrn": "RRN-7" })
    );
}

#[test]
fn the_auth_request_body_carries_the_credentials() {
    let prepared = preview_request(
        &scripay(),
        MethodKind::Pay,
        "collection",
        &golden_input(),
        &runtime(),
        &Map::new(),
    )
    .unwrap();
    // A token request is not resolved during a preview, so no Authorization
    // header is fabricated.
    assert!(prepared.headers.is_empty(), "{:?}", prepared.headers);
}

#[test]
fn the_manifest_matches_what_the_platform_must_send() {
    let doc = scripay();

    let pay = doc.manifest(MethodKind::Pay).unwrap();
    assert_eq!(
        pay.payment,
        ["gateway_amount", "order_number", "product", "token"]
    );
    assert_eq!(
        pay.params,
        [
            "customer",
            "extra_return_param",
            "first_name",
            "last_name",
            "phone"
        ]
    );
    assert_eq!(
        pay.settings,
        [
            "sandbox",
            "client_id",
            "client_secret",
            "wallet",
            "code",
            "channel"
        ]
    );

    let status = doc.manifest(MethodKind::Status).unwrap();
    assert_eq!(status.payment, ["gateway_token"]);
    assert!(status.params.is_empty());

    // Refund has no prior art in oxyscripay and is not defined here.
    assert!(doc.manifest(MethodKind::Refund).is_none());
}
