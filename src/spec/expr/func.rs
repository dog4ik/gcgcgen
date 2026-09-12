//! The builtin function table.
//!
//! Every builtin is pure: it takes the piped-in value plus already-evaluated
//! arguments and returns a value. Nothing here reads the clock, the network or
//! a random source — non-deterministic inputs (`env.now`, `env.request_id`)
//! are injected into the scope by the caller instead, which keeps previews in
//! the browser identical to execution on the server.

use base64::Engine as _;
use serde_json::{Map, Number, Value};

/// A value is "absent" when it is `null` or the empty string. Absence drives
/// `??`, `{{? }}` omission, and argument skipping in [`join`]-style builtins.
///
/// Treating `""` as absent is deliberate: gateways and the platform use empty
/// strings and missing keys interchangeably, and the alternative is every
/// expression in every spec carrying a `| null_if('')`.
pub fn is_absent(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        _ => false,
    }
}

pub struct FnSpec {
    pub name: &'static str,
    pub min_args: usize,
    pub max_args: usize,
    /// When true, an absent input short-circuits to `null` without calling `f`.
    pub skip_absent: bool,
    pub f: fn(Value, &[Value]) -> Result<Value, String>,
    pub help: &'static str,
}

pub fn lookup(name: &str) -> Option<&'static FnSpec> {
    BUILTINS.iter().find(|s| s.name == name)
}

pub fn names() -> impl Iterator<Item = &'static str> {
    BUILTINS.iter().map(|s| s.name)
}

macro_rules! spec {
    ($name:literal, $min:literal..=$max:literal, $skip:literal, $f:expr, $help:literal) => {
        FnSpec {
            name: $name,
            min_args: $min,
            max_args: $max,
            skip_absent: $skip,
            f: $f,
            help: $help,
        }
    };
}

static BUILTINS: &[FnSpec] = &[
    // -- money ------------------------------------------------------------
    spec!(
        "minor_to_major",
        0..=1,
        true,
        minor_to_major,
        "Minor units to major, e.g. 1000 -> 10. Stays an integer when whole."
    ),
    spec!(
        "major_to_minor",
        0..=1,
        true,
        major_to_minor,
        "Major units to minor, e.g. 10.5 -> 1050."
    ),
    // -- absence ----------------------------------------------------------
    spec!(
        "null_if",
        1..=1,
        true,
        null_if,
        "null when the input equals the argument."
    ),
    spec!(
        "default",
        1..=1,
        false,
        default_to,
        "The argument when the input is absent."
    ),
    // -- string -----------------------------------------------------------
    spec!(
        "trim",
        0..=0,
        true,
        |v, _| text(&v).map(|s| str_or_null(s.trim())),
        "Strips surrounding whitespace."
    ),
    spec!(
        "upper",
        0..=0,
        true,
        |v, _| text(&v).map(|s| Value::String(s.to_uppercase())),
        "Uppercases."
    ),
    spec!(
        "lower",
        0..=0,
        true,
        |v, _| text(&v).map(|s| Value::String(s.to_lowercase())),
        "Lowercases."
    ),
    spec!(
        "join",
        0..=1,
        true,
        join,
        "Joins an array, skipping absent elements."
    ),
    spec!(
        "split",
        1..=1,
        true,
        split,
        "Splits a string into an array."
    ),
    spec!(
        "concat",
        0..=8,
        true,
        concat,
        "Appends every argument to the input."
    ),
    spec!(
        "substr",
        1..=2,
        true,
        substr,
        "substr(start) or substr(start, len), by chars."
    ),
    spec!(
        "replace",
        2..=2,
        true,
        replace,
        "Replaces every occurrence."
    ),
    spec!(
        "pad_start",
        2..=2,
        true,
        pad_start,
        "Left-pads to a width with a fill char."
    ),
    // -- casts ------------------------------------------------------------
    spec!(
        "to_string",
        0..=0,
        true,
        |v, _| text(&v).map(Value::String),
        "Renders as a string."
    ),
    spec!("to_number", 0..=0, true, to_number, "Parses as a number."),
    spec!("to_bool", 0..=0, true, to_bool, "Truthiness as a bool."),
    // -- encoding ---------------------------------------------------------
    spec!(
        "base64",
        0..=0,
        true,
        |v, _| bytes(&v)
            .map(|b| Value::String(base64::engine::general_purpose::STANDARD.encode(b))),
        "Standard base64."
    ),
    spec!(
        "base64url",
        0..=0,
        true,
        |v, _| bytes(&v)
            .map(|b| Value::String(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b))),
        "URL-safe base64, unpadded."
    ),
    spec!(
        "hex",
        0..=0,
        true,
        |v, _| bytes(&v).map(|b| Value::String(hex::encode(b))),
        "Lowercase hex."
    ),
    // -- digests ----------------------------------------------------------
    spec!(
        "md5",
        0..=0,
        true,
        |v, _| digest_md5(&v),
        "MD5, lowercase hex."
    ),
    spec!(
        "sha256",
        0..=0,
        true,
        |v, _| digest_sha256(&v),
        "SHA-256, lowercase hex."
    ),
    spec!(
        "sha512",
        0..=0,
        true,
        |v, _| digest_sha512(&v),
        "SHA-512, lowercase hex."
    ),
    spec!(
        "hmac_sha256",
        1..=1,
        true,
        hmac_sha256,
        "HMAC-SHA-256 with the given key, lowercase hex."
    ),
    spec!(
        "hmac_sha512",
        1..=1,
        true,
        hmac_sha512,
        "HMAC-SHA-512 with the given key, lowercase hex."
    ),
    // -- lookup -----------------------------------------------------------
    spec!(
        "map",
        1..=1,
        true,
        map_lookup,
        "Table lookup, e.g. status | map({Success:'approved', _default:'pending'})."
    ),
];

