pub mod func;

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use mahoraga::engine::Engine;
use mahoraga::env::Env;
use mahoraga::error::{Error, RuntimeError};
use mahoraga::facts::Facts;
use mahoraga::fns::{Callable, Function};
use mahoraga::lex::Atom;
use mahoraga::parser::{Node, NodeKind};
use mahoraga::span::{Span, Spanned};
use mahoraga::value::ValueType;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// A parsed expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    src: String,
    root: Node,
}

impl Expr {
    pub fn parse(src: impl Into<String>) -> Result<Self, Error> {
        let src = src.into();
        let root = mahoraga::parser::parse_expr(&src)?;
        Ok(Self { src, root })
    }

    /// A string literal.
    pub fn literal(text: impl Into<String>) -> Self {
        Self::parse(quote(&text.into())).expect("quoted literal parses")
    }

    pub fn empty_object() -> Self {
        Self::parse("{}").expect("empty object parses")
    }

    pub fn src(&self) -> &str {
        &self.src
    }

    pub fn span(&self) -> Span {
        self.root.span
    }

    /// The text of an expression that is nothing but a string literal.
    pub fn as_literal(&self) -> Option<&str> {
        match &self.root.kind {
            NodeKind::Atom(Atom::StrLit(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// `Ok(None)` is void: whatever this value was filling is dropped.
    pub fn eval(&self, scope: &Value) -> Result<Option<Value>, Error> {
        let mut env = Env {
            fns: FNS.with(Clone::clone),
            ..Env::default()
        };
        if let mahoraga::Value::Object(roots) = mahoraga::Value::from(scope.clone()) {
            env.attach_object(roots);
        }
        match Engine::new(env).eval(&self.root)? {
            v @ (mahoraga::Value::Function(_)
            | mahoraga::Value::Closure(_)
            | mahoraga::Value::VmClosure(_)) => Err(self.unexpected(v.value_type())),
            out => Ok(out.into_json()),
        }
    }

    /// `Ok(None)` for an absent, null or blank result. Containers are an error.
    pub fn eval_text(&self, scope: &Value) -> Result<Option<String>, Error> {
        Ok(match self.eval(scope)? {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.is_empty() => None,
            Some(Value::String(s)) => Some(s),
            Some(Value::Number(n)) => Some(n.to_string()),
            Some(Value::Bool(b)) => Some(b.to_string()),
            Some(Value::Array(_)) => return Err(self.unexpected(ValueType::Array)),
            Some(Value::Object(_)) => return Err(self.unexpected(ValueType::Object)),
        })
    }

    fn unexpected(&self, got: ValueType) -> Error {
        let err = RuntimeError::UnexpectedType {
            got,
            expected: ValueType::String,
        };
        Spanned::new(err, self.span()).into()
    }

    /// Paths read and functions invoked, for validation.
    pub fn facts(&self) -> Facts {
        self.root.collect_facts()
    }
}

/// Arguments beside the piped input, or `None` when `name` is not a function.
pub fn arity(name: &str) -> Option<usize> {
    FNS.with(|fns| fns.get(name).map(|f| f.arity().saturating_sub(1)))
}

thread_local! {
    static FNS: HashMap<&'static str, Rc<Function>> = func::fns();
}

/// Source text for a string literal. Single quotes: these sources live inside
/// a JSON document.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        if c == '\'' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('\'');
    out
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
        let e = Expr::parse(r#"payment.token ?? "x""#).unwrap();
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(s, r#""payment.token ?? \"x\"""#);
        assert_eq!(serde_json::from_str::<Expr>(&s).unwrap(), e);
    }

    #[test]
    fn deserialising_rejects_a_bad_expression() {
        assert!(serde_json::from_str::<Expr>(r#""payment.""#).is_err());
    }

    fn scope() -> Value {
        json!({
            "payment": { "token": "tok_1", "gateway_amount": 1000, "product": null,
                         "order_number": "ORD-9" },
            "params":  { "phone": null, "customer": { "phone": "0798288410" },
                         "first_name": "Satoru", "last_name": "Gojo",
                         "extra_return_param": "_blank_" },
            "settings": { "wallet": "w1", "code": "MPESA", "channel": "Mpesa" },
            "steps":   { "auth": { "access_token": "abc" } },
            "env":     { "callback_url": "https://cb.example/gateway/callback" }
        })
    }

    fn ev(src: &str) -> Option<Value> {
        Expr::parse(src).unwrap().eval(&scope()).unwrap()
    }

    #[test]
    fn evaluates_a_builtin_through_the_bridge() {
        let e = Expr::parse("payment.gateway_amount | minor_to_major").unwrap();
        assert_eq!(
            e.eval(&json!({"payment": {"gateway_amount": 2500}}))
                .unwrap(),
            Some(json!(25))
        );
    }

    #[test]
    fn whole_numbers_come_back_as_integers() {
        let v = Expr::parse("payment.a | minor_to_major")
            .unwrap()
            .eval(&json!({"payment": {"a": 10000}}))
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::to_string(&v).unwrap(), "100", "not 100.0");
    }

    #[test]
    fn a_missing_path_is_absent_and_an_explicit_null_is_not() {
        assert_eq!(ev("payment.nope"), None);
        assert_eq!(ev("payment.product"), Some(Value::Null));
        assert_eq!(ev("payment.nope | void_as_null"), Some(Value::Null));
    }

    #[test]
    fn null_and_void_literals() {
        assert_eq!(ev("null"), Some(Value::Null));
        assert_eq!(ev("void"), None);
        assert_eq!(ev("payment.product == null"), Some(json!(true)));
        assert_eq!(ev("payment.nope == void"), Some(json!(true)));
        assert_eq!(ev("payment.token != null"), Some(json!(true)));
        assert_eq!(
            Expr::parse("payment.token != null && x == void")
                .unwrap()
                .facts()
                .roots(),
            ["payment", "x"]
        );
    }

    #[test]
    fn integers_and_floats_stay_apart() {
        assert_eq!(ev("payment.gateway_amount * 2"), Some(json!(2000)));
        assert_eq!(ev("payment.gateway_amount / 8."), Some(json!(125.0)));
        assert_eq!(ev("payment.gateway_amount | to_f"), Some(json!(1000.0)));
    }

    #[test]
    fn a_blank_builtin_result_is_absent() {
        let s = json!({ "params": { "first_name": "", "last_name": null } });
        let e = Expr::parse("[params.first_name, params.last_name] | join(' ') | trim").unwrap();
        assert_eq!(e.eval(&s).unwrap(), None);
        assert_eq!(
            ev("[params.first_name, params.last_name] | join(' ') | trim"),
            Some(json!("Satoru Gojo"))
        );
    }

    #[test]
    fn absent_values_drop_out_of_objects_and_arrays() {
        assert_eq!(
            ev(r#"{"keep": 1, "drop": payment.nope, "null": payment.product}"#),
            Some(json!({"keep": 1, "null": null}))
        );
        assert_eq!(ev("['a', payment.nope, 'b']"), Some(json!(["a", "b"])));
    }

    #[test]
    fn evaluates_an_object_literal_body() {
        let e = Expr::parse(
            r#"{"purpose": 'payment',
                "order_id": payment.token,
                "amount": payment.gateway_amount | minor_to_major,
                "channel": (params.extra_return_param | void_if('_blank_')) ?? settings.channel,
                "data": { "phone_number": params.phone ?? params.customer.phone }}"#,
        )
        .unwrap();
        assert_eq!(
            e.eval(&scope()).unwrap(),
            Some(json!({
                "purpose": "payment",
                "order_id": "tok_1",
                "amount": 10,
                "channel": "Mpesa",
                "data": { "phone_number": "0798288410" }
            }))
        );
    }

    #[test]
    fn text_is_blank_sensitive_and_rejects_containers() {
        let e = |src: &str| Expr::parse(src).unwrap();
        assert_eq!(
            e("'Bearer ' + steps.auth.access_token")
                .eval_text(&scope())
                .unwrap()
                .as_deref(),
            Some("Bearer abc")
        );
        assert_eq!(
            e("payment.gateway_amount")
                .eval_text(&scope())
                .unwrap()
                .as_deref(),
            Some("1000")
        );
        assert_eq!(e("payment.product").eval_text(&scope()).unwrap(), None);
        assert_eq!(e("payment.nope").eval_text(&scope()).unwrap(), None);
        assert_eq!(e("''").eval_text(&scope()).unwrap(), None);
        assert!(e("params.customer").eval_text(&scope()).is_err());
    }

    #[test]
    fn a_literal_round_trips_through_its_source() {
        let e = Expr::literal("/v1/gateway/initiate/collection");
        assert_eq!(e.src(), "'/v1/gateway/initiate/collection'");
        assert_eq!(e.as_literal(), Some("/v1/gateway/initiate/collection"));
        assert_eq!(Expr::parse(e.src()).unwrap(), e);
        assert_eq!(Expr::literal("it's").src(), r"'it\'s'");
        assert_eq!(Expr::literal("it's").as_literal(), Some("it's"));
        assert!(Expr::parse("payment.token").unwrap().as_literal().is_none());
        assert_eq!(
            Expr::empty_object().eval(&scope()).unwrap(),
            Some(json!({}))
        );
    }

    #[test]
    fn validation_sees_every_registered_function() {
        for (name, f) in &func::fns() {
            assert_eq!(
                arity(name),
                Some(f.arity().saturating_sub(1)),
                "`{name}` is registered but validation disagrees about its arity"
            );
        }
    }

    #[test]
    fn every_function_takes_a_piped_input_and_carries_help() {
        for (name, f) in func::fns() {
            assert!(f.arity() >= 1, "`{name}` cannot be piped into");
            assert!(f.help.is_some(), "`{name}` has no help for the editor");
        }
    }
}
