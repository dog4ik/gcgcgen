//! Server functions backing the editor UI.
//!
//! Validation is *not* here: [`crate::spec::validate`] is pure and compiles to
//! wasm, so the editor checks a document as it is typed without a round trip.
//! These calls are the ones that genuinely need the server — storage, and the
//! dry run that needs `env` to be filled in the same way a real call would.

use leptos::prelude::*;

use connect::{ConnectInput, MethodKind};

use crate::model::{IntegrationSummary, PreviewOutput, VersionInfo};
use crate::spec::Integration;

#[cfg(feature = "ssr")]
fn state() -> Result<crate::state::AppState, ServerFnError> {
    use_context::<crate::state::AppState>()
        .ok_or_else(|| ServerFnError::new("server state is missing from the request context"))
}

#[server]
pub async fn list_integrations() -> Result<Vec<IntegrationSummary>, ServerFnError> {
    state()?
        .repo
        .list()
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn load_integration(key: String) -> Result<Integration, ServerFnError> {
    state()?
        .repo
        .load(&key)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Validates and stores, returning the new version number.
#[server]
pub async fn save_integration(
    doc: Integration,
    note: Option<String>,
) -> Result<i64, ServerFnError> {
    state()?
        .repo
        .save(&doc, note.as_deref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn delete_integration(key: String) -> Result<(), ServerFnError> {
    state()?
        .repo
        .delete(&key)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn list_versions(key: String) -> Result<Vec<VersionInfo>, ServerFnError> {
    state()?
        .repo
        .versions(&key)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn rollback_integration(key: String, version: i64) -> Result<i64, ServerFnError> {
    state()?
        .repo
        .rollback(&key, version)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Renders one request against sample input. Nothing is sent: this is the
/// same code path the golden mapping tests assert against.
#[server]
pub async fn preview_request(
    doc: Integration,
    method: MethodKind,
    request: String,
    input: ConnectInput,
    steps: serde_json::Value,
) -> Result<PreviewOutput, ServerFnError> {
    use crate::engine::http::PreparedBody;
    use crate::engine::Runtime;

    let steps = match steps {
        serde_json::Value::Object(m) => m,
        serde_json::Value::Null => serde_json::Map::new(),
        _ => return Err(ServerFnError::new("`steps` must be an object")),
    };

    let runtime = Runtime {
        callback_url: state()?.config.callback_url(&doc.key),
        request_id: "preview".into(),
    };

    let prepared = crate::engine::preview_request(&doc, method, &request, &input, &runtime, &steps)
        .map_err(ServerFnError::new)?;

    Ok(PreviewOutput {
        method: prepared.method.as_str().to_string(),
        url: prepared.full_url(),
        headers: prepared.headers.clone(),
        body: match &prepared.body {
            PreparedBody::None => None,
            _ => Some(prepared.body_value()),
        },
        body_raw: prepared.body_raw(),
    })
}
