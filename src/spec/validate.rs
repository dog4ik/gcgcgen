//! Whole-document validation, run before an integration is saved.
//!
//! The point is to move every knowable failure from "mid-payment, in
//! production" to "the editor refuses to save". It checks that expressions
//! reference roots that exist *in their context*, that builtin names and
//! arities are real, that a request only reads steps that ran before it, that
//! auth references resolve and do not form a cycle, and that the status
//! mapping can only produce the three canonical values.

use std::collections::{BTreeMap, BTreeSet};

use super::auth::{AuthId, AuthKind, TokenRequestAuth};
use super::expr::{self, Expr, Span};
use super::request::RequestDef;
use super::{CallbackDef, Integration, MethodDef, MethodKind, ResultMapping, Status};

/// Roots available to a request's own expressions.
const REQUEST_ROOTS: &[&str] = &[
    "payment", "params", "refund", "settings", "steps", "env", "method",
];
/// Roots available while interpreting a response.
const RESPONSE_ROOTS: &[&str] = &[
    "payment", "params", "refund", "settings", "steps", "env", "method", "resp",
];
/// Roots available to a signature's canonical expression.
const SIGNATURE_ROOTS: &[&str] = &[
    "payment", "params", "settings", "steps", "env", "method", "req",
];
/// Roots available inside a callback: the stored settings plus the inbound
/// callback. The payment, params and refund buckets are not kept.
const CALLBACK_ROOTS: &[&str] = &["settings", "steps", "env", "method", "callback"];
/// A callback's `lookup` runs before the stored context is found.
const LOOKUP_ROOTS: &[&str] = &["callback", "env"];
/// A cache key must be derivable before anything runs.
const CACHE_KEY_ROOTS: &[&str] = &["payment", "params", "settings", "env", "method"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// Where in the document, e.g. `methods.pay.requests[0].body`.
    pub location: String,
    pub message: String,
    pub span: Option<Span>,
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub issues: Vec<Issue>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, issue) in self.issues.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Report {}

pub fn validate(integration: &Integration) -> Result<(), Report> {
    let mut v = Validator {
        integration,
        issues: Vec::new(),
    };
    v.run();
    if v.issues.is_empty() {
        Ok(())
    } else {
        // Fields are walked in hash order: sort, or the report shuffles.
        let mut issues = v.issues;
        issues.sort_by(|a, b| (&a.location, &a.message).cmp(&(&b.location, &b.message)));
        Err(Report { issues })
    }
}

struct Validator<'a> {
    integration: &'a Integration,
    issues: Vec<Issue>,
}

impl<'a> Validator<'a> {
    fn issue(&mut self, location: impl Into<String>, message: impl Into<String>) {
        self.issues.push(Issue {
            location: location.into(),
            message: message.into(),
            span: None,
        });
    }

    fn issue_at(&mut self, location: impl Into<String>, message: impl Into<String>, span: Span) {
        self.issues.push(Issue {
            location: location.into(),
            message: message.into(),
            span: Some(span),
        });
    }

    fn run(&mut self) {
        self.check_key();
        self.check_auths();
        self.check_base_url();
        self.check_methods();
        self.check_callback();
    }

