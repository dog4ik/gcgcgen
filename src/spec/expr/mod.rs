//! The mapping expression language, parsed and evaluated by [`mahoraga`].

pub mod func;

use std::fmt;
use std::rc::Rc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

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

impl From<mahoraga::Span> for Span {
    fn from(s: mahoraga::Span) -> Self {
        Self::new(s.start, s.end)
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

/// A runtime failure.
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
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    src: String,
    node: mahoraga::Node,
}

impl Expr {
    pub fn parse(src: impl Into<String>) -> Result<Self, ExprError> {
        let src = src.into();
        match mahoraga::parse_expr(&src) {
            Ok(node) => Ok(Self { src, node }),
            Err(e) => {
                let span = e.span.map_or(Span::new(0, src.len()), Span::from);
                Err(ExprError::new(e.message, span))
            }
        }
    }

    /// A string literal.
    pub fn literal(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            src: quote(&text),
            node: mahoraga::Node::Atom(mahoraga::Atom::StrLit(text)),
        }
    }

    /// The empty object.
    pub fn empty_object() -> Self {
        Self {
            src: "{}".to_string(),
            node: mahoraga::Node::ObjectLit(Default::default()),
        }
    }

    pub fn src(&self) -> &str {
        &self.src
    }

    /// The text of an expression that is nothing but a string literal.
    pub fn as_literal(&self) -> Option<&str> {
        match &self.node {
            mahoraga::Node::Atom(mahoraga::Atom::StrLit(s)) => Some(s),
            _ => None,
        }
    }

    /// `Ok(None)` is void: whatever this value was filling is dropped.
    pub fn eval(&self, scope: &Value) -> Result<Option<Value>, EvalError> {
        let mut env = env();
        if let mahoraga::Value::Object(roots) = mahoraga::Value::from(scope.clone()) {
            env.attach_object(roots);
        }
        let out = mahoraga::eval(self.node.clone(), &env)
            .map_err(|e| EvalError::new(e.message, e.span.map(Span::from)))?;
        to_json(out)
    }

    /// `Ok(None)` for an absent, null or blank result. Containers are an error.
    pub fn eval_text(&self, scope: &Value) -> Result<Option<String>, EvalError> {
        Ok(match self.eval(scope)? {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.is_empty() => None,
            Some(Value::String(s)) => Some(s),
            Some(Value::Number(n)) => Some(n.to_string()),
            Some(Value::Bool(b)) => Some(b.to_string()),
            Some(other) => {
                return Err(EvalError::new(
                    format!(
                        "expected text, got {}",
                        if other.is_array() {
                            "an array"
                        } else {
                            "an object"
                        }
                    ),
                    None,
                ))
            }
        })
    }

    /// Scope roots this expression reads, for context validation.
    pub fn roots(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for root in self
            .paths()
            .into_iter()
            .filter_map(|p| p.into_iter().next())
        {
            if !out.contains(&root) {
                out.push(root);
            }
        }
        out
    }

    /// Every path read, as its static segments: `steps["auth"].token` is
    /// `["steps", "auth", "token"]`.
    pub fn paths(&self) -> Vec<Vec<String>> {
        self.facts().paths
    }

    /// Every function invoked, whether written as `x | f(a)` or `f(x, a)`.
    pub fn calls(&self) -> Vec<Call> {
        self.facts().calls
    }

    /// String literals the expression can evaluate to directly.
    pub fn string_results(&self) -> Vec<String> {
        self.facts().string_results
    }

    fn facts(&self) -> Facts {
        let mut facts = Facts::default();
        collect_facts(&self.node, true, &mut facts);
        facts
    }
}

/// One invocation of a function, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub name: String,
    /// Arguments beside the input: `x | join(" ")` and `join(x, " ")` are both one.
    pub args: usize,
}

#[derive(Default)]
struct Facts {
    paths: Vec<Vec<String>>,
    calls: Vec<Call>,
    string_results: Vec<String>,
}

