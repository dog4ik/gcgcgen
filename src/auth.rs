use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::headers::authorization::Basic;
use axum_extra::headers::Authorization;
use axum_extra::typed_header::TypedHeaderRejection;
use axum_extra::TypedHeader;

#[derive(Clone)]
pub struct BasicAuth {
    user: String,
    password: String,
}

impl std::fmt::Debug for BasicAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BasicAuth")
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl BasicAuth {
    pub fn from_env() -> Option<Self> {
        let password = std::env::var("API_PASSWORD")
            .ok()
            .filter(|p| !p.is_empty())?;
        Some(Self {
            user: std::env::var("API_USER").unwrap_or_else(|_| "admin".to_string()),
            password,
        })
    }

    fn matches(&self, offered: &Basic) -> bool {
        // Both halves are always compared, so a wrong username and a wrong
        // password take the same time to reject.
        let user = constant_time_eq(self.user.as_bytes(), offered.username().as_bytes());
        let password = constant_time_eq(self.password.as_bytes(), offered.password().as_bytes());
        user && password
    }
}

/// Non-short-circuiting comparison. The lengths still leak, which is what every
/// other basic-auth check leaks too.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub async fn require_basic_auth(
    State(expected): State<Arc<BasicAuth>>,
    auth: Result<TypedHeader<Authorization<Basic>>, TypedHeaderRejection>,
    request: Request,
    next: Next,
) -> Response {
    match auth {
        Ok(TypedHeader(Authorization(offered))) if expected.matches(&offered) => {
            next.run(request).await
        }
        Ok(_) => {
            tracing::warn!(path = %request.uri().path(), "rejected bad credentials");
            challenge()
        }
        Err(_) => challenge(),
    }
}

fn challenge() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            r#"Basic realm="gcgcgen", charset="UTF-8""#,
        )],
    )
        .into_response()
}
