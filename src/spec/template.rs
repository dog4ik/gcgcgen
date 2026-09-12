//! Templates: literal text and JSON interleaved with `{{ … }}` expressions.
//!
//! Two shapes, sharing one placeholder syntax:
//!
//! * [`Template`] — a string with holes, used for URL paths, header and query
//!   values, and signature canonical strings.
//! * [`JsonTemplate`] — a whole JSON document whose *string leaves* are
//!   [`Template`]s. This is how a request body is authored: paste the
//!   gateway's example JSON and replace the values.
//!
//! Three rendering rules carry the ergonomics:
//!
//! 1. **Typed passthrough.** A string consisting of exactly one placeholder
//!    yields that expression's value with its type intact, so
//!    `"{{ payment.gateway_amount | minor_to_major }}"` renders as the number
//!    `10`, not the string `"10"`.
//! 2. **`{{? … }}` omits.** An absent result drops the enclosing object key or
//!    array element entirely. In a mixed string (`"Bearer {{? tok }}"`) an
//!    absent hole omits the whole string.
//! 3. Non-string leaves — numbers, bools, nested objects and arrays — are
//!    literals and are emitted as-is.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::expr::{eval::eval, func::is_absent, EvalError, Expr, ExprError, Span};

// ---------------------------------------------------------------------------
// string templates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Part {
    Lit(String),
    Hole(Hole),
}

#[derive(Debug, Clone, PartialEq)]
struct Hole {
    expr: Expr,
    /// `{{? … }}` — omit the surrounding key/element when the value is absent.
    omit: bool,
    /// Byte offset of the expression source within the template, so spans in
    /// errors point at the template the author is editing.
    offset: usize,
}

/// A string with `{{ … }}` holes.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    src: String,
    parts: Vec<Part>,
}

impl Template {
    pub fn parse(src: impl Into<String>) -> Result<Self, ExprError> {
        let src = src.into();
        let parts = parse_parts(&src)?;
        Ok(Self { src, parts })
    }

    /// A template with no holes.
    pub fn literal(text: impl Into<String>) -> Self {
        let text = text.into();
        let parts = if text.is_empty() {
            Vec::new()
        } else {
            vec![Part::Lit(text.clone())]
        };
        Self { src: text, parts }
    }

    pub fn src(&self) -> &str {
        &self.src
    }

    /// True when the template contains no expressions.
    pub fn is_static(&self) -> bool {
        self.parts.iter().all(|p| matches!(p, Part::Lit(_)))
    }

    /// Renders to a value. `Ok(None)` means "omit this key/element".
    ///
    /// A lone placeholder keeps the expression's type; anything else is string
    /// interpolation.
    pub fn render(&self, scope: &Value) -> Result<Option<Value>, EvalError> {
        // Rule 1: exactly one hole and nothing else.
        if let [Part::Hole(h)] = self.parts.as_slice() {
            let v = self.eval_hole(h, scope)?;
            return Ok(if h.omit && is_absent(&v) {
                None
            } else {
                Some(v)
            });
        }

        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Lit(s) => out.push_str(s),
                Part::Hole(h) => {
                    let v = self.eval_hole(h, scope)?;
                    if is_absent(&v) {
                        // Rule 2, mixed case: one absent `{{? }}` drops the
                        // whole string, so `"Bearer {{? tok }}"` yields no
                        // header at all rather than a dangling `"Bearer "`.
                        if h.omit {
                            return Ok(None);
                        }
                        continue;
                    }
                    match v {
                        Value::String(s) => out.push_str(&s),
                        Value::Number(n) => out.push_str(&n.to_string()),
                        Value::Bool(b) => out.push_str(if b { "true" } else { "false" }),
                        Value::Null => {}
                        other => {
                            return Err(self.shift(
                                EvalError::new(
                                    format!(
                                        "cannot interpolate {} into a string",
                                        if other.is_array() {
                                            "an array"
                                        } else {
                                            "an object"
                                        }
                                    ),
                                    Some(Span::new(0, h.expr.src().len())),
                                ),
                                h,
                            ))
                        }
                    }
                }
            }
        }
        Ok(Some(Value::String(out)))
    }

    /// Renders to a string, omitting as [`Template::render`] does.
    pub fn render_string(&self, scope: &Value) -> Result<Option<String>, EvalError> {
        Ok(match self.render(scope)? {
            None => None,
            Some(Value::String(s)) => Some(s),
            Some(Value::Null) => Some(String::new()),
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

    fn eval_hole(&self, h: &Hole, scope: &Value) -> Result<Value, EvalError> {
        eval(h.expr.node(), scope).map_err(|e| self.shift(e, h))
    }

    /// Rebases an expression-relative span onto the template source.
    fn shift(&self, mut e: EvalError, h: &Hole) -> EvalError {
        if let Some(s) = e.span.as_mut() {
            *s = Span::new(s.start + h.offset, s.end + h.offset);
        }
        e
    }

    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        self.parts.iter().filter_map(|p| match p {
            Part::Hole(h) => Some(&h.expr),
            Part::Lit(_) => None,
        })
    }
}

impl fmt::Display for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.src)
    }
}