/// `is_result` is whether this node's value can become the whole expression's
/// value.
fn collect_facts(node: &mahoraga::Node, is_result: bool, out: &mut Facts) {
    use mahoraga::{Atom, Ident, Node, Punct};
    if let Some(path) = static_path(node) {
        out.paths.push(path);
        return;
    }
    match node {
        Node::Op((Punct::Pipe, (input, call))) => {
            collect_facts(input, false, out);
            match &**call {
                Node::Atom(Atom::Ident(Ident(name))) => collect_call(name, &[], is_result, out),
                Node::Call { callee, args } => collect_callee(callee, args, is_result, out),
                other => collect_facts(other, false, out),
            }
        }
        Node::Op((Punct::Coalesce | Punct::Or | Punct::And, (lhs, rhs))) => {
            collect_facts(lhs, is_result, out);
            collect_facts(rhs, is_result, out);
        }
        Node::Op((_, (lhs, rhs))) => {
            collect_facts(lhs, false, out);
            collect_facts(rhs, false, out);
        }
        Node::Turnary {
            operand,
            truth_node,
            false_node,
        } => {
            collect_facts(operand, false, out);
            collect_facts(truth_node, is_result, out);
            collect_facts(false_node, is_result, out);
        }
        Node::Member { object, .. } => collect_facts(object, false, out),
        Node::Index { object, index } => {
            collect_facts(object, false, out);
            collect_facts(index, false, out);
        }
        // A string inside a literal is a part of the result, not the result.
        Node::ArrayLit(items) => {
            for item in items {
                collect_facts(item, false, out);
            }
        }
        Node::ObjectLit(fields) => {
            for value in fields.values() {
                collect_facts(value, false, out);
            }
        }
        Node::Atom(Atom::StrLit(s)) if is_result => out.string_results.push(s.clone()),
        Node::Atom(_) => {}
        Node::Call { callee, args } => match args.split_first() {
            Some((input, rest)) => {
                collect_facts(input, false, out);
                collect_callee(callee, rest, is_result, out);
            }
            None => collect_callee(callee, &[], is_result, out),
        },
    }
}

fn collect_callee(
    callee: &mahoraga::Node,
    args: &[mahoraga::Node],
    is_result: bool,
    out: &mut Facts,
) {
    use mahoraga::{Atom, Ident, Node};
    match callee {
        Node::Atom(Atom::Ident(Ident(name))) => collect_call(name, args, is_result, out),
        other => {
            collect_facts(other, false, out);
            for arg in args {
                collect_facts(arg, false, out);
            }
        }
    }
}

/// `is_result` carries through the arguments a builtin hands back untouched,
/// so `| map({"a": "approved"})` still exposes a status literal.
fn collect_call(name: &str, args: &[mahoraga::Node], is_result: bool, out: &mut Facts) {
    use mahoraga::Node;
    out.calls.push(Call {
        name: name.to_string(),
        args: args.len(),
    });
    let passes_through = is_result && matches!(name, "map" | "default");
    for arg in args {
        match arg {
            Node::ObjectLit(fields) if passes_through => {
                for value in fields.values() {
                    collect_facts(value, true, out);
                }
            }
            _ => collect_facts(arg, passes_through, out),
        }
    }
}

/// The segments of a chain of identifier, `.field` and `["key"]` accesses.
fn static_path(node: &mahoraga::Node) -> Option<Vec<String>> {
    use mahoraga::{Atom, Ident, Node};
    match node {
        Node::Atom(Atom::Ident(Ident(root))) => Some(vec![root.clone()]),
        Node::Member {
            object,
            field: Ident(field),
        } => {
            let mut path = static_path(object)?;
            path.push(field.clone());
            Some(path)
        }
        Node::Index { object, index } => match &**index {
            Node::Atom(Atom::StrLit(key)) => {
                let mut path = static_path(object)?;
                path.push(key.clone());
                Some(path)
            }
            _ => None,
        },
        _ => None,
    }
}

/// Arguments beside the piped input, or `None` when `name` is not a function.
pub fn arity(name: &str) -> Option<usize> {
    thread_local! {
        static ARITIES: std::collections::HashMap<&'static str, usize> = env()
            .fns
            .iter()
            .map(|(name, f)| (*name, f.arity().saturating_sub(1)))
            .collect();
    }
    ARITIES.with(|m| m.get(name).copied())
}