    fn check_key(&mut self) {
        let key = &self.integration.key;
        if key.is_empty() {
            self.issue("key", "must not be empty");
        } else if !key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            self.issue(
                "key",
                "must contain only lowercase letters, digits, `-` and `_` — it is a URL segment",
            );
        }
        if self.integration.name.trim().is_empty() {
            self.issue("name", "must not be empty");
        }
    }

    fn check_base_url(&mut self) {
        let base_url = self.integration.base_url.clone();
        self.check_expr(&base_url, REQUEST_ROOTS, "base_url");
        // Only a literal can be checked here. The engine re-checks the
        // evaluated value on every call, which is the check that protects
        // traffic.
        if let Some(url) = base_url.as_literal() {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                self.issue("base_url", "must start with `http://` or `https://`");
            }
            if url.ends_with('/') {
                self.issue(
                    "base_url",
                    "must not end with `/` — request paths supply it",
                );
            }
        }
    }

    fn check_auths(&mut self) {
        let mut seen: BTreeSet<&AuthId> = BTreeSet::new();
        for def in &self.integration.auths {
            let loc = format!("auths.{}", def.id);
            if !seen.insert(&def.id) {
                self.issue(&loc, format!("duplicate auth id `{}`", def.id));
            }
            if def.id.as_str().is_empty() {
                self.issue(&loc, "auth id must not be empty");
            }
            self.check_auth_kind(&def.kind, &loc);
        }
        self.check_auth_cycles();
    }

    fn check_auth_kind(&mut self, kind: &AuthKind, loc: &str) {
        match kind {
            AuthKind::None => {}
            AuthKind::Bearer { token } => self.check_expr(token, REQUEST_ROOTS, loc),
            AuthKind::Basic { username, password } => {
                self.check_expr(username, REQUEST_ROOTS, loc);
                self.check_expr(password, REQUEST_ROOTS, loc);
            }
            AuthKind::Header { name, value } | AuthKind::Query { name, value } => {
                if name.trim().is_empty() {
                    self.issue(loc, "name must not be empty");
                }
                self.check_expr(value, REQUEST_ROOTS, loc);
            }
            AuthKind::Signature(sig) => {
                self.check_expr(&sig.canonical, SIGNATURE_ROOTS, loc);
                match (&sig.secret, sig.algorithm.needs_secret()) {
                    (Some(s), _) => self.check_expr(s, REQUEST_ROOTS, loc),
                    (None, true) => {
                        self.issue(loc, format!("{:?} requires a `secret`", sig.algorithm))
                    }
                    (None, false) => {}
                }
            }
            AuthKind::TokenRequest(tr) => self.check_token_request(tr, loc),
        }
    }

    fn check_token_request(&mut self, tr: &TokenRequestAuth, loc: &str) {
        if tr.cache_key.is_empty() {
            self.issue(
                loc,
                "cache_key must not be empty, without a settings-derived key, every merchant on this integration would share one token",
            );
        }
        for e in &tr.cache_key {
            self.check_expr(e, CACHE_KEY_ROOTS, &format!("{loc}.cache_key"));
        }
        if tr.refresh_buffer_secs >= tr.default_ttl_secs {
            self.issue(
                loc,
                format!(
                    "refresh_buffer_secs ({}) must be less than default_ttl_secs ({})",
                    tr.refresh_buffer_secs, tr.default_ttl_secs
                ),
            );
        }
        self.check_expr(&tr.token, RESPONSE_ROOTS, &format!("{loc}.token"));
        if let Some(e) = &tr.expires_in {
            self.check_expr(e, RESPONSE_ROOTS, &format!("{loc}.expires_in"));
        }
        // An auth request runs before any step, so `steps` is unavailable.
        let roots: Vec<&str> = REQUEST_ROOTS
            .iter()
            .copied()
            .filter(|r| *r != "steps")
            .collect();
        self.check_request(
            &tr.request,
            &roots,
            &format!("{loc}.request"),
            &BTreeSet::new(),
        );
    }

    /// An auth request may itself reference an auth, so the graph can loop.
    fn check_auth_cycles(&mut self) {
        let mut edges: BTreeMap<&AuthId, Vec<&AuthId>> = BTreeMap::new();
        for def in &self.integration.auths {
            let mut out = Vec::new();
            if let AuthKind::TokenRequest(tr) = &def.kind {
                if let Some(next) = &tr.request.auth {
                    out.push(next);
                }
            }
            edges.insert(&def.id, out);
        }

        let mut done: BTreeSet<&AuthId> = BTreeSet::new();
        for start in edges.keys().copied() {
            if done.contains(start) {
                continue;
            }
            let mut path: Vec<&AuthId> = Vec::new();
            let mut cur = start;
            loop {
                if path.contains(&cur) {
                    let names: Vec<&str> = path.iter().chain([&cur]).map(|a| a.as_str()).collect();
                    self.issue(
                        format!("auths.{start}"),
                        format!("auth reference cycle: {}", names.join(" -> ")),
                    );
                    break;
                }
                path.push(cur);
                match edges.get(cur).and_then(|v| v.first()) {
                    Some(next) => cur = next,
                    None => break,
                }
            }
            done.extend(path);
        }
    }

    fn check_methods(&mut self) {
        if self.integration.methods.is_empty() {
            self.issue("methods", "define at least one method");
        }
        let kinds: Vec<MethodKind> = self.integration.methods.keys().copied().collect();
        for kind in kinds {
            let method = &self.integration.methods[&kind];
            self.check_method(kind, method);
        }
    }

    fn check_method(&mut self, kind: MethodKind, method: &MethodDef) {
        let base = format!("methods.{kind}");
        if method.requests.is_empty() {
            self.issue(&base, "define at least one request");
        }
        let steps = self.check_requests(&base, &method.requests, REQUEST_ROOTS);
        self.check_result(
            &method.result,
            REQUEST_ROOTS,
            &format!("{base}.result"),
            &steps,
        );
    }

    /// Checks a request sequence and returns the step names it produces.
    fn check_requests(
        &mut self,
        base: &str,
        requests: &[RequestDef],
        roots: &[&str],
    ) -> BTreeSet<String> {
        // A request may only read steps that already ran.
        let mut available: BTreeSet<String> = BTreeSet::new();
        for (i, req) in requests.iter().enumerate() {
            let loc = format!("{base}.requests[{i}]");
            if req.name.trim().is_empty() {
                self.issue(&loc, "name must not be empty — it addresses `steps.<name>`");
            } else if !req
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                self.issue(
                    &loc,
                    format!(
                        "name `{}` must be alphanumeric or `_` so `steps.{}` parses",
                        req.name, req.name
                    ),
                );
            }
            if !available.insert(req.name.clone()) {
                self.issue(&loc, format!("duplicate request name `{}`", req.name));
            }
            let mut earlier = available.clone();
            earlier.remove(&req.name);
            self.check_request(req, roots, &loc, &earlier);
        }
        available
    }

    fn check_callback(&mut self) {
        let Some(cb) = &self.integration.callback else {
            return;
        };
        let base = "callback";
        if !self.integration.methods.contains_key(&MethodKind::Pay)
            && !self.integration.methods.contains_key(&MethodKind::Payout)
        {
            self.issue(
                base,
                "a callback needs a `pay` or `payout` method — that is where its context is kept",
            );
        }
        self.check_expr(&cb.lookup, LOOKUP_ROOTS, &format!("{base}.lookup"));
        if let Some(e) = &cb.verify {
            self.check_expr(e, CALLBACK_ROOTS, &format!("{base}.verify"));
            self.check_steps([e], &BTreeSet::new(), &format!("{base}.verify"));
        }
        let steps = self.check_requests(base, &cb.requests, CALLBACK_ROOTS);
        let result_loc = format!("{base}.result");
        self.check_result(&cb.result, CALLBACK_ROOTS, &result_loc, &steps);
        self.check_callback_result(cb, &result_loc);
        if let Some(body) = &cb.ack.body {
            self.check_expr(body, CALLBACK_ROOTS, &format!("{base}.ack"));
            self.check_steps([body], &steps, &format!("{base}.ack"));
        }
        if !(100..=599).contains(&cb.ack.status) {
            self.issue(format!("{base}.ack.status"), "must be an HTTP status code");
        }
    }

    fn check_callback_result(&mut self, cb: &CallbackDef, loc: &str) {
        let r = &cb.result;
        // Nothing to fall back to: the payment bucket is not kept.
        if r.amount.is_none() {
            self.issue(
                format!("{loc}.amount"),
                "required — the platform needs the settled amount",
            );
        }
        if r.currency.is_none() {
            self.issue(
                format!("{loc}.currency"),
                "required — the platform needs the settled currency",
            );
        }
        for (field, set) in [
            ("gateway_token", r.gateway_token.is_some()),
            ("redirect_request", r.redirect_request.is_some()),
            ("requisites", r.requisites.is_some()),
        ] {
            if set {
                self.issue(
                    format!("{loc}.{field}"),
                    "not part of a callback — remove it",
                );
            }
        }
    }

    fn check_request(
        &mut self,
        req: &RequestDef,
        roots: &[&str],
        loc: &str,
        earlier_steps: &BTreeSet<String>,
    ) {
        if let Some(id) = &req.auth {
            if self.integration.auth(id).is_none() {
                self.issue(loc, format!("unknown auth `{id}`"));
            }
        }
        if req
            .path
            .as_literal()
            .is_some_and(|p| p.starts_with("http://") || p.starts_with("https://"))
        {
            self.issue(loc, "path is appended to base_url; it must not be absolute");
        }
        for e in req.exprs() {
            self.check_expr(e, roots, loc);
            self.check_steps([e], earlier_steps, loc);
        }
        if let Some(e) = &req.run_if {
            self.check_expr(e, roots, &format!("{loc}.run_if"));
            self.check_steps([e], earlier_steps, loc);
        }

        // The response spec additionally sees `resp`.
        let resp_roots: Vec<&str> = roots.iter().copied().chain(["resp"]).collect();
        let resp_loc = format!("{loc}.response");
        if let Some(e) = &req.response.success_when {
            self.check_expr(e, &resp_roots, &resp_loc);
        }
        if let Some(e) = &req.response.success {
            self.check_expr(e, &resp_roots, &resp_loc);
        }
        for e in req
            .response
            .error
            .message
            .iter()
            .chain(req.response.error.code.iter())
        {
            self.check_expr(e, &resp_roots, &resp_loc);
        }
    }

    fn check_result(
        &mut self,
        result: &ResultMapping,
        roots: &[&str],
        loc: &str,
        steps: &BTreeSet<String>,
    ) {
        let mut exprs = Vec::new();
        result.exprs(&mut exprs);
        for e in &exprs {
            self.check_expr(e, roots, loc);
        }
        self.check_steps(exprs.iter().copied(), steps, loc);

        // A computed status is checked at runtime instead.
        for lit in result.status.string_results() {
            if Status::parse(&lit).is_none() {
                self.issue_at(
                    format!("{loc}.status"),
                    format!(
                        "`{lit}` is not a status — expected one of {}",
                        Status::ALL.map(|s| s.as_str()).join(", ")
                    ),
                    Span::new(0, result.status.src().len()),
                );
            }
        }
    }

    fn check_expr(&mut self, e: &Expr, roots: &[&str], loc: &str) {
        // mahoraga carries no spans on parsed nodes.
        let whole = Span::new(0, e.src().len());
        for root in e.roots() {
            if !roots.contains(&root.as_str()) {
                self.issue_at(
                    loc,
                    format!(
                        "unknown root `{root}` here — available: {}",
                        roots.join(", ")
                    ),
                    whole,
                );
            }
        }
        for call in e.calls() {
            let Some(arity) = expr::arity(&call.name) else {
                self.issue_at(loc, format!("unknown function `{}`", call.name), whole);
                continue;
            };
            if call.args != arity {
                let plural = if arity == 1 { "argument" } else { "arguments" };
                self.issue_at(
                    loc,
                    format!("`{}` takes {arity} {plural}, got {}", call.name, call.args),
                    whole,
                );
            }
        }
    }

    /// Rejects `steps.foo` when `foo` has not run yet.
    fn check_steps<'e>(
        &mut self,
        exprs: impl IntoIterator<Item = &'e Expr>,
        available: &BTreeSet<String>,
        loc: &str,
    ) {
        for e in exprs {
            let span = Span::new(0, e.src().len());
            for name in step_refs(e) {
                if !available.contains(&name) {
                    let hint = if available.is_empty() {
                        "no earlier request in this method".to_string()
                    } else {
                        format!(
                            "available here: {}",
                            available.iter().cloned().collect::<Vec<_>>().join(", ")
                        )
                    };
                    self.issue_at(loc, format!("`steps.{name}` — {hint}"), span);
                }
            }
        }
    }
}

