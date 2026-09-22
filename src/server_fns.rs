use leptos::prelude::*;
use leptos::server_fn::codec::Json;

use crate::model::IntegrationSummary;
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
#[server(input = Json)]
pub async fn save_integration(doc: Integration) -> Result<i64, ServerFnError> {
    state()?
        .repo
        .save(&doc)
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
