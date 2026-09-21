use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::error::{EngineError, Result};
use super::http::Prepared;
use super::scope::Scope;
use crate::spec::auth::{SigAlg, SigEncoding, SigPlacement, SignatureAuth, TokenPlacement};
use crate::spec::{AuthId, AuthKind};

/// Cached gateway tokens
#[derive(Debug, Default)]
pub struct TokenStore {
    inner: Mutex<HashMap<String, Entry>>,
}

#[derive(Debug, Clone)]
struct Entry {
    token: String,
    usable_until: Instant,
}

const MAX_ENTRIES_BEFORE_CLEANUP: usize = 128;

impl TokenStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cached(&self, key: &str) -> Option<String> {
        let map = self.inner.lock().expect("token store poisoned");
        let entry = map.get(key)?;
        (entry.usable_until > Instant::now()).then(|| entry.token.clone())
    }

    pub fn insert(&self, key: String, token: String, ttl: Duration, refresh_buffer: Duration) {
        let usable = ttl.saturating_sub(refresh_buffer);
        let mut map = self.inner.lock().expect("token store poisoned");
        if map.len() >= MAX_ENTRIES_BEFORE_CLEANUP {
            let now = Instant::now();
            map.retain(|_, e| e.usable_until > now);
        }
        map.insert(
            key,
            Entry {
                token,
                usable_until: Instant::now() + usable,
            },
        );
    }

    pub fn invalidate(&self, key: &str) {
        self.inner.lock().expect("token store poisoned").remove(key);
    }
}

/// Builds the cache key for one auth definition.
pub fn cache_key(integration_key: &str, auth: &AuthId, parts: &[Value]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(serde_json::to_vec(p).unwrap_or_default());
        h.update([0xff]);
    }
    format!("{integration_key}:{auth}:{}", hex::encode(h.finalize()))
}

pub fn place_token(prepared: &mut Prepared, placement: &TokenPlacement, token: &str) {
    match placement {
        TokenPlacement::Bearer => prepared.set_header("authorization", format!("Bearer {token}")),
        TokenPlacement::Header { name, prefix } => {
            let value = match prefix {
                Some(p) => format!("{p}{token}"),
                None => token.to_string(),
            };
            prepared.set_header(name, value)
        }
        TokenPlacement::Query { name } => prepared.set_query(name, token.to_string()),
    }
}

/// Applies every auth kind except [`AuthKind::TokenRequest`], which the
/// executor handles because it may need to send a request.
pub fn apply_inline(prepared: &mut Prepared, kind: &AuthKind, scope: &Scope) -> Result<()> {
    let value = scope.value();
    match kind {
        AuthKind::None | AuthKind::TokenRequest(_) => Ok(()),

        AuthKind::Bearer { token } => {
            if let Some(t) = token.eval_text(&value)? {
                prepared.set_header("authorization", format!("Bearer {t}"));
            }
            Ok(())
        }

        AuthKind::Basic { username, password } => {
            let u = username.eval_text(&value)?.unwrap_or_default();
            let p = password.eval_text(&value)?.unwrap_or_default();
            let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"));
            prepared.set_header("authorization", format!("Basic {encoded}"));
            Ok(())
        }

        AuthKind::Header { name, value: e } => {
            if let Some(v) = e.eval_text(&value)? {
                prepared.set_header(name, v);
            }
            Ok(())
        }

        AuthKind::Query { name, value: e } => {
            if let Some(v) = e.eval_text(&value)? {
                prepared.set_query(name, v);
            }
            Ok(())
        }

        AuthKind::Signature(sig) => apply_signature(prepared, sig, scope),
    }
}

