//! The outbound response envelope.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::log::InteractionLog;
use crate::status::Status;

/// The transaction facts, flattened into a successful reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionResponse {
    pub status: Status,
    /// The gateway's identifier for the transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_token: Option<String>,
    /// Minor units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_request: Option<RedirectRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requisites: Option<Map<String, Value>>,
}

/// The handover to the gateway's own page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RedirectRequest {
    Post {
        url: String,
        #[serde(default)]
        params: Map<String, Value>,
    },
    Get {
        url: String,
    },
    GetWithProcessing {
        url: String,
    },
    PostIframes {
        iframes: Vec<Iframe>,
    },
    RedirectHtml {
        html: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Iframe {
    pub url: String,
    #[serde(default)]
    pub data: Map<String, Value>,
}

impl TransactionResponse {
    pub fn new(status: Status) -> Self {
        Self {
            status,
            gateway_token: None,
            amount: None,
            currency: None,
            details: None,
            redirect_request: None,
            requisites: None,
        }
    }

    /// The reply for a call whose outcome could not be determined.
    pub fn uncertain(details: impl Into<String>) -> Self {
        Self::new(Status::Pending).with_details(details)
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

/// What the platform receives.
///
/// Response should always be sent with HTTP 200
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConnectResponse {
    Success(SuccessResponse),
    Failure(FailureResponse),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuccessResponse {
    /// Always `true`.
    pub result: bool,
    pub logs: Vec<InteractionLog>,
    #[serde(flatten)]
    pub transaction: TransactionResponse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailureResponse {
    /// Always `false`.
    pub result: bool,
    pub error: String,
    pub logs: Vec<InteractionLog>,
}

impl ConnectResponse {
    pub fn success(transaction: TransactionResponse, logs: Vec<InteractionLog>) -> Self {
        ConnectResponse::Success(SuccessResponse {
            result: true,
            logs,
            transaction,
        })
    }

    pub fn failure(error: impl Into<String>, logs: Vec<InteractionLog>) -> Self {
        ConnectResponse::Failure(FailureResponse {
            result: false,
            error: error.into(),
            logs,
        })
    }

    pub fn is_success(&self) -> bool {
        matches!(self, ConnectResponse::Success(_))
    }

    pub fn logs(&self) -> &[InteractionLog] {
        match self {
            ConnectResponse::Success(s) => &s.logs,
            ConnectResponse::Failure(f) => &f.logs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn success_flattens_the_transaction() {
        let r = ConnectResponse::success(
            TransactionResponse {
                gateway_token: Some("rrn-1".into()),
                amount: Some(1000),
                currency: Some("KES".into()),
                ..TransactionResponse::new(Status::Pending)
            },
            vec![],
        );
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({
                "result": true, "logs": [],
                "status": "pending", "gateway_token": "rrn-1",
                "amount": 1000, "currency": "KES"
            })
        );
    }

    #[test]
    fn absent_fields_are_omitted_not_nulled() {
        let r = ConnectResponse::success(TransactionResponse::new(Status::Approved), vec![]);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v, json!({"result": true, "logs": [], "status": "approved"}));
        assert!(v.get("gateway_token").is_none());
    }

    #[test]
    fn a_redirect_is_tagged_by_type() {
        let r = ConnectResponse::success(
            TransactionResponse {
                redirect_request: Some(RedirectRequest::Post {
                    url: "https://pay.example/hosted".into(),
                    params: json!({"order": "ORD1"}).as_object().unwrap().clone(),
                }),
                requisites: Some(
                    json!({"account": "0011", "bank": "KCB"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
                ..TransactionResponse::new(Status::Pending)
            },
            vec![],
        );
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(
            v["redirect_request"],
            json!({"type": "post", "url": "https://pay.example/hosted",
                   "params": {"order": "ORD1"}})
        );
        assert_eq!(v["requisites"], json!({"account": "0011", "bank": "KCB"}));
    }

    #[test]
    fn redirect_variants_use_the_platform_spelling() {
        let tags = [
            RedirectRequest::Get { url: "u".into() },
            RedirectRequest::GetWithProcessing { url: "u".into() },
            RedirectRequest::PostIframes {
                iframes: vec![Iframe {
                    url: "u".into(),
                    data: Map::new(),
                }],
            },
            RedirectRequest::RedirectHtml {
                html: "<form/>".into(),
            },
        ]
        .map(|r| serde_json::to_value(r).unwrap()["type"].clone());
        assert_eq!(
            tags,
            [
                "get",
                "get_with_processing",
                "post_iframes",
                "redirect_html"
            ]
            .map(|s| json!(s))
        );
    }

    #[test]
    fn failure_carries_the_error_and_the_logs() {
        let r = ConnectResponse::failure("gateway said no", vec![]);
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"result": false, "error": "gateway said no", "logs": []})
        );
        assert!(!r.is_success());
    }

    #[test]
    fn an_uncertain_outcome_reports_no_gateway_token() {
        let t = TransactionResponse::uncertain("uncertain outcome: connection reset");
        assert_eq!(t.status, Status::Pending);
        assert_eq!(t.gateway_token, None);
    }
}
