//! The builtins, declared as mahoraga functions.

use std::rc::Rc;

use base64::Engine as _;
use mahoraga::value::Object;
use mahoraga::{declare_fn, Args, Callable, Error, Function, Value};

pub fn builtins() -> impl Iterator<Item = (&'static str, Rc<Function>)> {
    [
        skip_blank(declare_fn!(
            "minor_to_major",
            money::minor_to_major,
            (Num),
            "Minor units to major, e.g. 1000 -> 10. Stays an integer when whole."
        )),
        skip_blank(declare_fn!(
            "major_to_minor",
            money::major_to_minor,
            (Num),
            "Major units to minor, e.g. 10.5 -> 1050."
        )),
        skip_blank(declare_fn!(
            "null_if",
            absence::null_if,
            (Value, Value),
            "Absent when the input equals the argument."
        )),
        declare_fn!(
            "default",
            absence::default_to,
            (Value, Value),
            "The argument when the input is absent."
        ),
        skip_blank(declare_fn!(
            "trim",
            string::trim,
            (Text),
            "Strips surrounding whitespace."
        )),
        skip_blank(declare_fn!("upper", string::upper, (Text), "Uppercases.")),
        skip_blank(declare_fn!("lower", string::lower, (Text), "Lowercases.")),
        skip_blank(declare_fn!(
            "join",
            string::join,
            (Value, Text),
            "Joins an array, skipping absent elements."
        )),
        skip_blank(declare_fn!(
            "split",
            string::split,
            (Text, Text),
            "Splits a string into an array."
        )),
        skip_blank(declare_fn!(
            "substr",
            string::substr,
            (Text, Count),
            "substr(start), by chars."
        )),
        skip_blank(declare_fn!(
            "replace",
            string::replace,
            (Text, Text, Text),
            "Replaces every occurrence."
        )),
        skip_blank(declare_fn!(
            "pad_start",
            string::pad_start,
            (Text, Count, Text),
            "Left-pads to a width with a fill char."
        )),
        skip_blank(declare_fn!(
            "to_string",
            casts::to_string,
            (Text),
            "Renders as a string."
        )),
        skip_blank(declare_fn!(
            "to_number",
            casts::to_number,
            (Num),
            "Parses as a number."
        )),
        skip_blank(declare_fn!(
            "to_bool",
            casts::to_bool,
            (Value),
            "Truthiness as a bool."
        )),
        skip_blank(declare_fn!(
            "base64",
            encoding::base64,
            (Text),
            "Standard base64."
        )),
        skip_blank(declare_fn!(
            "base64url",
            encoding::base64url,
            (Text),
            "URL-safe base64, unpadded."
        )),
        skip_blank(declare_fn!("hex", encoding::hex, (Text), "Lowercase hex.")),
        skip_blank(declare_fn!(
            "md5",
            digests::md5,
            (Text),
            "MD5, lowercase hex."
        )),
        skip_blank(declare_fn!(
            "sha256",
            digests::sha256,
            (Text),
            "SHA-256, lowercase hex."
        )),
        skip_blank(declare_fn!(
            "sha512",
            digests::sha512,
            (Text),
            "SHA-512, lowercase hex."
        )),
        skip_blank(declare_fn!(
            "hmac_sha256",
            digests::hmac_sha256,
            (Text, Text),
            "HMAC-SHA-256 with the given key, lowercase hex."
        )),
        skip_blank(declare_fn!(
            "hmac_sha512",
            digests::hmac_sha512,
            (Text, Text),
            "HMAC-SHA-512 with the given key, lowercase hex."
        )),
        skip_blank(declare_fn!(
            "map",
            table::map_lookup,
            (Text, Object),
            "Table lookup, e.g. status | map({\"Success\": \"approved\", \"_default\": \"pending\"})."
        )),
    ]
    .into_iter()
}

/// Blank in, blank out: both become void, so the key, element or field the
/// value was filling is dropped rather than sent empty.
struct SkipBlank(Box<dyn Callable>);

impl Callable for SkipBlank {
    fn call(&self, args: Args) -> mahoraga::Result<Value> {
        if args.0.first().is_some_and(Value::blank) {
            return Ok(Value::Void);
        }
        let out = self.0.call(args)?;
        Ok(if out.blank() { Value::Void } else { out })
    }

    fn arity(&self) -> usize {
        self.0.arity()
    }
}

