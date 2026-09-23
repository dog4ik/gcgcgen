//! The integration document.

pub mod auth;
pub mod callback;
pub mod expr;
pub mod request;
pub mod settings;
pub mod validate;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub use connect::{Iframe, Manifest, MethodKind, RedirectRequest, Status};

pub use auth::{AuthDef, AuthId, AuthKind};
pub use callback::{AckDef, CallbackDef};
pub use expr::{EvalError, Expr, ExprError};
pub use request::{Body, Envelope, HttpMethod, NameValue, OnError, RequestDef, ResponseSpec};
pub use settings::{FieldDef, SettingsSchema};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Integration {
    /// URL segment for the integration
    pub key: String,
    pub name: String,
    pub base_url: Expr,
    #[serde(default)]
    pub settings: SettingsSchema,
    #[serde(default)]
    pub auths: Vec<AuthDef>,
    #[serde(default)]
    pub methods: BTreeMap<MethodKind, MethodDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback: Option<CallbackDef>,
    /// List of keys that must be concealed in logs
    #[serde(default)]
    pub redacted_key_list: Vec<String>,
}

impl Integration {
    pub fn auth(&self, id: &AuthId) -> Option<&AuthDef> {
        self.auths.iter().find(|a| a.id == *id)
    }

    pub fn method(&self, kind: MethodKind) -> Option<&MethodDef> {
        self.methods.get(&kind)
    }

    pub fn callback(&self) -> Option<&CallbackDef> {
        self.callback.as_ref()
    }

    /// Should we store the additional context for callback?
    pub fn stores_callback_context(&self, kind: MethodKind) -> bool {
        matches!(
            kind,
            MethodKind::Pay | MethodKind::Payout | MethodKind::Refund
        ) && self.callback().is_some()
    }

    /// Generate gc settings from the integration based on field usage
    pub fn manifest(&self, kind: MethodKind) -> Option<Manifest> {
        let method = self.methods.get(&kind)?;
        let mut exprs = Vec::new();
        method.collect_exprs(self, &mut exprs);
        let mut payment = first_segments(&exprs, "payment");
        // The callback forward is signed with it, so the platform has to send
        // it even though no expression reads it.
        if self.stores_callback_context(kind) {
            for field in ["token", "merchant_private_key"] {
                if !payment.iter().any(|p| p == field) {
                    payment.push(field.to_string());
                }
            }
            payment.sort();
        }
        Some(Manifest {
            payment,
            params: first_segments(&exprs, "params"),
            refund: first_segments(&exprs, "refund"),
            settings: self.settings.manifest(),
        })
    }
}

fn first_segments(exprs: &[&Expr], root: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for path in exprs.iter().flat_map(|e| e.paths()) {
        if let [r, first, ..] = path.as_slice() {
            if r == root {
                out.insert(first.clone());
            }
        }
    }
    out.into_iter().collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodDef {
    /// All requests that are executed in order
    #[serde(default)]
    pub requests: Vec<RequestDef>,
    pub result: ResultMapping,
}

impl MethodDef {
    /// Every expression reachable from this method, auth included.
    pub fn collect_exprs<'a>(&'a self, integration: &'a Integration, out: &mut Vec<&'a Expr>) {
        let mut seen_auth: BTreeSet<&AuthId> = BTreeSet::new();
        for req in &self.requests {
            collect_request_exprs(req, integration, out, &mut seen_auth);
        }
        self.result.exprs(out);
    }
}

fn collect_request_exprs<'a>(
    req: &'a RequestDef,
    integration: &'a Integration,
    out: &mut Vec<&'a Expr>,
    seen_auth: &mut BTreeSet<&'a AuthId>,
) {
    out.extend(req.exprs());
    if let Some(e) = &req.run_if {
        out.push(e);
    }
    out.extend(req.response.success_when.iter());
    out.extend(req.response.error.message.iter());

    let Some(id) = &req.auth else { return };
    // Guard against a cyclic auth graph; `validate` rejects one outright, but
    // this walker must terminate even on an unvalidated document.
    if !seen_auth.insert(id) {
        return;
    }
    let Some(def) = integration.auth(id) else {
        return;
    };
    match &def.kind {
        AuthKind::None => {}
        AuthKind::Bearer { token } => out.push(token),
        AuthKind::Basic { username, password } => {
            out.push(username);
            out.push(password);
        }
        AuthKind::Header { value, .. } | AuthKind::Query { value, .. } => out.push(value),
        AuthKind::Signature(sig) => {
            out.push(&sig.canonical);
            out.extend(sig.secret.iter());
        }
        AuthKind::TokenRequest(tr) => {
            collect_request_exprs(&tr.request, integration, out, seen_auth);
            out.push(&tr.token);
            out.extend(tr.expires_in.iter());
            out.extend(tr.cache_key.iter());
        }
    }
}