impl Serialize for Template {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.src)
    }
}

impl<'de> Deserialize<'de> for Template {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let src = String::deserialize(d)?;
        Template::parse(src).map_err(serde::de::Error::custom)
    }
}

/// Splits a template into literals and holes.
///
/// The scan for the closing `}}` respects quoted strings so an expression like
/// `map({a: '}}'})` does not terminate early.
fn parse_parts(src: &str) -> Result<Vec<Part>, ExprError> {
    let bytes = src.as_bytes();
    let mut parts = Vec::new();
    let mut lit = String::new();
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'{' && bytes.get(i + 1) == Some(&b'{') {
            let open = i;
            i += 2;
            let omit = bytes.get(i) == Some(&b'?');
            if omit {
                i += 1;
            }
            let body_start = i;
            let body_end = find_close(src, i)
                .ok_or_else(|| ExprError::new("unclosed `{{`", Span::new(open, src.len())))?;

            if !lit.is_empty() {
                parts.push(Part::Lit(std::mem::take(&mut lit)));
            }
            let body = &src[body_start..body_end];
            let expr = Expr::parse(body).map_err(|e| {
                ExprError::new(
                    e.message,
                    Span::new(e.span.start + body_start, e.span.end + body_start),
                )
            })?;
            parts.push(Part::Hole(Hole {
                expr,
                omit,
                offset: body_start,
            }));
            i = body_end + 2;
        } else if bytes[i] == b'}' && bytes.get(i + 1) == Some(&b'}') {
            return Err(ExprError::new("stray `}}`", Span::new(i, i + 2)));
        } else {
            let ch = src[i..].chars().next().expect("index is on a boundary");
            lit.push(ch);
            i += ch.len_utf8();
        }
    }

    if !lit.is_empty() {
        parts.push(Part::Lit(lit));
    }
    Ok(parts)
}

/// Byte index of the `}}` closing a hole that starts at `from`, skipping over
/// quoted string literals inside the expression.
fn find_close(src: &str, from: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut i = from;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == b'\\' {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => {
                if c == b'\'' || c == b'"' {
                    quote = Some(c);
                } else if c == b'}' && bytes.get(i + 1) == Some(&b'}') {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// JSON templates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tmpl {
    Str(Template),
    Lit(Value),
    Arr(Vec<Tmpl>),
    Obj(Vec<(String, Tmpl)>),
}

/// A JSON document whose string leaves are [`Template`]s.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonTemplate {
    root: Tmpl,
}

impl JsonTemplate {
    pub fn from_value(v: Value) -> Result<Self, ExprError> {
        Ok(Self { root: compile(v)? })
    }

    pub fn parse_str(s: &str) -> Result<Self, serde_json::Error> {
        let v: Value = serde_json::from_str(s)?;
        Self::from_value(v).map_err(serde::de::Error::custom)
    }

    /// The empty object — the default body for a request that sends nothing.
    pub fn empty() -> Self {
        Self {
            root: Tmpl::Obj(Vec::new()),
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(&self.root, Tmpl::Obj(f) if f.is_empty())
    }

    /// The authored source, for round-tripping into the editor.
    pub fn to_value(&self) -> Value {
        decompile(&self.root)
    }

    pub fn render(&self, scope: &Value) -> Result<Value, EvalError> {
        Ok(render_node(&self.root, scope)?.unwrap_or(Value::Null))
    }

    pub fn templates(&self) -> Vec<&Template> {
        let mut out = Vec::new();
        collect(&self.root, &mut out);
        out
    }
}

impl Serialize for JsonTemplate {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(s)
    }
}

impl<'de> Deserialize<'de> for JsonTemplate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        JsonTemplate::from_value(v).map_err(serde::de::Error::custom)
    }
}