fn skip_blank((name, f): (&'static str, Rc<Function>)) -> (&'static str, Rc<Function>) {
    let f = Rc::try_unwrap(f).expect("just declared, so unique");
    let wrapped = Function {
        name: f.name,
        help: f.help,
        f: Box::new(SkipBlank(f.f)),
    };
    (name, Rc::new(wrapped))
}

/// A scalar as text. Containers are an error rather than a silent `"[object]"`.
struct Text(String);

impl TryFrom<Value> for Text {
    type Error = Error;

    fn try_from(v: Value) -> Result<Self, Error> {
        Ok(Text(match v {
            Value::String(s) => s,
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null | Value::Void => String::new(),
            other => return Err(expected("a scalar", &other)),
        }))
    }
}

/// A number, or a string that reads as one: gateways quote their amounts.
struct Num(f64);

impl TryFrom<Value> for Num {
    type Error = Error;

    fn try_from(v: Value) -> Result<Self, Error> {
        match v {
            Value::Number(n) => Ok(Num(n)),
            Value::String(ref s) => s
                .trim()
                .parse()
                .map(Num)
                .map_err(|_| Error::new(format!("`{s}` is not a number"))),
            other => Err(expected("a number", &other)),
        }
    }
}

/// A whole, non-negative number, for a length or an offset.
struct Count(usize);

impl TryFrom<Value> for Count {
    type Error = Error;

    fn try_from(v: Value) -> Result<Self, Error> {
        match Num::try_from(v)?.0 {
            n if n >= 0.0 && n.fract() == 0.0 => Ok(Count(n as usize)),
            n => Err(Error::new(format!(
                "expected a whole, non-negative number, got {n}"
            ))),
        }
    }
}

fn expected(what: &str, got: &Value) -> Error {
    Error::new(format!("expected {what}, got {}", got.value_type()))
}

mod money {
    use super::*;

    /// Fixed, because a builtin has one arity: a zero-decimal currency needs
    /// optional arguments first.
    const EXPONENT: i32 = 2;

    /// Mirrors oxyscripay's `minor_to_major`: whole amounts stay integers
    /// because some gateways reject `100.0` where they accept `100`.
    pub fn minor_to_major(v: Num) -> mahoraga::Result<Value> {
        if v.0.fract() != 0.0 {
            return Err(Error::new(format!(
                "minor units must be a whole number, got {}",
                v.0
            )));
        }
        Ok(Value::Number(v.0 / 10f64.powi(EXPONENT)))
    }

    pub fn major_to_minor(v: Num) -> mahoraga::Result<Value> {
        let scaled = (v.0 * 10f64.powi(EXPONENT)).round();
        if !scaled.is_finite() {
            return Err(Error::new("amount is not finite"));
        }
        Ok(Value::Number(scaled.max(0.0)))
    }
}

mod absence {
    use super::*;

    pub fn null_if(v: Value, other: Value) -> mahoraga::Result<Value> {
        Ok(if v == other { Value::Void } else { v })
    }

    pub fn default_to(v: Value, fallback: Value) -> mahoraga::Result<Value> {
        Ok(if v.blank() { fallback } else { v })
    }
}

mod string {
    use super::*;

    pub fn trim(v: Text) -> mahoraga::Result<Value> {
        Ok(Value::String(v.0.trim().to_string()))
    }

    pub fn upper(v: Text) -> mahoraga::Result<Value> {
        Ok(Value::String(v.0.to_uppercase()))
    }

    pub fn lower(v: Text) -> mahoraga::Result<Value> {
        Ok(Value::String(v.0.to_lowercase()))
    }

    /// Joins an array (or passes a scalar through), skipping absent elements so
    /// `[first_name, last_name] | join(" ")` never yields a leading space.
    pub fn join(v: Value, sep: Text) -> mahoraga::Result<Value> {
        let items = match v {
            Value::Array(items) => items.0,
            scalar => vec![scalar],
        };
        let parts = items
            .into_iter()
            .filter(|i| !i.blank())
            .map(|i| Text::try_from(i).map(|t| t.0))
            .collect::<mahoraga::Result<Vec<_>>>()?;
        Ok(Value::String(parts.join(&sep.0)))
    }

    pub fn split(v: Text, sep: Text) -> mahoraga::Result<Value> {
        if sep.0.is_empty() {
            return Err(Error::new("separator must not be empty"));
        }
        let parts: Vec<Value> = v.0.split(sep.0.as_str()).map(Value::from).collect();
        Ok(parts.into())
    }

    pub fn substr(v: Text, start: Count) -> mahoraga::Result<Value> {
        Ok(Value::String(v.0.chars().skip(start.0).collect()))
    }

    pub fn replace(v: Text, from: Text, to: Text) -> mahoraga::Result<Value> {
        if from.0.is_empty() {
            return Err(Error::new("pattern must not be empty"));
        }
        Ok(Value::String(v.0.replace(from.0.as_str(), &to.0)))
    }

    pub fn pad_start(v: Text, width: Count, fill: Text) -> mahoraga::Result<Value> {
        let mut chars = fill.0.chars();
        let (Some(fill), None) = (chars.next(), chars.next()) else {
            return Err(Error::new("fill must be exactly one character"));
        };
        let missing = width.0.saturating_sub(v.0.chars().count());
        Ok(Value::String(
            std::iter::repeat_n(fill, missing)
                .chain(v.0.chars())
                .collect(),
        ))
    }
}

mod casts {
    use super::*;

    pub fn to_string(v: Text) -> mahoraga::Result<Value> {
        Ok(Value::String(v.0))
    }

    pub fn to_number(v: Num) -> mahoraga::Result<Value> {
        Ok(Value::Number(v.0))
    }

    pub fn to_bool(v: Value) -> mahoraga::Result<Value> {
        Ok(Value::Bool(match &v {
            Value::String(s) => !matches!(s.to_ascii_lowercase().as_str(), "false" | "0" | "no"),
            other => other.truthy(),
        }))
    }
}

mod encoding {
    use super::*;

    pub fn base64(v: Text) -> mahoraga::Result<Value> {
        Ok(Value::String(
            base64::engine::general_purpose::STANDARD.encode(v.0),
        ))
    }

    pub fn base64url(v: Text) -> mahoraga::Result<Value> {
        Ok(Value::String(
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.0),
        ))
    }

    pub fn hex(v: Text) -> mahoraga::Result<Value> {
        Ok(Value::String(hex::encode(v.0)))
    }
}