// ---------------------------------------------------------------------------
// coercions
// ---------------------------------------------------------------------------

/// Renders a scalar as text. Arrays and objects have no sensible rendering
/// here, so they are an error rather than a silent `"[object]"`.
fn text(v: &Value) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null => Ok(String::new()),
        Value::Array(_) => Err("expected a scalar, got an array".into()),
        Value::Object(_) => Err("expected a scalar, got an object".into()),
    }
}

/// Empty text becomes `null` so that absence propagates through a pipeline
/// instead of turning into `""` halfway down it.
fn str_or_null(s: &str) -> Value {
    if s.is_empty() {
        Value::Null
    } else {
        Value::String(s.to_string())
    }
}

fn bytes(v: &Value) -> Result<Vec<u8>, String> {
    text(v).map(String::into_bytes)
}

fn number(v: &Value) -> Result<f64, String> {
    match v {
        Value::Number(n) => n.as_f64().ok_or_else(|| "number is out of range".into()),
        Value::String(s) => s
            .trim()
            .parse()
            .map_err(|_| format!("`{s}` is not a number")),
        other => Err(format!("expected a number, got {}", kind(other))),
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a bool",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn exponent(args: &[Value]) -> Result<u32, String> {
    match args.first() {
        None => Ok(2),
        Some(v) => {
            let n = number(v)?;
            if !(0.0..=9.0).contains(&n) || n.fract() != 0.0 {
                return Err(format!("exponent must be an integer 0..=9, got {n}"));
            }
            Ok(n as u32)
        }
    }
}

// ---------------------------------------------------------------------------
// money
// ---------------------------------------------------------------------------

/// Mirrors oxyscripay's `minor_to_major`: whole amounts stay integers because
/// some gateways reject `100.0` where they accept `100`.
fn minor_to_major(v: Value, args: &[Value]) -> Result<Value, String> {
    let exp = exponent(args)?;
    let divisor = 10u64.pow(exp);
    let minor = number(&v)?;
    if minor.fract() != 0.0 {
        return Err(format!("minor units must be a whole number, got {minor}"));
    }
    let minor = minor as i64;
    if minor % (divisor as i64) == 0 {
        Ok(Value::Number(Number::from(minor / divisor as i64)))
    } else {
        let major = minor as f64 / divisor as f64;
        Number::from_f64(major)
            .map(Value::Number)
            .ok_or_else(|| format!("{major} is not representable"))
    }
}

fn major_to_minor(v: Value, args: &[Value]) -> Result<Value, String> {
    let exp = exponent(args)?;
    let scaled = (number(&v)? * 10f64.powi(exp as i32)).round();
    if !scaled.is_finite() {
        return Err("amount is not finite".into());
    }
    Ok(Value::Number(Number::from(scaled.max(0.0) as u64)))
}

// ---------------------------------------------------------------------------
// absence
// ---------------------------------------------------------------------------

fn null_if(v: Value, args: &[Value]) -> Result<Value, String> {
    Ok(if v == args[0] { Value::Null } else { v })
}

fn default_to(v: Value, args: &[Value]) -> Result<Value, String> {
    Ok(if is_absent(&v) { args[0].clone() } else { v })
}

// ---------------------------------------------------------------------------
// string
// ---------------------------------------------------------------------------

/// Joins an array (or passes a scalar through), skipping absent elements so
/// `[first_name, last_name] | join(' ')` never yields a leading space.
fn join(v: Value, args: &[Value]) -> Result<Value, String> {
    let sep = match args.first() {
        Some(s) => text(s)?,
        None => String::new(),
    };
    let items = match v {
        Value::Array(items) => items,
        scalar => vec![scalar],
    };
    let parts = items
        .iter()
        .filter(|i| !is_absent(i))
        .map(text)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(str_or_null(&parts.join(&sep)))
}

fn split(v: Value, args: &[Value]) -> Result<Value, String> {
    let sep = text(&args[0])?;
    if sep.is_empty() {
        return Err("split separator must not be empty".into());
    }
    Ok(Value::Array(
        text(&v)?
            .split(sep.as_str())
            .map(|p| Value::String(p.to_string()))
            .collect(),
    ))
}

fn concat(v: Value, args: &[Value]) -> Result<Value, String> {
    let mut out = text(&v)?;
    for a in args {
        if !is_absent(a) {
            out.push_str(&text(a)?);
        }
    }
    Ok(str_or_null(&out))
}

fn substr(v: Value, args: &[Value]) -> Result<Value, String> {
    let s = text(&v)?;
    let start = number(&args[0])?;
    if start < 0.0 || start.fract() != 0.0 {
        return Err(format!("start must be a non-negative integer, got {start}"));
    }
    let chars: Vec<char> = s.chars().collect();
    let start = (start as usize).min(chars.len());
    let end = match args.get(1) {
        None => chars.len(),
        Some(l) => {
            let len = number(l)?;
            if len < 0.0 || len.fract() != 0.0 {
                return Err(format!("length must be a non-negative integer, got {len}"));
            }
            start.saturating_add(len as usize).min(chars.len())
        }
    };
    Ok(str_or_null(&chars[start..end].iter().collect::<String>()))
}

fn replace(v: Value, args: &[Value]) -> Result<Value, String> {
    let from = text(&args[0])?;
    if from.is_empty() {
        return Err("replace pattern must not be empty".into());
    }
    Ok(str_or_null(
        &text(&v)?.replace(from.as_str(), &text(&args[1])?),
    ))
}

fn pad_start(v: Value, args: &[Value]) -> Result<Value, String> {
    let width = number(&args[0])?;
    if width < 0.0 || width.fract() != 0.0 {
        return Err(format!("width must be a non-negative integer, got {width}"));
    }
    let fill = text(&args[1])?;
    let mut fill = fill.chars();
    let (Some(fill), None) = (fill.next(), fill.next()) else {
        return Err("fill must be exactly one character".into());
    };
    let s = text(&v)?;
    let missing = (width as usize).saturating_sub(s.chars().count());
    Ok(Value::String(
        std::iter::repeat_n(fill, missing)
            .chain(s.chars())
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// casts
// ---------------------------------------------------------------------------

fn to_number(v: Value, _: &[Value]) -> Result<Value, String> {
    let n = number(&v)?;
    Number::from_f64(n)
        .map(Value::Number)
        .ok_or_else(|| format!("{n} is not representable"))
}

fn to_bool(v: Value, _: &[Value]) -> Result<Value, String> {
    Ok(Value::Bool(match &v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !matches!(s.to_ascii_lowercase().as_str(), "false" | "0" | "no"),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Null => false,
    }))
}

// ---------------------------------------------------------------------------
// digests
// ---------------------------------------------------------------------------

fn digest_md5(v: &Value) -> Result<Value, String> {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(bytes(v)?);
    Ok(Value::String(hex::encode(h.finalize())))
}

fn digest_sha256(v: &Value) -> Result<Value, String> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes(v)?);
    Ok(Value::String(hex::encode(h.finalize())))
}

fn digest_sha512(v: &Value) -> Result<Value, String> {
    use sha2::{Digest, Sha512};
    let mut h = Sha512::new();
    h.update(bytes(v)?);
    Ok(Value::String(hex::encode(h.finalize())))
}

fn hmac_sha256(v: Value, args: &[Value]) -> Result<Value, String> {
    use hmac::{Hmac, KeyInit, Mac};
    let mut mac = <Hmac<sha2::Sha256>>::new_from_slice(&bytes(&args[0])?)
        .map_err(|e| format!("invalid hmac key: {e}"))?;
    mac.update(&bytes(&v)?);
    Ok(Value::String(hex::encode(mac.finalize().into_bytes())))
}

fn hmac_sha512(v: Value, args: &[Value]) -> Result<Value, String> {
    use hmac::{Hmac, KeyInit, Mac};
    let mut mac = <Hmac<sha2::Sha512>>::new_from_slice(&bytes(&args[0])?)
        .map_err(|e| format!("invalid hmac key: {e}"))?;
    mac.update(&bytes(&v)?);
    Ok(Value::String(hex::encode(mac.finalize().into_bytes())))
}

// ---------------------------------------------------------------------------
// lookup
// ---------------------------------------------------------------------------

/// `status | map({Success: 'approved', _default: 'pending'})`
fn map_lookup(v: Value, args: &[Value]) -> Result<Value, String> {
    let Value::Object(table) = &args[0] else {
        return Err(format!("map expects an object, got {}", kind(&args[0])));
    };
    let key = text(&v)?;
    Ok(lookup_key(table, &key))
}

fn lookup_key(table: &Map<String, Value>, key: &str) -> Value {
    table
        .get(key)
        .or_else(|| table.get("_default"))
        .cloned()
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, input: Value, args: &[Value]) -> Result<Value, String> {
        let spec = lookup(name).unwrap_or_else(|| panic!("no builtin `{name}`"));
        if spec.skip_absent && is_absent(&input) {
            return Ok(Value::Null);
        }
        (spec.f)(input, args)
    }

    #[test]
    fn minor_to_major_keeps_whole_amounts_integral() {
        assert_eq!(call("minor_to_major", json!(1000), &[]).unwrap(), json!(10));
        assert_eq!(
            call("minor_to_major", json!(1050), &[]).unwrap(),
            json!(10.5)
        );
        // JPY-style zero-exponent currency
        assert_eq!(
            call("minor_to_major", json!(1000), &[json!(0)]).unwrap(),
            json!(1000)
        );
    }

    #[test]
    fn major_to_minor_rounds() {
        assert_eq!(
            call("major_to_minor", json!(10.5), &[]).unwrap(),
            json!(1050)
        );
        assert_eq!(
            call("major_to_minor", json!(10.005), &[]).unwrap(),
            json!(1001)
        );
        assert_eq!(call("major_to_minor", json!(-1), &[]).unwrap(), json!(0));
    }

    #[test]
    fn money_round_trips() {
        for minor in [1u64, 99, 100, 1000, 123456] {
            let major = call("minor_to_major", json!(minor), &[]).unwrap();
            assert_eq!(call("major_to_minor", major, &[]).unwrap(), json!(minor));
        }
    }

    #[test]
    fn join_skips_absent_and_yields_null_when_empty() {
        assert_eq!(
            call("join", json!(["John", "Doe"]), &[json!(" ")]).unwrap(),
            json!("John Doe")
        );
        // A missing first name must not leave a leading space.
        assert_eq!(
            call("join", json!([null, "Doe"]), &[json!(" ")]).unwrap(),
            json!("Doe")
        );
        assert_eq!(
            call("join", json!(["", ""]), &[json!(" ")]).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn absent_input_short_circuits() {
        assert_eq!(call("trim", Value::Null, &[]).unwrap(), Value::Null);
        assert_eq!(call("upper", json!(""), &[]).unwrap(), Value::Null);
        // `default` is the one builtin that must see absence.
        assert_eq!(
            call("default", Value::Null, &[json!("x")]).unwrap(),
            json!("x")
        );
    }

    #[test]
    fn null_if_strips_sentinels() {
        assert_eq!(
            call("null_if", json!("_blank_"), &[json!("_blank_")]).unwrap(),
            Value::Null
        );
        assert_eq!(
            call("null_if", json!("Mpesa"), &[json!("_blank_")]).unwrap(),
            json!("Mpesa")
        );
    }

    #[test]
    fn map_falls_back_to_default() {
        let table = json!({"Success": "approved", "Failed": "declined", "_default": "pending"});
        assert_eq!(
            call("map", json!("Success"), &[table.clone()]).unwrap(),
            json!("approved")
        );
        assert_eq!(
            call("map", json!("Weird"), &[table]).unwrap(),
            json!("pending")
        );
        // No `_default` and no hit is null, not an error.
        assert_eq!(
            call("map", json!("Weird"), &[json!({"a": 1})]).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn digests_match_known_vectors() {
        assert_eq!(
            call("sha256", json!("abc"), &[]).unwrap(),
            json!("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert_eq!(
            call("md5", json!("abc"), &[]).unwrap(),
            json!("900150983cd24fb0d6963f7d28e17f72")
        );
        // RFC 4231 test case 1
        assert_eq!(
            call("hmac_sha256", json!("Hi There"), &[json!("\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b")]).unwrap(),
            json!("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7")
        );
    }

    #[test]
    fn encodings() {
        assert_eq!(
            call("base64", json!("hello"), &[]).unwrap(),
            json!("aGVsbG8=")
        );
        assert_eq!(
            call("base64url", json!("hello"), &[]).unwrap(),
            json!("aGVsbG8")
        );
        assert_eq!(call("hex", json!("hi"), &[]).unwrap(), json!("6869"));
    }

    #[test]
    fn string_helpers() {
        assert_eq!(
            call("substr", json!("abcdef"), &[json!(1), json!(3)]).unwrap(),
            json!("bcd")
        );
        assert_eq!(
            call("substr", json!("abc"), &[json!(10)]).unwrap(),
            Value::Null
        );
        assert_eq!(
            call("replace", json!("a-b-c"), &[json!("-"), json!("")]).unwrap(),
            json!("abc")
        );
        assert_eq!(
            call("pad_start", json!(7), &[json!(3), json!("0")]).unwrap(),
            json!("007")
        );
        assert_eq!(
            call("split", json!("a,b"), &[json!(",")]).unwrap(),
            json!(["a", "b"])
        );
    }

    #[test]
    fn arrays_and_objects_are_not_scalars() {
        let err = call("trim", json!([1, 2]), &[]).unwrap_err();
        assert!(err.contains("array"), "{err}");
    }
}
