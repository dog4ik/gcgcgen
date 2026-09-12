//! Evaluation of a parsed [`Node`] against a scope.
//!
//! The scope is a JSON object whose top-level keys are the roots (`payment`,
//! `params`, `settings`, `steps`, `env`, `method`, and inside a response spec
//! `resp`). Missing roots and missing path segments both evaluate to `null`
//! rather than erroring — that is what makes `??` chains readable, and it
//! keeps the editor's live preview usable while a path is half-typed.

use serde_json::Value;

use super::func::{self, is_absent};
use super::parse::{Node, Seg};
use super::EvalError;

pub fn eval(node: &Node, scope: &Value) -> Result<Value, EvalError> {
    match node {
        Node::Literal(v) => Ok(v.clone()),

        Node::Path { root, segs, .. } => Ok(resolve(scope, root, segs)),

        Node::Array(items) => items
            .iter()
            .map(|n| eval(n, scope))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),

        Node::Object(fields) => {
            let mut out = serde_json::Map::with_capacity(fields.len());
            for (k, n) in fields {
                out.insert(k.clone(), eval(n, scope)?);
            }
            Ok(Value::Object(out))
        }

        Node::Coalesce(alts) => {
            for alt in alts {
                let v = eval(alt, scope)?;
                if !is_absent(&v) {
                    return Ok(v);
                }
            }
            Ok(Value::Null)
        }

        Node::Pipe { input, calls } => {
            let mut acc = eval(input, scope)?;
            for call in calls {
                let spec = func::lookup(&call.name)
                    .ok_or_else(|| EvalError::new(unknown_function(&call.name), Some(call.span)))?;

                if !(spec.min_args..=spec.max_args).contains(&call.args.len()) {
                    return Err(EvalError::new(
                        arity_message(spec, call.args.len()),
                        Some(call.span),
                    ));
                }

                // Absence flows through a pipeline untouched, so `null | trim`
                // stays null instead of collapsing to "".
                if spec.skip_absent && is_absent(&acc) {
                    acc = Value::Null;
                    continue;
                }

                let args = call
                    .args
                    .iter()
                    .map(|a| eval(a, scope))
                    .collect::<Result<Vec<_>, _>>()?;

                acc = (spec.f)(acc, &args)
                    .map_err(|m| EvalError::new(format!("{}: {m}", call.name), Some(call.span)))?;
            }
            Ok(acc)
        }
    }
}

fn resolve(scope: &Value, root: &str, segs: &[Seg]) -> Value {
    let mut cur = match scope.get(root) {
        Some(v) => v,
        None => return Value::Null,
    };
    for seg in segs {
        let next = match seg {
            Seg::Key(k) => cur.get(k),
            Seg::Index(i) => cur.get(i),
        };
        match next {
            Some(v) => cur = v,
            None => return Value::Null,
        }
    }
    cur.clone()
}

fn arity_message(spec: &func::FnSpec, got: usize) -> String {
    let want = if spec.min_args == spec.max_args {
        format!("{}", spec.min_args)
    } else {
        format!("{}..={}", spec.min_args, spec.max_args)
    };
    format!("`{}` takes {want} argument(s), got {got}", spec.name)
}

fn unknown_function(name: &str) -> String {
    let mut best: Option<(usize, &str)> = None;
    for candidate in func::names() {
        let d = edit_distance(name, candidate);
        if d <= 2 && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, candidate));
        }
    }
    match best {
        Some((_, hint)) => format!("unknown function `{name}` — did you mean `{hint}`?"),
        None => format!("unknown function `{name}`"),
    }
}

/// Levenshtein, for "did you mean" hints on typo'd builtin names.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Convenience for callers that already have a source string.
pub fn eval_str(src: &str, scope: &Value) -> Result<Value, EvalError> {
    let node = super::parse::parse(src).map_err(EvalError::from_parse)?;
    eval(&node, scope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::expr::Span;
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

    fn ev(src: &str) -> Value {
        eval_str(src, &scope()).unwrap_or_else(|e| panic!("{src}: {e}"))
    }

    #[test]
    fn reads_nested_paths() {
        assert_eq!(ev("payment.token"), json!("tok_1"));
        assert_eq!(ev("steps.auth.access_token"), json!("abc"));
        assert_eq!(
            ev("env.callback_url"),
            json!("https://cb.example/gateway/callback")
        );
    }

    #[test]
    fn missing_paths_and_roots_are_null() {
        assert_eq!(ev("payment.nope"), Value::Null);
        assert_eq!(ev("payment.nope.deeper"), Value::Null);
        assert_eq!(ev("nosuchroot.x"), Value::Null);
        assert_eq!(ev("params.customer[3]"), Value::Null);
    }

    #[test]
    fn coalesce_skips_null_and_empty() {
        assert_eq!(
            ev("payment.product ?? payment.order_number ?? 'Payment'"),
            json!("ORD-9")
        );
        assert_eq!(
            ev("payment.product ?? payment.nope ?? 'Payment'"),
            json!("Payment")
        );
        assert_eq!(
            ev("params.phone ?? params.customer.phone"),
            json!("0798288410")
        );
    }

    #[test]
    fn reproduces_the_scripay_channel_rule() {
        // extra_return_param is the sentinel, so the merchant default wins.
        assert_eq!(
            ev("params.extra_return_param | null_if('_blank_') ?? settings.channel"),
            json!("Mpesa")
        );
    }

    #[test]
    fn reproduces_the_scripay_account_name_rule() {
        assert_eq!(
            ev("[params.first_name, params.last_name] | join(' ') | trim"),
            json!("John Doe")
        );
    }

    #[test]
    fn amount_conversion_stays_typed() {
        assert_eq!(ev("payment.gateway_amount | minor_to_major"), json!(10));
    }

    #[test]
    fn absence_flows_through_a_pipeline() {
        assert_eq!(ev("payment.product | trim | upper"), Value::Null);
        assert_eq!(
            ev("payment.product | trim ?? 'fallback'"),
            json!("fallback")
        );
    }

    #[test]
    fn unknown_function_suggests_a_name() {
        let err = eval_str("payment.token | trm", &scope()).unwrap_err();
        assert!(
            err.message.contains("did you mean `trim`"),
            "{}",
            err.message
        );
        assert!(err.span.is_some());
    }

    #[test]
    fn arity_is_checked_before_evaluation() {
        let err = eval_str("payment.token | null_if", &scope()).unwrap_err();
        assert!(err.message.contains("takes 1 argument"), "{}", err.message);
    }

    #[test]
    fn function_errors_carry_the_call_span() {
        let err = eval_str("params.customer | trim", &scope()).unwrap_err();
        assert!(err.message.starts_with("trim:"), "{}", err.message);
        assert_eq!(err.span, Some(Span::new(18, 22)));
    }

    #[test]
    fn object_and_array_literals_evaluate() {
        assert_eq!(
            ev("{a: payment.token, b: 2}"),
            json!({"a": "tok_1", "b": 2})
        );
        assert_eq!(ev("[1, payment.token]"), json!([1, "tok_1"]));
    }

    #[test]
    fn map_lookup_over_a_step_output() {
        let scope = json!({ "resp": { "body": { "status": "Completed" } } });
        assert_eq!(
            eval_str(
                "resp.body.status | map({Success:'approved', Completed:'approved', _default:'pending'})",
                &scope
            )
            .unwrap(),
            json!("approved")
        );
    }
}