/// Signs the *finished* request, so the canonical string can read the rendered
/// path, query and body via the `req` root.
pub fn apply_signature(prepared: &mut Prepared, sig: &SignatureAuth, scope: &Scope) -> Result<()> {
    let req_scope = scope.with_req(prepared.req_scope());
    let canonical = sig.canonical.eval_text(&req_scope)?.unwrap_or_default();

    let secret = match &sig.secret {
        Some(e) => e.eval_text(&req_scope)?.unwrap_or_default(),
        None => String::new(),
    };
    if sig.algorithm.needs_secret() && secret.is_empty() {
        return Err(EngineError::Config(format!(
            "{:?} signature has no secret — check the `secret` expression",
            sig.algorithm
        )));
    }

    let digest = compute(sig.algorithm, canonical.as_bytes(), secret.as_bytes())?;
    let encoded = match sig.encoding {
        SigEncoding::Hex => hex::encode(&digest),
        SigEncoding::Base64 => base64::engine::general_purpose::STANDARD.encode(&digest),
        SigEncoding::Base64Url => base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&digest),
    };

    match &sig.placement {
        SigPlacement::Header { name } => prepared.set_header(name, encoded),
        SigPlacement::Query { name } => prepared.set_query(name, encoded),
        SigPlacement::BodyField { path } => {
            prepared.set_body_field(path, Value::String(encoded))?
        }
    }
    Ok(())
}

