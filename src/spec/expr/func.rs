//! Our builtins, registered over mahoraga's standard functions.

use std::collections::HashMap;
use std::rc::Rc;

use base64::Engine as _;
use mahoraga::error::{FunctionCallError, RuntimeError};
use mahoraga::fns::{
    ArgConversionError, Args, Callable, ExecutionContext, Function, TryFromArgument,
};
use mahoraga::value::{Number, ValueType};
use mahoraga::{declare_fn, Value};

type Ctx<'a> = &'a mut dyn ExecutionContext;
type Out = Result<Value, RuntimeError>;

/// Standard functions that exist to see absence, so blank input must reach them.
const SEES_ABSENCE: &[&str] = &["blank_as_null", "blank_as_void", "void_as_null", "len"];

pub fn fns() -> HashMap<&'static str, Rc<Function>> {
    let mut fns: HashMap<_, _> = mahoraga::env::Env::std()
        .fns
        .into_iter()
        .map(|(name, f)| match SEES_ABSENCE.contains(&name) {
            true => (name, f),
            false => skip_blank((name, f)),
        })
        .collect();
    fns.extend(builtins().map(skip_blank));
    fns
}

fn builtins() -> impl Iterator<Item = (&'static str, Rc<Function>)> {
    [
        declare_fn!(
            "minor_to_major",
            money::minor_to_major,
            (Num),
            "Minor units to major, e.g. 1000 -> 10. Stays an integer when whole."
        ),
        declare_fn!(
            "major_to_minor",
            money::major_to_minor,
            (Num),
            "Major units to minor, e.g. 10.5 -> 1050."
        ),
        declare_fn!("base64", encoding::base64, (String), "Standard base64."),
        declare_fn!(
            "base64url",
            encoding::base64url,
            (String),
            "URL-safe base64, unpadded."
        ),
        declare_fn!("hex", encoding::hex, (String), "Lowercase hex."),
        declare_fn!("md5", digests::md5, (String), "MD5, lowercase hex."),
        declare_fn!(
            "sha256",
            digests::sha256,
            (String),
            "SHA-256, lowercase hex."
        ),
        declare_fn!(
            "sha512",
            digests::sha512,
            (String),
            "SHA-512, lowercase hex."
        ),
        declare_fn!(
            "hmac_sha256",
            digests::hmac_sha256,
            (String, String),
            "HMAC-SHA-256 with the given key, lowercase hex."
        ),
        declare_fn!(
            "hmac_sha512",
            digests::hmac_sha512,
            (String, String),
            "HMAC-SHA-512 with the given key, lowercase hex."
        ),
        // Generic: these belong in mahoraga's std.
        declare_fn!(
            "to_number",
            generic::to_number,
            (Num),
            "Parses as a number."
        ),
    ]
    .into_iter()
}

/// Blank in, blank out: both become void, so the key, element or field the
/// value was filling is dropped rather than sent empty.
struct SkipBlank(Rc<Function>);

impl Callable for SkipBlank {
    fn call(&self, ctx: Ctx, args: Args) -> Out {
        if args.0.first().is_some_and(Value::blank) {
            return Ok(Value::Void);
        }
        let out = self.0.call(ctx, args)?;
        Ok(if out.blank() { Value::Void } else { out })
    }

    fn arity(&self) -> usize {
        self.0.arity()
    }
}