fn compile(v: Value) -> Result<Tmpl, ExprError> {
    Ok(match v {
        Value::String(s) => Tmpl::Str(Template::parse(s)?),
        Value::Array(items) => Tmpl::Arr(items.into_iter().map(compile).collect::<Result<_, _>>()?),
        Value::Object(fields) => Tmpl::Obj(
            fields
                .into_iter()
                .map(|(k, v)| compile(v).map(|t| (k, t)))
                .collect::<Result<_, _>>()?,
        ),
        lit => Tmpl::Lit(lit),
    })
}

fn decompile(t: &Tmpl) -> Value {
    match t {
        Tmpl::Str(tpl) => Value::String(tpl.src().to_string()),
        Tmpl::Lit(v) => v.clone(),
        Tmpl::Arr(items) => Value::Array(items.iter().map(decompile).collect()),
        Tmpl::Obj(fields) => {
            let mut m = Map::new();
            for (k, v) in fields {
                m.insert(k.clone(), decompile(v));
            }
            Value::Object(m)
        }
    }
}

fn render_node(t: &Tmpl, scope: &Value) -> Result<Option<Value>, EvalError> {
    Ok(match t {
        Tmpl::Str(tpl) => tpl.render(scope)?,
        Tmpl::Lit(v) => Some(v.clone()),
        Tmpl::Arr(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                if let Some(v) = render_node(item, scope)? {
                    out.push(v);
                }
            }
            Some(Value::Array(out))
        }
        Tmpl::Obj(fields) => {
            let mut out = Map::new();
            for (k, v) in fields {
                if let Some(v) = render_node(v, scope)? {
                    out.insert(k.clone(), v);
                }
            }
            Some(Value::Object(out))
        }
    })
}