/// Every `steps.<name>` referenced by an expression.
fn step_refs(e: &Expr) -> Vec<String> {
    e.paths()
        .into_iter()
        .filter_map(|path| match path.as_slice() {
            [root, name, ..] if root == "steps" => Some(name.clone()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Integration;

    fn doc(methods: &str, auths: &str) -> Integration {
        let src = format!(
            r#"{{
                "key": "scripay",
                "name": "Scripay",
                "base_url": "'https://api.example.com'",
                "settings": {{ "fields": [{{ "name": "client_id" }}] }},
                "auths": {auths},
                "methods": {methods}
            }}"#
        );
        serde_json::from_str(&src).unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    fn simple_method(extra: &str) -> String {
        format!(
            r#"{{ "pay": {{
                 "requests": [
                   {{ "name": "collection", "path": "'/c'",
                      "body": {{ "kind": "json", "expr": "{{\"a\": payment.token}}" }} }}
                   {extra}
                 ],
                 "result": {{ "status": "\"pending\"", "gateway_token": "steps.collection.body.rrn" }}
               }} }}"#
        )
    }

    #[test]
    fn a_well_formed_document_passes() {
        let d = doc(&simple_method(""), "[]");
        validate(&d).unwrap();
    }

    #[test]
    fn rejects_unknown_root() {
        let m = r#"{ "pay": { "requests": [
                    { "name": "c", "path": "'/c'",
                      "body": { "kind": "json", "expr": "{\"a\": nope.x}" } }],
                    "result": { "status": "\"pending\"" } } }"#;
        let e = validate(&doc(m, "[]")).unwrap_err();
        assert!(e.to_string().contains("unknown root `nope`"), "{e}");
    }

    #[test]
    fn rejects_resp_outside_a_response_spec() {
        let m = r#"{ "pay": { "requests": [
                    { "name": "c", "path": "'/' + resp.body.x" }],
                    "result": { "status": "\"pending\"" } } }"#;
        let e = validate(&doc(m, "[]")).unwrap_err();
        assert!(e.to_string().contains("unknown root `resp`"), "{e}");
    }

    #[test]
    fn allows_resp_inside_a_response_spec() {
        let m = r#"{ "pay": { "requests": [
                    { "name": "c", "path": "'/c'",
                      "response": { "error": { "message": "resp.body.message" } } }],
                    "result": { "status": "\"pending\"" } } }"#;
        validate(&doc(m, "[]")).unwrap();
    }

    #[test]
    fn rejects_a_forward_step_reference() {
        let m = r#"{ "pay": { "requests": [
                    { "name": "one", "path": "'/' + steps.two.body.x" },
                    { "name": "two", "path": "'/t'" }],
                    "result": { "status": "\"pending\"" } } }"#;
        let e = validate(&doc(m, "[]")).unwrap_err();
        assert!(e.to_string().contains("`steps.two`"), "{e}");
    }

    #[test]
    fn allows_a_backward_step_reference() {
        let m = r#"{ "pay": { "requests": [
                    { "name": "one", "path": "'/o'" },
                    { "name": "two", "path": "'/' + steps.one.body.x" }],
                    "result": { "status": "\"pending\"" } } }"#;
        validate(&doc(m, "[]")).unwrap();
    }

    #[test]
    fn rejects_unknown_function_and_bad_arity() {
        let m = r#"{ "pay": { "requests": [
                    { "name": "c", "path": "'/c'",
                      "body": { "kind": "json",
                        "expr": "{\"a\": payment.token | trm, \"b\": payment.token | null_if}" } }],
                    "result": { "status": "\"pending\"" } } }"#;
        let e = validate(&doc(m, "[]")).unwrap_err();
        let s = e.to_string();
        assert!(s.contains("unknown function `trm`"), "{s}");
        assert!(s.contains("`null_if` takes 1 argument"), "{s}");
    }

    #[test]
    fn rejects_a_bad_literal_status() {
        let m = r#"{ "pay": { "requests": [{ "name": "c", "path": "'/c'" }],
                    "result": { "status": "\"succeeded\"" } } }"#;
        let e = validate(&doc(m, "[]")).unwrap_err();
        assert!(e.to_string().contains("`succeeded` is not a status"), "{e}");
    }

    #[test]
    fn checks_statuses_produced_by_a_map_table() {
        let m = r#"{ "pay": { "requests": [{ "name": "c", "path": "'/c'" }],
                    "result": { "status": "steps.c.body.status | map({\"Success\":\"approved\", \"Bad\":\"oops\"})" } } }"#;
        let e = validate(&doc(m, "[]")).unwrap_err();
        assert!(e.to_string().contains("`oops` is not a status"), "{e}");
    }

    #[test]
    fn checks_the_redirect_and_the_requisites_like_any_other_expression() {
        let m = r#"{ "pay": { "requests": [{ "name": "c", "path": "'/c'" }],
                    "result": { "status": "\"pending\"",
                                "redirect_request": { "type": "post",
                                                      "url": "steps.nope.body.url",
                                                      "params": "{}" },
                                "requisites": "{\"iban\": resp.body.iban}" } } }"#;
        let e = validate(&doc(m, "[]")).unwrap_err().to_string();
        assert!(e.contains("`steps.nope`"), "{e}");
        assert!(e.contains("unknown root `resp`"), "{e}");
    }

    #[test]
    fn rejects_unknown_auth_reference() {
        let m = r#"{ "pay": { "requests": [{ "name": "c", "path": "'/c'", "auth": "nope" }],
                    "result": { "status": "\"pending\"" } } }"#;
        let e = validate(&doc(m, "[]")).unwrap_err();
        assert!(e.to_string().contains("unknown auth `nope`"), "{e}");
    }

    #[test]
    fn rejects_an_empty_cache_key() {
        let a = r#"[{ "id": "oauth", "kind": "token_request",
                      "request": { "name": "auth", "path": "'/t'" },
                      "token": "resp.body.access_token",
                      "cache_key": [],
                      "placement": { "kind": "bearer" } }]"#;
        let e = validate(&doc(&simple_method(""), a)).unwrap_err();
        assert!(e.to_string().contains("cache_key must not be empty"), "{e}");
    }

    #[test]
    fn rejects_a_cache_key_that_depends_on_a_step() {
        let a = r#"[{ "id": "oauth", "kind": "token_request",
                      "request": { "name": "auth", "path": "'/t'" },
                      "token": "resp.body.access_token",
                      "cache_key": ["steps.c.body.x"],
                      "placement": { "kind": "bearer" } }]"#;
        let e = validate(&doc(&simple_method(""), a)).unwrap_err();
        assert!(e.to_string().contains("unknown root `steps`"), "{e}");
    }

    #[test]
    fn rejects_an_auth_cycle() {
        let a = r#"[
            { "id": "a", "kind": "token_request",
              "request": { "name": "ra", "path": "'/a'", "auth": "b" },
              "token": "resp.body.t", "cache_key": ["settings.client_id"],
              "placement": { "kind": "bearer" } },
            { "id": "b", "kind": "token_request",
              "request": { "name": "rb", "path": "'/b'", "auth": "a" },
              "token": "resp.body.t", "cache_key": ["settings.client_id"],
              "placement": { "kind": "bearer" } }]"#;
        let e = validate(&doc(&simple_method(""), a)).unwrap_err();
        assert!(e.to_string().contains("cycle"), "{e}");
    }

    #[test]
    fn rejects_hmac_without_a_secret() {
        let a = r#"[{ "id": "sig", "kind": "signature",
                      "canonical": "req.path",
                      "algorithm": "hmac_sha256",
                      "placement": { "kind": "header", "name": "X-Sig" } }]"#;
        let e = validate(&doc(&simple_method(""), a)).unwrap_err();
        assert!(e.to_string().contains("requires a `secret`"), "{e}");
    }

    #[test]
    fn allows_a_computed_base_url() {
        let mut d = doc(&simple_method(""), "[]");
        d.base_url =
            Expr::parse(r#"settings.sandbox ? "https://sandbox.example" : "https://api.example""#)
                .unwrap();
        validate(&d).unwrap();
    }

    #[test]
    fn rejects_an_absolute_request_path_and_a_bad_base_url() {
        let mut d = doc(&simple_method(""), "[]");
        d.base_url = Expr::literal("api.example.com/");
        d.methods.get_mut(&MethodKind::Pay).unwrap().requests[0].path =
            Expr::literal("https://evil.example/x");
        let e = validate(&d).unwrap_err();
        let s = e.to_string();
        assert!(s.contains("`http://`"), "{s}");
        assert!(s.contains("must not be absolute"), "{s}");
    }

    #[test]
    fn rejects_duplicate_request_names() {
        let m = r#"{ "pay": { "requests": [
                    { "name": "c", "path": "'/a'" }, { "name": "c", "path": "'/b'" }],
                    "result": { "status": "\"pending\"" } } }"#;
        let e = validate(&doc(m, "[]")).unwrap_err();
        assert!(e.to_string().contains("duplicate request name"), "{e}");
    }

    #[test]
    fn derives_the_platform_manifest_from_expressions() {
        let m = r#"{ "pay": { "requests": [
                    { "name": "c", "path": "'/c'", "body": { "kind": "json",
                        "expr": "{\"o\": payment.token, \"a\": payment.gateway_amount | minor_to_major, \"p\": params.phone ?? params.customer.phone}" } }],
                    "result": { "status": "\"pending\"" } } }"#;
        let d = doc(m, "[]");
        let man = d.manifest(MethodKind::Pay).unwrap();
        assert_eq!(man.payment, ["gateway_amount", "token"]);
        assert_eq!(man.params, ["customer", "phone"]);
        assert_eq!(man.settings, ["client_id"]);
    }

    fn with_callback(callback: &str) -> Integration {
        let mut d = doc(&simple_method(""), "[]");
        d.callback = Some(serde_json::from_str(callback).unwrap());
        d
    }

    #[test]
    fn a_callback_reads_settings_and_the_callback_but_not_payment() {
        let ok = with_callback(
            r#"{ "lookup": "callback.body.rrn",
                 "verify": "callback.query.sig == settings.client_id",
                 "result": { "status": "\"approved\"", "amount": "callback.body.amount",
                             "currency": "callback.body.currency" } }"#,
        );
        validate(&ok).unwrap();

        let bad = with_callback(
            r#"{ "lookup": "settings.client_id",
                 "result": { "status": "\"approved\"", "amount": "payment.gateway_amount",
                             "currency": "callback.body.currency" } }"#,
        );
        let e = validate(&bad).unwrap_err().to_string();
        assert!(
            e.contains("callback.lookup: unknown root `settings`"),
            "{e}"
        );
        assert!(e.contains("callback.result: unknown root `payment`"), "{e}");
    }

    #[test]
    fn a_callback_must_report_amount_and_currency_and_nothing_else() {
        let d = with_callback(
            r#"{ "lookup": "callback.body.rrn",
                 "result": { "status": "\"approved\"", "gateway_token": "callback.body.rrn" } }"#,
        );
        let e = validate(&d).unwrap_err().to_string();
        assert!(e.contains("callback.result.amount: required"), "{e}");
        assert!(e.contains("callback.result.currency: required"), "{e}");
        assert!(
            e.contains("callback.result.gateway_token: not part of a callback"),
            "{e}"
        );
    }

    #[test]
    fn a_callback_request_may_read_only_earlier_steps() {
        let d = with_callback(
            r#"{ "lookup": "callback.body.rrn",
                 "requests": [{ "name": "check", "path": "'/s/' + steps.later.body.x" }],
                 "result": { "status": "\"approved\"", "amount": "steps.check.body.amount",
                             "currency": "steps.check.body.currency" } }"#,
        );
        let e = validate(&d).unwrap_err().to_string();
        assert!(e.contains("callback.requests[0]: `steps.later`"), "{e}");
        assert!(!e.contains("steps.check"), "{e}");
    }
}