mod digests {
    use super::*;

    pub fn md5(v: Text) -> mahoraga::Result<Value> {
        use md5::{Digest, Md5};
        Ok(Value::String(hex::encode(Md5::digest(v.0.as_bytes()))))
    }

    pub fn sha256(v: Text) -> mahoraga::Result<Value> {
        use sha2::{Digest, Sha256};
        Ok(Value::String(hex::encode(Sha256::digest(v.0.as_bytes()))))
    }

    pub fn sha512(v: Text) -> mahoraga::Result<Value> {
        use sha2::{Digest, Sha512};
        Ok(Value::String(hex::encode(Sha512::digest(v.0.as_bytes()))))
    }

    pub fn hmac_sha256(v: Text, key: Text) -> mahoraga::Result<Value> {
        use hmac::{Hmac, KeyInit, Mac};
        let mut mac = <Hmac<sha2::Sha256>>::new_from_slice(key.0.as_bytes())
            .map_err(|e| Error::new(format!("invalid hmac key: {e}")))?;
        mac.update(v.0.as_bytes());
        Ok(Value::String(hex::encode(mac.finalize().into_bytes())))
    }

    pub fn hmac_sha512(v: Text, key: Text) -> mahoraga::Result<Value> {
        use hmac::{Hmac, KeyInit, Mac};
        let mut mac = <Hmac<sha2::Sha512>>::new_from_slice(key.0.as_bytes())
            .map_err(|e| Error::new(format!("invalid hmac key: {e}")))?;
        mac.update(v.0.as_bytes());
        Ok(Value::String(hex::encode(mac.finalize().into_bytes())))
    }
}

mod table {
    use super::*;

    /// `status | map({"Success": "approved", "_default": "pending"})`
    pub fn map_lookup(v: Text, table: Object) -> mahoraga::Result<Value> {
        Ok(table
            .0
            .get(&v.0)
            .or_else(|| table.0.get("_default"))
            .cloned()
            .unwrap_or(Value::Void))
    }
}

#[cfg(test)]
mod tests {
    use crate::spec::expr::Expr;
    use serde_json::{json, Value};