fn compute(alg: SigAlg, message: &[u8], secret: &[u8]) -> Result<Vec<u8>> {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha512;

    Ok(match alg {
        SigAlg::Sha256 => Sha256::digest(message).to_vec(),
        SigAlg::Sha512 => Sha512::digest(message).to_vec(),
        SigAlg::Md5 => {
            use md5::Md5;
            Md5::digest(message).to_vec()
        }
        SigAlg::HmacSha256 => {
            let mut m = <Hmac<Sha256>>::new_from_slice(secret)
                .map_err(|e| EngineError::Config(format!("invalid hmac key: {e}")))?;
            m.update(message);
            m.finalize().into_bytes().to_vec()
        }
        SigAlg::HmacSha512 => {
            let mut m = <Hmac<Sha512>>::new_from_slice(secret)
                .map_err(|e| EngineError::Config(format!("invalid hmac key: {e}")))?;
            m.update(message);
            m.finalize().into_bytes().to_vec()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::http::{prepare, PreparedBody};
    use crate::engine::scope::{ConnectInput, Env};
    use crate::spec::{MethodKind, RequestDef};
    use serde_json::json;

    fn env() -> Env {
        Env {
            integration_key: "scripay".into(),
            base_url: "https://api.example.com".into(),
            callback_url: "https://cb/x".into(),
            sandbox: false,
            now_rfc3339: "2026-09-06T00:00:00Z".into(),
            unix_now: 1_788_000_000,
            request_id: "req-1".into(),
        }
    }

    fn scope() -> Scope {
        Scope::new(
            &ConnectInput {
                payment: json!({"token": "tok_1"}),
                params: json!({}),
                settings: json!({"user": "u", "pass": "p", "api_secret": "shhh", "tok": "T"}),
                ..Default::default()
            },
            &env(),
            MethodKind::Pay,
        )
    }

    fn prepared(src: &str) -> Prepared {
        let r: RequestDef = serde_json::from_str(src).unwrap();
        prepare(&r, "https://api.example.com", &scope().value()).unwrap()
    }

    #[test]
    fn token_store_honours_the_refresh_buffer() {
        let s = TokenStore::new();
        s.insert(
            "k".into(),
            "t".into(),
            Duration::from_secs(60),
            Duration::from_secs(10),
        );
        assert_eq!(s.cached("k").as_deref(), Some("t"));

        // A TTL entirely inside the buffer is never usable.
        s.insert(
            "k2".into(),
            "t2".into(),
            Duration::from_secs(5),
            Duration::from_secs(10),
        );
        assert_eq!(s.cached("k2"), None);
        assert_eq!(s.cached("missing"), None);
    }

    #[test]
    fn cache_keys_separate_merchants() {
        let a = cache_key("scripay", &AuthId::new("oauth"), &[json!("client_a")]);
        let b = cache_key("scripay", &AuthId::new("oauth"), &[json!("client_b")]);
        let c = cache_key("other", &AuthId::new("oauth"), &[json!("client_a")]);
        let d = cache_key("scripay", &AuthId::new("other_auth"), &[json!("client_a")]);
        assert_ne!(a, b, "different merchants must not share a token");
        assert_ne!(a, c, "different integrations must not share a token");
        assert_ne!(a, d, "different auth definitions must not share a token");
        assert_eq!(
            a,
            cache_key("scripay", &AuthId::new("oauth"), &[json!("client_a")])
        );
        // The raw credential never appears in the key.
        assert!(!a.contains("client_a"));
    }

    #[test]
    fn bearer_and_basic_set_authorization() {
        let mut p = prepared(r#"{"name":"c","path":"'/p'"}"#);
        apply_inline(
            &mut p,
            &AuthKind::Bearer {
                token: crate::spec::Expr::parse("settings.tok").unwrap(),
            },
            &scope(),
        )
        .unwrap();
        assert_eq!(p.headers, vec![("authorization".into(), "Bearer T".into())]);

        let mut p = prepared(r#"{"name":"c","path":"'/p'"}"#);
        apply_inline(
            &mut p,
            &AuthKind::Basic {
                username: crate::spec::Expr::parse("settings.user").unwrap(),
                password: crate::spec::Expr::parse("settings.pass").unwrap(),
            },
            &scope(),
        )
        .unwrap();
        // base64("u:p")
        assert_eq!(
            p.headers,
            vec![("authorization".into(), "Basic dTpw".into())]
        );
    }

    #[test]
    fn token_placements() {
        let mut p = prepared(r#"{"name":"c","path":"'/p'"}"#);
        place_token(&mut p, &TokenPlacement::Bearer, "T");
        assert_eq!(p.headers[0].1, "Bearer T");

        let mut p = prepared(r#"{"name":"c","path":"'/p'"}"#);
        place_token(
            &mut p,
            &TokenPlacement::Header {
                name: "X-Token".into(),
                prefix: Some("tok ".into()),
            },
            "T",
        );
        assert_eq!(p.headers[0], ("X-Token".to_string(), "tok T".to_string()));

        let mut p = prepared(r#"{"name":"c","path":"'/p'"}"#);
        place_token(
            &mut p,
            &TokenPlacement::Query {
                name: "access_token".into(),
            },
            "T",
        );
        assert_eq!(p.full_url(), "https://api.example.com/p?access_token=T");
    }

    #[test]
    fn signature_covers_the_rendered_request() {
        let sig: SignatureAuth = serde_json::from_str(
            r#"{"canonical": "concat([req.method, req.path, req.body_raw])",
                "algorithm": "hmac_sha256",
                "secret": "settings.api_secret",
                "encoding": "hex",
                "placement": {"kind": "header", "name": "X-Sig"}}"#,
        )
        .unwrap();
        let mut p =
            prepared(r#"{"name":"c","path":"'/p'","body":{"kind":"json","expr":"{\"a\": 1}"}}"#);
        apply_signature(&mut p, &sig, &scope()).unwrap();

        let expected = {
            use hmac::{Hmac, KeyInit, Mac};
            let mut m = <Hmac<Sha256>>::new_from_slice(b"shhh").unwrap();
            m.update(br#"POST/p{"a":1}"#);
            hex::encode(m.finalize().into_bytes())
        };
        assert_eq!(p.headers, vec![("X-Sig".to_string(), expected)]);
    }

    #[test]
    fn signature_can_land_in_the_body() {
        let sig: SignatureAuth = serde_json::from_str(
            r#"{"canonical": "req.path", "algorithm": "sha256",
                "encoding": "base64",
                "placement": {"kind": "body_field", "path": "auth.sig"}}"#,
        )
        .unwrap();
        let mut p =
            prepared(r#"{"name":"c","path":"'/p'","body":{"kind":"json","expr":"{\"a\": 1}"}}"#);
        apply_signature(&mut p, &sig, &scope()).unwrap();
        let PreparedBody::Json(body) = &p.body else {
            panic!("expected json")
        };
        assert!(body["auth"]["sig"].is_string());
    }

    #[test]
    fn hmac_without_a_resolvable_secret_is_a_config_error() {
        let sig: SignatureAuth = serde_json::from_str(
            r#"{"canonical": "x", "algorithm": "hmac_sha256",
                "secret": "settings.missing",
                "placement": {"kind": "header", "name": "X-Sig"}}"#,
        )
        .unwrap();
        let mut p = prepared(r#"{"name":"c","path":"'/p'"}"#);
        let err = apply_signature(&mut p, &sig, &scope()).unwrap_err();
        assert!(!err.is_uncertain(), "a pre-send config error is certain");
        assert!(err.to_string().contains("no secret"), "{err}");
    }
}