/// The reply sent back to reactivepay, built from the final scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultMapping {
    /// Must evaluate to gc status.
    pub status: Expr,
    #[serde(default)]
    pub gateway_token: Option<Expr>,
    #[serde(default)]
    pub amount: Option<Expr>,
    #[serde(default)]
    pub currency: Option<Expr>,
    #[serde(default)]
    pub details: Option<Expr>,
    #[serde(default)]
    pub redirect_request: Option<RedirectDef>,
    #[serde(default)]
    pub requisites: Option<Expr>,
}

impl ResultMapping {
    pub fn exprs<'a>(&'a self, out: &mut Vec<&'a Expr>) {
        out.push(&self.status);
        out.extend(self.gateway_token.iter());
        out.extend(self.amount.iter());
        out.extend(self.currency.iter());
        out.extend(self.details.iter());
        if let Some(r) = &self.redirect_request {
            out.extend(r.exprs());
        }
        out.extend(self.requisites.iter());
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RedirectDef {
    Post {
        url: Expr,
        #[serde(default = "Expr::empty_object")]
        params: Expr,
    },
    Get {
        url: Expr,
    },
    GetWithProcessing {
        url: Expr,
    },
    PostIframes {
        iframes: Vec<IframeDef>,
    },
    RedirectHtml {
        html: Expr,
    },
}

impl RedirectDef {
    pub fn exprs(&self) -> Vec<&Expr> {
        match self {
            RedirectDef::Post { url, params } => vec![url, params],
            RedirectDef::Get { url } | RedirectDef::GetWithProcessing { url } => vec![url],
            RedirectDef::PostIframes { iframes } => {
                iframes.iter().flat_map(|f| [&f.url, &f.data]).collect()
            }
            RedirectDef::RedirectHtml { html } => vec![html],
        }
    }

    /// The `type` tag, for the editor's kind picker.
    pub fn tag(&self) -> &'static str {
        match self {
            RedirectDef::Post { .. } => "post",
            RedirectDef::Get { .. } => "get",
            RedirectDef::GetWithProcessing { .. } => "get_with_processing",
            RedirectDef::PostIframes { .. } => "post_iframes",
            RedirectDef::RedirectHtml { .. } => "redirect_html",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IframeDef {
    pub url: Expr,
    #[serde(default = "Expr::empty_object")]
    pub data: Expr,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_carries_a_redirect_and_requisites_through_a_round_trip() {
        const RESULT: &str = r#"{
            "status": "\"pending\"",
            "redirect_request": { "type": "get_with_processing",
                                  "url": "steps.c.body.pay_url" },
            "requisites": "{\"account\": steps.c.body.account}"
        }"#;
        let r: ResultMapping = serde_json::from_str(RESULT).unwrap();
        assert_eq!(
            r.redirect_request.as_ref().map(RedirectDef::tag),
            Some("get_with_processing")
        );
        let back: ResultMapping =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r, back);

        // Both reach validation and the manifest through the usual walk.
        let mut exprs = Vec::new();
        r.exprs(&mut exprs);
        let paths: Vec<Vec<String>> = exprs.iter().flat_map(|e| e.paths()).collect();
        assert!(paths.contains(&vec![
            "steps".to_string(),
            "c".to_string(),
            "body".to_string(),
            "pay_url".to_string()
        ]));
        assert!(paths.contains(&vec![
            "steps".to_string(),
            "c".to_string(),
            "body".to_string(),
            "account".to_string()
        ]));
    }

    #[test]
    fn manifest_is_derived_from_the_document() {
        let d: Integration = serde_json::from_str(
            r#"{
                "key": "g", "name": "G", "base_url": "'https://x.example'",
                "settings": { "fields": [{ "name": "client_id" }] },
                "methods": { "status": {
                    "requests": [{ "name": "s", "path": "'/s/' + payment.gateway_token" }],
                    "result": { "status": "\"pending\"" } } }
            }"#,
        )
        .unwrap();
        let m = d.manifest(MethodKind::Status).unwrap();
        assert_eq!(m.payment, ["gateway_token"]);
        assert!(m.params.is_empty());
        assert_eq!(m.settings, ["client_id"]);
        assert!(d.manifest(MethodKind::Refund).is_none());
    }

    #[test]
    fn a_callback_asks_the_platform_for_the_merchant_key_on_pay() {
        let d: Integration = serde_json::from_str(
            r#"{
                "key": "g", "name": "G", "base_url": "'https://x.example'",
                "methods": {
                    "pay": { "requests": [{ "name": "c", "path": "'/c'" }],
                             "result": { "status": "\"pending\"" } },
                    "status": { "requests": [{ "name": "s", "path": "'/s'" }],
                                "result": { "status": "\"pending\"" } } },
                "callback": { "lookup": "callback.body.id",
                              "result": { "status": "\"approved\"", "amount": "callback.body.a",
                                          "currency": "callback.body.c" } }
            }"#,
        )
        .unwrap();
        assert_eq!(
            d.manifest(MethodKind::Pay).unwrap().payment,
            ["merchant_private_key", "token"]
        );
        assert!(d.manifest(MethodKind::Status).unwrap().payment.is_empty());
    }
}