fn skip_blank((name, f): (&'static str, Rc<Function>)) -> (&'static str, Rc<Function>) {
    let wrapped = Function {
        name: f.name.clone(),
        help: f.help,
        f: Box::new(SkipBlank(f)),
    };
    (name, Rc::new(wrapped))
}

fn external(message: impl Into<String>) -> RuntimeError {
    FunctionCallError::External(message.into()).into()
}

/// A number, or a string that reads as one: gateways quote their amounts.
struct Num(Number);

impl TryFromArgument for Num {
    fn parse_argument(v: Value) -> Result<Self, ArgConversionError> {
        let parsed = match &v {
            Value::Number(n) => Some(*n),
            Value::String(s) => {
                let s = s.trim();
                s.parse::<i64>()
                    .map(Number::Int)
                    .or_else(|_| s.parse::<f64>().map(Number::Float))
                    .ok()
            }
            _ => None,
        };
        parsed
            .map(Num)
            .ok_or(ArgConversionError::UnexpectedArgumentType {
                got: v.value_type(),
                expected: &[ValueType::Integer, ValueType::Float],
            })
    }
}

mod money {
    use super::*;

    /// Fixed, because a builtin has one arity: a zero-decimal currency needs
    /// optional arguments first.
    const SCALE: i64 = 100;

    /// Mirrors oxyscripay's `minor_to_major`: whole amounts stay integers
    /// because some gateways reject `100.0` where they accept `100`.
    pub fn minor_to_major(_: Ctx, v: Num) -> Out {
        let minor = match v.0 {
            Number::Int(n) => n,
            Number::Float(n) if n.fract() == 0.0 => n as i64,
            Number::Float(n) => {
                return Err(external(format!(
                    "minor units must be a whole number, got {n}"
                )))
            }
        };
        Ok(if minor % SCALE == 0 {
            Value::from(minor / SCALE)
        } else {
            Value::from(minor as f64 / SCALE as f64)
        })
    }

    pub fn major_to_minor(_: Ctx, v: Num) -> Out {
        let minor = match v.0 {
            Number::Int(n) => n.saturating_mul(SCALE),
            Number::Float(n) => {
                let scaled = (n * SCALE as f64).round();
                if !scaled.is_finite() {
                    return Err(external("amount is not finite"));
                }
                scaled as i64
            }
        };
        Ok(Value::from(minor.max(0)))
    }
}

mod encoding {
    use super::*;

    pub fn base64(_: Ctx, v: String) -> Out {
        Ok(base64::engine::general_purpose::STANDARD.encode(v).into())
    }

    pub fn base64url(_: Ctx, v: String) -> Out {
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(v)
            .into())
    }

    pub fn hex(_: Ctx, v: String) -> Out {
        Ok(hex::encode(v).into())
    }
}

mod digests {
    use super::*;

    pub fn md5(_: Ctx, v: String) -> Out {
        use md5::{Digest, Md5};
        Ok(hex::encode(Md5::digest(v)).into())
    }

    pub fn sha256(_: Ctx, v: String) -> Out {
        use sha2::{Digest, Sha256};
        Ok(hex::encode(Sha256::digest(v)).into())
    }

    pub fn sha512(_: Ctx, v: String) -> Out {
        use sha2::{Digest, Sha512};
        Ok(hex::encode(Sha512::digest(v)).into())
    }

    pub fn hmac_sha256(_: Ctx, v: String, key: String) -> Out {
        use hmac::{Hmac, KeyInit, Mac};
        let mut mac = <Hmac<sha2::Sha256>>::new_from_slice(key.as_bytes()).map_err(bad_key)?;
        mac.update(v.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()).into())
    }

    pub fn hmac_sha512(_: Ctx, v: String, key: String) -> Out {
        use hmac::{Hmac, KeyInit, Mac};
        let mut mac = <Hmac<sha2::Sha512>>::new_from_slice(key.as_bytes()).map_err(bad_key)?;
        mac.update(v.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()).into())
    }

    fn bad_key(e: impl std::fmt::Display) -> RuntimeError {
        external(format!("invalid hmac key: {e}"))
    }
}

mod generic {
    use super::*;

