use serde::{Deserialize, Serialize};

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
    pub callback_defined: bool,
}
