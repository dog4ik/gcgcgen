//! DTOs shared between the server and the browser: they compile for both
//! `ssr` and `hydrate`, so no database or HTTP types here.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use connect::MethodKind;

/// Enough to render the integration list without loading every document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntegrationSummary {
    pub key: String,
    pub name: String,
    pub version: i64,
    pub updated_at: String,
    /// Methods with at least one request, in canonical order.
    pub methods: Vec<MethodKind>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: i64,
    pub created_at: String,
    pub note: Option<String>,
}

/// What the dry-run panel shows: the request as it would go out, with nothing
/// sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewOutput {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// Present for JSON and form bodies.
    pub body: Option<Value>,
    pub body_raw: String,
}
