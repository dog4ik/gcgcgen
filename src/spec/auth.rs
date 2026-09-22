use serde::{Deserialize, Serialize};

use super::expr::Expr;
use super::request::RequestDef;

/// Reference to an [`AuthDef`] by its `id`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthId(pub String);

impl AuthId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AuthId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthDef {
    pub id: AuthId,
    #[serde(flatten)]
    pub kind: AuthKind,
}

impl AuthDef {
    pub fn display(&self) -> &str {
        self.id.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthKind {
    None,
    Bearer {
        token: Expr,
    },
    Basic {
        username: Expr,
        password: Expr,
    },
    /// A fixed header (`X-Api-Key: settings.api_key`).
    Header {
        name: String,
        value: Expr,
    },
    Query {
        name: String,
        value: Expr,
    },
    Signature(Box<SignatureAuth>),
    TokenRequest(Box<TokenRequestAuth>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureAuth {
    pub canonical: Expr,
    pub algorithm: SigAlg,
    /// Ignored by the unkeyed digests (`sha256`, `md5`).
    #[serde(default)]
    pub secret: Option<Expr>,
    #[serde(default)]
    pub encoding: SigEncoding,
    pub placement: SigPlacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SigAlg {
    HmacSha256,
    HmacSha512,
    Sha256,
    Sha512,
    Md5,
}

impl SigAlg {
    pub fn needs_secret(self) -> bool {
        matches!(self, SigAlg::HmacSha256 | SigAlg::HmacSha512)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SigEncoding {
    #[default]
    Hex,
    Base64,
    Base64Url,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SigPlacement {
    Header {
        name: String,
    },
    Query {
        name: String,
    },
    /// Dotted path into the rendered JSON body, e.g. `auth.signature`.
    BodyField {
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenRequestAuth {
    pub request: RequestDef,
    pub token: Expr,
    #[serde(default)]
    pub expires_in: Option<Expr>,
    #[serde(default = "default_ttl")]
    pub default_ttl_secs: u64,
    #[serde(default = "default_refresh_buffer")]
    pub refresh_buffer_secs: u64,
    /// Settings-derived values that partition the cache. Without one, two
    /// merchants on the same integration would share a token
    pub cache_key: Vec<Expr>,
    pub placement: TokenPlacement,
}

fn default_ttl() -> u64 {
    3600
}

fn default_refresh_buffer() -> u64 {
    300
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TokenPlacement {
    Bearer,
    Header {
        name: String,
        #[serde(default)]
        prefix: Option<String>,
    },
    Query {
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPAY_AUTH: &str = r#"{
        "id": "oauth",
        "kind": "token_request",
        "request": {
            "name": "auth",
            "method": "post",
            "path": "'/v1/auth/access-token'",
            "body": {
                "kind": "json",
                "expr": "{\"client_id\": settings.client_id, \"client_secret\": settings.client_secret}"
            }
        },
        "token": "resp.body.access_token",
        "expires_in": "resp.body.expires_in",
        "cache_key": ["settings.client_id"],
        "placement": { "kind": "bearer" }
    }"#;

    #[test]
    fn parses_the_scripay_token_request() {
        let def: AuthDef = serde_json::from_str(SCRIPAY_AUTH).unwrap();
        assert_eq!(def.id, AuthId::new("oauth"));
        let AuthKind::TokenRequest(tr) = &def.kind else {
            panic!("expected a token request")
        };
        assert_eq!(tr.token.src(), "resp.body.access_token");
        assert_eq!(tr.default_ttl_secs, 3600);
        assert_eq!(tr.refresh_buffer_secs, 300);
        assert_eq!(tr.placement, TokenPlacement::Bearer);
        assert_eq!(tr.request.name, "auth");
    }

    #[test]
    fn round_trips() {
        let def: AuthDef = serde_json::from_str(SCRIPAY_AUTH).unwrap();
        let back: AuthDef = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
        assert_eq!(def, back);
    }

    #[test]
    fn parses_signature_auth() {
        let def: AuthDef = serde_json::from_str(
            r#"{
                "id": "sig",
                "kind": "signature",
                "canonical": "concat([req.method, req.path, env.unix_now])",
                "algorithm": "hmac_sha256",
                "secret": "settings.api_secret",
                "encoding": "hex",
                "placement": { "kind": "header", "name": "X-Signature" }
            }"#,
        )
        .unwrap();
        let AuthKind::Signature(sig) = &def.kind else {
            panic!("expected a signature")
        };
        assert_eq!(sig.algorithm, SigAlg::HmacSha256);
        assert!(sig.algorithm.needs_secret());
        assert_eq!(sig.encoding, SigEncoding::Hex);
    }
}
