//! The outbound response envelope.

use serde::{Deserialize, Serialize};

use crate::log::InteractionLog;
use crate::status::Status;

/// The transaction facts, flattened into a successful reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionResponse {
    pub status: Status,
    /// The gateway's own identifier for the transaction.
    ///
    /// Deliberately absent on an uncertain outcome: if the call may not have
    /// reached the gateway there is no identifier to report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_token: Option<String>,
    /// Minor units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl TransactionResponse {
    pub fn new(status: Status) -> Self {
        Self {
            status,
            gateway_token: None,
            amount: None,
            currency: None,
            details: None,
        }
    }

    /// The reply for a call whose outcome could not be determined.
    pub fn uncertain(details: impl Into<String>) -> Self {
        Self {
            status: Status::Pending,
            gateway_token: None,
            amount: None,
            currency: None,
            details: Some(details.into()),
        }
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

/// What the platform receives.
///
/// **Always sent with HTTP 200**, including failures: a non-200 is treated as
/// a transport problem and retried rather than recorded, so an integration
/// error has to arrive as `{"result": false, …}` with a 200 status.
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
                status: Status::Pending,
                gateway_token: Some("rrn-1".into()),
                amount: Some(1000),
                currency: Some("KES".into()),
                details: None,
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