    pub fn to_number(_: Ctx, v: Num) -> Out {
        Ok(Value::Number(v.0))
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
            .to_string()
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
    fn a_full_name_drops_missing_parts() {
        assert_eq!(
            ev("['Satoru', 'Gojo'] | join(' ')"),
            Some(json!("Satoru Gojo"))
        );
        assert_eq!(ev("[nope, 'Gojo'] | join(' ') | trim"), Some(json!("Gojo")));
        assert_eq!(
            ev("['Satoru', nope, 'Gojo'] | filter(|p| p) | join(' ')"),
            Some(json!("Satoru Gojo"))
        );
        assert_eq!(ev("['', ''] | join(' ') | trim"), None);
    }

    #[test]
    fn blank_input_short_circuits() {
        assert_eq!(ev("nope | trim"), None);
        assert_eq!(ev("'' | to_uppercase"), None);
        assert_eq!(ev("'  ' | trim | to_uppercase"), None);
        assert_eq!(ev("nope ?? 'x'"), Some(json!("x")));
        assert_eq!(ev("'' || 'x'"), Some(json!("x")));
    }

    #[test]
    fn void_if_strips_sentinels() {
        assert_eq!(ev("'_blank_' | void_if('_blank_')"), None);
        assert_eq!(ev("'Mpesa' | void_if('_blank_')"), Some(json!("Mpesa")));
        assert_eq!(
            ev("('_blank_' | void_if('_blank_')) ?? 'fallback'"),
            Some(json!("fallback"))
        );
    }

    #[test]
    fn a_status_table_is_an_object_index() {
        const T: &str = r#"{"Success": "approved", "Failed": "declined"}"#;
        assert_eq!(
            ev(&format!("{T}['Success'] ?? 'pending'")),
            Some(json!("approved"))
        );
        assert_eq!(
            ev(&format!("{T}['Weird'] ?? 'pending'")),
            Some(json!("pending"))
        );
        assert_eq!(
            ev(&format!("{T}[nope] ?? 'pending'")),
            Some(json!("pending"))
        );
        assert_eq!(ev(&format!("{T}['Weird']")), None);
    }

    #[test]
    fn map_is_the_std_array_map() {
        assert_eq!(ev("[1, 2] | map(|n| n * 2)"), Some(json!([2, 4])));
    }

    #[test]
    fn string_helpers() {
        assert_eq!(ev("'abcdef' | slice(2)"), Some(json!("cdef")));
        assert_eq!(ev("'abc' | slice(10)"), None);
        assert_eq!(ev("'a-b-c' | replace_all('-', '')"), Some(json!("abc")));
        assert_eq!(ev("7 | to_s | pad_start(3, '0')"), Some(json!("007")));
        assert_eq!(ev("'a,b' | split(',')"), Some(json!(["a", "b"])));
        assert!(err("'a' | pad_start(-1, '0')").contains("invalid function argument in pos 2"));
    }

    #[test]
    fn numbers_reach_text_builtins_through_to_s() {
        assert_eq!(ev("1000 | to_s"), Some(json!("1000")));
        assert_eq!(ev("'10' | to_number"), Some(json!(10)));
        assert_eq!(ev("'42' | sha256"), ev("42 | to_s | sha256"));
        assert!(err("42 | sha256").contains("got integer"));
        assert_eq!(ev("10.5 | to_s"), Some(json!("10.5")));
        assert_eq!(ev("'10.5' | to_number"), Some(json!(10.5)));
    }

    #[test]
    fn arrays_and_objects_are_not_scalars() {
        assert!(err("[1, 2] | sha256").contains("got array"));
        assert!(err(r#"{"a": 1} | base64"#).contains("got object"));
        assert!(err("'x' | to_number").contains("got str"));
    }

    #[test]
    fn a_flag_setting_picks_a_branch() {
        const E: &str = "(settings.sandbox | to_s | to_lowercase) == 'true' ? 'sandbox' : 'live'";
        let at = |sandbox| {
            Expr::parse(E)
                .unwrap()
                .eval(&json!({"settings": {"sandbox": sandbox}}))
                .unwrap()
        };
        assert_eq!(at(json!(true)), Some(json!("sandbox")));
        assert_eq!(at(json!("TRUE")), Some(json!("sandbox")));
        assert_eq!(at(json!("false")), Some(json!("live")));
        assert_eq!(at(json!(null)), Some(json!("live")));
    }
}
