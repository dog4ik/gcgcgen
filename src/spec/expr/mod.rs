//! The mapping expression language.
//!
//! Expressions appear inside `{{ … }}` placeholders in request templates and
//! standalone in places like [`ResultMapping`](crate::spec::ResultMapping).
//! They are intentionally not a programming language: no statements, no
//! loops, no user-defined functions. Everything terminates.
//!
//! ```text
//! payment.product ?? payment.order_number ?? 'Payment'
//! payment.gateway_amount | minor_to_major
//! [params.first_name, params.last_name] | join(' ') | trim
//! resp.body.status | map({Success: 'approved', _default: 'pending'})
//! ```
//!
//! Two rules do most of the work:
//!
//! * **Absence.** A value is absent when it is `null` or `""`. `??` skips
//!   absent alternatives, `{{? }}` omits absent results, and most builtins
//!   pass absence straight through instead of coercing it to `""`.
//! * **Forgiving paths.** A missing root or segment is `null`, never an error,
//!   so half-typed paths still preview in the editor. Root *names* are checked
//!   per context by [`crate::spec::validate`].
//!
//! Evaluation is pure — no clock, no network, no randomness — so the browser
//! preview and the server executor always agree. Time-varying inputs are
//! injected by the caller under the `env` root.

pub mod eval;
pub mod func;
pub mod lex;
pub mod parse;

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

pub use parse::Node;

/// Byte range within the expression source, for editor underlining.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// A compile-time (lex or parse) failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprError {
    pub message: String,
    pub span: Span,
}

impl ExprError {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (at {}..{})",
            self.message, self.span.start, self.span.end
        )
    }
}

impl std::error::Error for ExprError {}

/// A runtime failure. Carries a span when the fault is attributable to a
/// particular call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalError {
    pub message: String,
    pub span: Option<Span>,
}

impl EvalError {
    pub fn new(message: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }

    pub fn from_parse(e: ExprError) -> Self {
        Self {
            message: e.message,
            span: Some(e.span),
        }
    }
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.span {
            Some(s) => write!(f, "{} (at {}..{})", self.message, s.start, s.end),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for EvalError {}

/// A parsed expression.
///
/// Serialises as its source string, so an integration document stays readable
/// and hand-editable; deserialising re-parses and therefore rejects a
/// malformed expression at load time rather than mid-payment.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    src: String,
    node: Node,
}

impl Expr {
    pub fn parse(src: impl Into<String>) -> Result<Self, ExprError> {
        let src = src.into();
        let node = parse::parse(&src)?;
        Ok(Self { src, node })
    }

    pub fn src(&self) -> &str {
        &self.src
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    pub fn eval(&self, scope: &Value) -> Result<Value, EvalError> {
        eval::eval(&self.node, scope)
    }

    /// Scope roots this expression reads, for context validation.
    pub fn roots(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.node.roots(&mut out);
        out
    }

    /// `(name, arg_count, span)` for every function invoked.
    pub fn calls(&self) -> Vec<(String, usize, Span)> {
        let mut out = Vec::new();
        self.node.calls(&mut out);
        out
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.src)
    }
}

impl Serialize for Expr {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.src)
    }
}

impl<'de> Deserialize<'de> for Expr {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let src = String::deserialize(d)?;
        Expr::parse(src).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_as_a_string() {
        let e = Expr::parse("payment.token ?? 'x'").unwrap();
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(s, r#""payment.token ?? 'x'""#);
        assert_eq!(serde_json::from_str::<Expr>(&s).unwrap(), e);
    }

    #[test]
    fn deserialising_rejects_a_bad_expression() {
        let err = serde_json::from_str::<Expr>(r#""payment.""#).unwrap_err();
        assert!(err.to_string().contains("field name after `.`"), "{err}");
    }

    #[test]
    fn evaluates_through_the_wrapper() {
        let e = Expr::parse("payment.gateway_amount | minor_to_major").unwrap();
        assert_eq!(
            e.eval(&json!({"payment": {"gateway_amount": 2500}}))
                .unwrap(),
            json!(25)
        );
    }

    #[test]
    fn exposes_roots_and_calls_for_validation() {
        let e = Expr::parse("payment.a ?? settings.b | trim").unwrap();
        assert_eq!(e.roots(), vec!["payment", "settings"]);
        assert_eq!(e.calls().len(), 1);
    }
}
