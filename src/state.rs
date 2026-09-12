//! Shared server state.

use std::sync::Arc;

use axum::extract::FromRef;
use leptos::prelude::LeptosOptions;

use crate::db::Repo;
use crate::engine::EngineCx;

#[derive(Debug, Clone)]
pub struct Config {
    /// Public origin the gateway should call back on, e.g.
    /// `https://gw.example.com`. Combined with the integration key to form
    /// `env.callback_url`.
    pub callback_base: String,
}

impl Config {
    pub fn callback_url(&self, integration_key: &str) -> String {
        format!(
            "{}/gw/{integration_key}/callback",
            self.callback_base.trim_end_matches('/')
        )
    }
}

/// `LeptosOptions` is pulled out by `FromRef` so `leptos_routes` keeps working
/// while the router also carries the repo and the engine.
#[derive(Clone, FromRef)]
pub struct AppState {
    pub leptos_options: LeptosOptions,
    pub repo: Repo,
    pub engine: Arc<EngineCx>,
    pub config: Arc<Config>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_urls_are_per_integration_and_slash_tolerant() {
        let c = Config {
            callback_base: "https://gw.example.com/".into(),
        };
        assert_eq!(
            c.callback_url("scripay"),
            "https://gw.example.com/gw/scripay/callback"
        );
    }
}