thread_local! {
    /// Built once per thread, not once per evaluation.
    static BUILTINS: Vec<(&'static str, Rc<mahoraga::Function>)> = func::builtins().collect();
}

/// The standard functions plus our builtins, which shadow them by name.
fn env() -> mahoraga::Env {
    let mut env = mahoraga::Env::std();
    BUILTINS.with(|fns| env.fns.extend(fns.iter().map(|(n, f)| (*n, f.clone()))));
    env
}

fn to_json(v: mahoraga::Value) -> Result<Option<Value>, EvalError> {
    use mahoraga::value::{Array, Object};
    Ok(Some(match v {
        mahoraga::Value::Void => return Ok(None),
        mahoraga::Value::Null => Value::Null,
        mahoraga::Value::Bool(b) => Value::Bool(b),
        mahoraga::Value::String(s) => Value::String(s),
        mahoraga::Value::Number(n) => number(n)?,
        mahoraga::Value::Function(f) => {
            return Err(EvalError::new(
                format!("`{f}` is a function, not a value"),
                None,
            ));
        }
        mahoraga::Value::Array(Array(items)) => Value::Array(
            items
                .into_iter()
                .filter_map(|v| to_json(v).transpose())
                .collect::<Result<_, _>>()?,
        ),
        mahoraga::Value::Object(Object(fields)) => {
            // Hash maps: sort, or the same body renders two ways.
            let mut fields: Vec<_> = fields.into_iter().collect();
            fields.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                fields
                    .into_iter()
                    .filter_map(|(k, v)| to_json(v).map(|v| v.map(|v| (k, v))).transpose())
                    .collect::<Result<_, EvalError>>()?,
            )
        }
    }))
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

/// mahoraga numbers are `f64`; a whole one goes back out as an integer, so an
/// amount renders as `100` rather than `100.0`.
fn number(n: f64) -> Result<Value, EvalError> {
    const MAX_EXACT: f64 = 9_007_199_254_740_992.0; // 2^53
    if n.fract() == 0.0 && n.abs() <= MAX_EXACT {
        return Ok(Value::from(n as i64));
    }
    serde_json::Number::from_f64(n)
        .map(Value::Number)
        .ok_or_else(|| EvalError::new(format!("`{n}` is not a finite number"), None))
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
        let err = serde_json::from_str::<Expr>(r#""payment.""#).unwrap_err();
        assert!(
            err.to_string().contains("expected member accessor"),
            "{err}"
        );
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
                .roots(),
            ["payment", "x"]
        );
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
                "channel": (params.extra_return_param | null_if('_blank_')) ?? settings.channel,
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
        assert!(Expr::parse("payment.token").unwrap().as_literal().is_none());
        assert_eq!(
            Expr::empty_object().eval(&scope()).unwrap(),
            Some(json!({}))
        );
    }

    #[test]
    fn validation_sees_every_registered_function() {
        for (name, f) in &env().fns {
            assert_eq!(
                arity(name),
                Some(f.arity().saturating_sub(1)),
                "`{name}` is registered but validation disagrees about its arity"
            );
        }
    }

    #[test]
    fn every_builtin_takes_a_piped_input_and_carries_help() {
        for (name, f) in func::builtins() {
            assert!(f.arity() >= 1, "`{name}` cannot be piped into");
            assert!(f.help.is_some(), "`{name}` has no help for the editor");
        }
    }

    #[test]
    fn exposes_roots_paths_and_calls_for_validation() {
        let e = Expr::parse(r#"payment.a["b"] ?? settings.c | trim"#).unwrap();
        assert_eq!(e.roots(), vec!["payment", "settings"]);
        assert_eq!(
            e.paths(),
            vec![vec!["payment", "a", "b"], vec!["settings", "c"]]
        );
        assert_eq!(
            e.calls(),
            vec![Call {
                name: "trim".into(),
                args: 0
            }]
        );
    }

    #[test]
    fn a_dynamic_index_reads_both_sides() {
        let e = Expr::parse("steps.auth[params.key].token").unwrap();
        assert_eq!(
            e.paths(),
            vec![vec!["steps", "auth"], vec!["params", "key"]]
        );
    }

    #[test]
    fn literals_expose_the_paths_they_contain() {
        let e =
            Expr::parse(r#"[params.first_name, "x", {"phone": params.customer.phone}]"#).unwrap();
        assert_eq!(
            e.paths(),
            vec![
                vec!["params", "first_name"],
                vec!["params", "customer", "phone"]
            ]
        );
        assert!(e.string_results().is_empty());
    }

    #[test]
    fn string_results_skip_compared_literals() {
        let e = Expr::parse(r#"resp.body.s == "ok" ? "approved" : "pending""#).unwrap();
        assert_eq!(e.string_results(), vec!["approved", "pending"]);
        assert_eq!(e.roots(), vec!["resp"]);
    }
}
