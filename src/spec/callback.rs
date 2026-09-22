use serde::{Deserialize, Serialize};

use super::expr::Expr;
use super::request::RequestDef;
use super::ResultMapping;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackDef {
    /// The id the stored context is found by
    pub lookup: Expr,
    /// Falsy rejects the callback without forwarding anything.
    #[serde(default)]
    pub verify: Option<Expr>,
    #[serde(default)]
    pub requests: Vec<RequestDef>,
    /// Only `status`, `amount`, `currency` and `details` apply.
    pub result: ResultMapping,
    #[serde(default)]
    pub ack: AckDef,
}

impl CallbackDef {
    /// Every expression the callback itself owns, outside its requests.
    pub fn exprs<'a>(&'a self, out: &mut Vec<&'a Expr>) {
        out.push(&self.lookup);
        out.extend(self.verify.iter());
        self.result.exprs(out);
        out.extend(self.ack.body.iter());
    }
}

/// The reply to the gateway once the callback has been handled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckDef {
    #[serde(default = "ok")]
    pub status: u16,
    #[serde(default)]
    pub body: Option<Expr>,
}

fn ok() -> u16 {
    200
}

impl Default for AckDef {
    fn default() -> Self {
        Self {
            status: ok(),
            body: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_a_bare_200_ack() {
        let c: CallbackDef = serde_json::from_str(
            r#"{ "lookup": "callback.body.rrn",
                 "result": { "status": "\"approved\"",
                             "amount": "callback.body.amount",
                             "currency": "callback.body.currency" } }"#,
        )
        .unwrap();
        assert!(c.requests.is_empty());
        assert_eq!(c.ack, AckDef::default());
        let back: CallbackDef = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(c, back);
    }
}