fn collect<'a>(t: &'a Tmpl, out: &mut Vec<&'a Template>) {
    match t {
        Tmpl::Str(tpl) => out.push(tpl),
        Tmpl::Lit(_) => {}
        Tmpl::Arr(items) => items.iter().for_each(|i| collect(i, out)),
        Tmpl::Obj(fields) => fields.iter().for_each(|(_, v)| collect(v, out)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scope() -> Value {
        json!({
            "payment": { "token": "tok_1", "gateway_amount": 1000, "product": null,
                         "order_number": "ORD-9" },
            "params":  { "phone": null, "customer": { "phone": "0798288410" },
                         "first_name": "John", "last_name": "Doe",
                         "extra_return_param": "_blank_" },
            "settings": { "wallet": "w1", "code": "MPESA", "channel": "Mpesa" },
            "steps":   { "auth": { "access_token": "abc" } },
            "env":     { "callback_url": "https://cb.example/gateway/callback" }
        })
    }

    fn render(src: &str) -> Option<Value> {
        Template::parse(src).unwrap().render(&scope()).unwrap()
    }

    #[test]
    fn lone_placeholder_keeps_its_type() {
        assert_eq!(
            render("{{ payment.gateway_amount | minor_to_major }}"),
            Some(json!(10))
        );
        assert_eq!(render("{{ payment.token }}"), Some(json!("tok_1")));
    }

    #[test]
    fn mixed_text_interpolates_to_a_string() {
        assert_eq!(
            render("Bearer {{ steps.auth.access_token }}"),
            Some(json!("Bearer abc"))
        );
        assert_eq!(
            render("{{ payment.token }}-{{ payment.gateway_amount }}"),
            Some(json!("tok_1-1000"))
        );
    }

    #[test]
    fn omit_marker_drops_the_value() {
        assert_eq!(render("{{? payment.product }}"), None);
        assert_eq!(render("{{ payment.product }}"), Some(Value::Null));
        // An absent omit-hole inside mixed text drops the whole string.
        assert_eq!(render("Bearer {{? payment.product }}"), None);
    }

    #[test]
    fn static_templates_pass_through() {
        let t = Template::parse("/v1/gateway/initiate/collection").unwrap();
        assert!(t.is_static());
        assert_eq!(
            t.render_string(&scope()).unwrap().as_deref(),
            Some("/v1/gateway/initiate/collection")
        );
    }

    #[test]
    fn braces_inside_expression_strings_do_not_close_the_hole() {
        let v = Template::parse("{{ payment.token | map({tok_1: '}}'}) }}")
            .unwrap()
            .render(&scope())
            .unwrap();
        assert_eq!(v, Some(json!("}}")));
    }

    #[test]
    fn reports_unclosed_and_stray_braces() {
        assert!(Template::parse("{{ a.b ")
            .unwrap_err()
            .message
            .contains("unclosed"));
        assert!(Template::parse("a }} b")
            .unwrap_err()
            .message
            .contains("stray"));
    }

    #[test]
    fn error_spans_are_template_relative() {
        let t = Template::parse("xx{{ params.customer | trim }}").unwrap();
        let err = t.render(&scope()).unwrap_err();
        let span = err.span.expect("span");
        // The `trim` call sits at offset 23 in the template, not 18 in the expr.
        assert_eq!(&t.src()[span.start..span.end], "trim");
    }

    #[test]
    fn json_template_renders_the_scripay_body() {
        let tpl = JsonTemplate::parse_str(
            r#"{
                 "purpose": "payment",
                 "order_id": "{{ payment.token }}",
                 "amount": "{{ payment.gateway_amount | minor_to_major }}",
                 "callback_url": "{{ env.callback_url }}",
                 "description": "{{ payment.product ?? payment.order_number ?? 'Payment' }}",
                 "wallet": "{{ settings.wallet }}",
                 "channel": "{{? params.extra_return_param | null_if('_blank_') ?? settings.channel }}",
                 "data": {
                   "phone_number": "{{ params.phone ?? params.customer.phone }}",
                   "account_name": "{{? [params.first_name, params.last_name] | join(' ') | trim }}",
                   "code": "{{ settings.code }}"
                 }
               }"#,
        )
        .unwrap();

        assert_eq!(
            tpl.render(&scope()).unwrap(),
            json!({
                "purpose": "payment",
                "order_id": "tok_1",
                "amount": 10,
                "callback_url": "https://cb.example/gateway/callback",
                "description": "ORD-9",
                "wallet": "w1",
                "channel": "Mpesa",
                "data": {
                    "phone_number": "0798288410",
                    "account_name": "John Doe",
                    "code": "MPESA"
                }
            })
        );
    }

    #[test]
    fn omitted_keys_disappear_from_objects_and_arrays() {
        let tpl = JsonTemplate::parse_str(
            r#"{"keep": 1, "drop": "{{? payment.product }}",
                "list": ["a", "{{? payment.product }}", "b"]}"#,
        )
        .unwrap();
        assert_eq!(
            tpl.render(&scope()).unwrap(),
            json!({"keep": 1, "list": ["a", "b"]})
        );
    }

    #[test]
    fn non_string_leaves_are_literals() {
        let tpl = JsonTemplate::parse_str(r#"{"n": 3, "b": true, "z": null}"#).unwrap();
        assert_eq!(
            tpl.render(&scope()).unwrap(),
            json!({"n": 3, "b": true, "z": null})
        );
    }

    #[test]
    fn json_template_round_trips_and_preserves_key_order() {
        let src = r#"{"z":"{{ payment.token }}","a":1,"m":{"y":2,"b":3}}"#;
        let tpl = JsonTemplate::parse_str(src).unwrap();
        assert_eq!(serde_json::to_string(&tpl).unwrap(), src);
    }

    #[test]
    fn bad_expression_inside_a_body_is_rejected_at_parse() {
        let err = JsonTemplate::parse_str(r#"{"a": "{{ payment. }}"}"#).unwrap_err();
        assert!(err.to_string().contains("field name after `.`"), "{err}");
    }
}
