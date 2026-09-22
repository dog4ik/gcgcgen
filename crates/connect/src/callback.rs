use serde::{Deserialize, Serialize};

use crate::log::InteractionLog;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallbackPayload {
    #[serde(flatten)]
    pub status: CallbackStatus,
    pub currency: String,
    /// Minor units.
    pub amount: u64,
    pub logs: Vec<InteractionLog>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "status")]
pub enum CallbackStatus {
    Approved,
    Refunded,
    Declined { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialises_in_the_platform_shape() {
        let approved = CallbackPayload {
            status: CallbackStatus::Approved,
            currency: "KES".into(),
            amount: 1000,
            logs: vec![],
        };
        assert_eq!(
            serde_json::to_value(&approved).unwrap(),
            json!({"status": "approved", "currency": "KES", "amount": 1000, "logs": []})
        );

        let declined = CallbackPayload {
            status: CallbackStatus::Declined {
                reason: "insufficient funds".into(),
            },
            ..approved
        };
        assert_eq!(
            serde_json::to_value(&declined).unwrap(),
            json!({"status": "declined", "reason": "insufficient funds",
                   "currency": "KES", "amount": 1000, "logs": []})
        );
    }
}
