//! The integration document.
//!
//! One [`Integration`] describes how to talk to one external gateway. It is
//! stored as a single JSON document, edited as a whole, and versioned on every
//! save. Nothing in it is credentials: those arrive per-request from
//! reactivepay in the `settings` bucket and are never persisted.
//!
//! ```text
//! Integration
//! ├── settings : SettingsSchema      what reactivepay sends in `settings`
//! ├── auths    : [AuthDef]           shared, referenced by id, token-cached
//! └── methods  : {pay,payout,refund,status}
//!                └── requests : [RequestDef]   run in order
//!                └── result   : ResultMapping  the reply to reactivepay
//! ```

pub mod auth;
pub mod expr;
pub mod request;
pub mod settings;
pub mod template;
pub mod validate;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub use connect::{Manifest, MethodKind, Status};

pub use auth::{AuthDef, AuthId, AuthKind};
pub use expr::{EvalError, Expr, ExprError};
pub use request::{Body, Condition, HttpMethod, NameValue, OnError, RequestDef, ResponseSpec};
pub use settings::{FieldDef, FieldType, SettingsSchema};
pub use template::{JsonTemplate, Template};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Integration {
    /// URL segment and platform `gateway_key`, e.g. `scripay`.
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Prefix for every request path. May branch on `settings.sandbox`.
    pub base_url: Template,
    #[serde(default)]
    pub settings: SettingsSchema,
    #[serde(default)]
    pub auths: Vec<AuthDef>,
    #[serde(default)]
    pub methods: BTreeMap<MethodKind, MethodDef>,
}

impl Integration {
    pub fn auth(&self, id: &AuthId) -> Option<&AuthDef> {
        self.auths.iter().find(|a| a.id == *id)
    }

    pub fn method(&self, kind: MethodKind) -> Option<&MethodDef> {
        self.methods.get(&kind).filter(|m| m.enabled)
    }

    /// The platform's `params_fields` manifest for one method, derived from
    /// the paths the method's expressions actually read. Keeps the
    /// registration payload honest instead of hand-maintained.
    pub fn manifest(&self, kind: MethodKind) -> Option<Manifest> {
        let method = self.methods.get(&kind)?;
        let mut exprs = Vec::new();
        method.collect_exprs(self, &mut exprs);
        Some(Manifest {
            payment: first_segments(&exprs, "payment"),
            params: first_segments(&exprs, "params"),
            settings: self.settings.manifest(),
        })
    }
}

fn first_segments(exprs: &[&Expr], root: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for e in exprs {
        collect_first_segments(e.node(), root, &mut out);
    }
    out.into_iter().collect()
}

fn collect_first_segments(node: &expr::Node, root: &str, out: &mut BTreeSet<String>) {
    use expr::parse::Seg;
    match node {
        expr::Node::Path { root: r, segs, .. } if r == root => {
            if let Some(Seg::Key(k)) = segs.first() {
                out.insert(k.clone());
            }
        }
        expr::Node::Path { .. } | expr::Node::Literal(_) => {}
        expr::Node::Array(items) | expr::Node::Coalesce(items) => items
            .iter()
            .for_each(|n| collect_first_segments(n, root, out)),
        expr::Node::Object(fields) => fields
            .iter()
            .for_each(|(_, n)| collect_first_segments(n, root, out)),
        expr::Node::Pipe { input, calls } => {
            collect_first_segments(input, root, out);
            for c in calls {
                c.args
                    .iter()
                    .for_each(|n| collect_first_segments(n, root, out));
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodDef {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Executed in order. Each sees `steps.<name>` for every earlier request.
    #[serde(default)]
    pub requests: Vec<RequestDef>,
    pub result: ResultMapping,
}

fn yes() -> bool {
    true
}

impl MethodDef {
    pub fn request(&self, name: &str) -> Option<&RequestDef> {
        self.requests.iter().find(|r| r.name == name)
    }

    /// Every expression reachable from this method, including those inside the
    /// auth definitions its requests reference.
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
    out.extend(req.templates().into_iter().flat_map(|t| t.exprs()));
    if let Some(e) = &req.run_if {
        out.push(e);
    }
    req.response.success_when.exprs(out);
    if let Some(t) = &req.response.success {
        out.extend(t.templates().into_iter().flat_map(|t| t.exprs()));
    }
    out.extend(req.response.error.message.iter());
    out.extend(req.response.error.code.iter());

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
        AuthKind::Header { value, .. } | AuthKind::Query { value, .. } => out.extend(value.exprs()),
        AuthKind::Signature(sig) => {
            out.extend(sig.canonical.exprs());
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
    /// Must evaluate to `approved`, `declined` or `pending`.
    pub status: Expr,
    #[serde(default)]
    pub gateway_token: Option<Expr>,
    #[serde(default)]
    pub amount: Option<Expr>,
    #[serde(default)]
    pub currency: Option<Expr>,
    #[serde(default)]
    pub details: Option<Expr>,
}

impl ResultMapping {
    pub fn exprs<'a>(&'a self, out: &mut Vec<&'a Expr>) {
        out.push(&self.status);
        out.extend(self.gateway_token.iter());
        out.extend(self.amount.iter());
        out.extend(self.currency.iter());
        out.extend(self.details.iter());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_derived_from_the_document() {
        let d: Integration = serde_json::from_str(
            r#"{
                "key": "g", "name": "G", "base_url": "https://x.example",
                "settings": { "fields": [{ "name": "client_id" }] },
                "methods": { "status": {
                    "requests": [{ "name": "s", "path": "/s/{{ payment.gateway_token }}" }],
                    "result": { "status": "'pending'" } } }
            }"#,
        )
        .unwrap();
        let m = d.manifest(MethodKind::Status).unwrap();
        assert_eq!(m.payment, ["gateway_token"]);
        assert!(m.params.is_empty());
        assert_eq!(m.settings, ["client_id"]);
        assert!(d.manifest(MethodKind::Refund).is_none());
    }
}