    /// Builtins are exercised the way a document reaches them.
    fn ev(src: &str) -> Option<Value> {
        Expr::parse(src)
            .unwrap_or_else(|e| panic!("{src}: {e}"))
            .eval(&json!({}))
            .unwrap_or_else(|e| panic!("{src}: {e}"))
    }

    fn err(src: &str) -> String {
        Expr::parse(src)
            .unwrap()
            .eval(&json!({}))
            .expect_err(src)
            .message
    }

    #[test]
    fn minor_to_major_keeps_whole_amounts_integral() {
        assert_eq!(ev("1000 | minor_to_major"), Some(json!(10)));
        assert_eq!(ev("1050 | minor_to_major"), Some(json!(10.5)));
        assert!(err("10.5 | minor_to_major").contains("whole number"));
    }

    #[test]
    fn major_to_minor_rounds() {
        assert_eq!(ev("10.5 | major_to_minor"), Some(json!(1050)));
        assert_eq!(ev("10.005 | major_to_minor"), Some(json!(1001)));
        assert_eq!(ev("-1 | major_to_minor"), Some(json!(0)));
    }

    #[test]
    fn money_round_trips() {
        for minor in [1u64, 99, 100, 1000, 123456] {
            let major = ev(&format!("{minor} | minor_to_major")).unwrap();
            assert_eq!(ev(&format!("{major} | major_to_minor")), Some(json!(minor)));
        }
    }

    #[test]
    fn join_skips_absent_and_is_absent_when_empty() {
        assert_eq!(
            ev("['Satoru', 'Gojo'] | join(' ')"),
            Some(json!("Satoru Gojo"))
        );
        // A missing first name must not leave a leading space.
        assert_eq!(ev("[nope, 'Gojo'] | join(' ')"), Some(json!("Gojo")));
        assert_eq!(ev("['', ''] | join(' ')"), None);
    }

    #[test]
    fn blank_input_short_circuits() {
        assert_eq!(ev("nope | trim"), None);
        assert_eq!(ev("'' | upper"), None);
        assert_eq!(ev("'  ' | trim | upper"), None);
        // `default` is the one builtin that must see absence.
        assert_eq!(ev("nope | default('x')"), Some(json!("x")));
        assert_eq!(ev("'' | default('x')"), Some(json!("x")));
    }

    #[test]
    fn null_if_strips_sentinels() {
        assert_eq!(ev("'_blank_' | null_if('_blank_')"), None);
        assert_eq!(ev("'Mpesa' | null_if('_blank_')"), Some(json!("Mpesa")));
        assert_eq!(
            ev("('_blank_' | null_if('_blank_')) ?? 'fallback'"),
            Some(json!("fallback"))
        );
    }

    #[test]
    fn map_falls_back_to_default() {
        const T: &str = r#"| map({"Success": "approved", "_default": "pending"})"#;
        assert_eq!(ev(&format!("'Success' {T}")), Some(json!("approved")));
        assert_eq!(ev(&format!("'Weird' {T}")), Some(json!("pending")));
        // No `_default` and no hit is absent, not an error.
        assert_eq!(ev(r#"'Weird' | map({"a": 1})"#), None);
        assert!(err("'x' | map('nope')").contains("expected object"));
    }

    #[test]
    fn string_helpers() {
        assert_eq!(ev("'abcdef' | substr(2)"), Some(json!("cdef")));
        assert_eq!(ev("'abc' | substr(10)"), None);
        assert_eq!(ev("'a-b-c' | replace('-', '')"), Some(json!("abc")));
        assert_eq!(ev("7 | pad_start(3, '0')"), Some(json!("007")));
        assert_eq!(ev("'a,b' | split(',')"), Some(json!(["a", "b"])));
        assert!(err("'a' | pad_start(-1, '0')").contains("non-negative"));
    }

    #[test]
    fn a_number_reaches_a_text_builtin() {
        assert_eq!(ev("1000 | to_string"), Some(json!("1000")));
        assert_eq!(ev("'10' | to_number"), Some(json!(10)));
        assert_eq!(ev("'42' | sha256"), ev("42 | sha256"));
    }

    #[test]
    fn arrays_and_objects_are_not_scalars() {
        assert!(err("[1, 2] | trim").contains("scalar"));
        assert!(err(r#"{"a": 1} | upper"#).contains("scalar"));
    }

    #[test]
    fn a_failing_builtin_names_itself() {
        assert!(err("'x' | to_number").starts_with("to_number:"));
    }
}
