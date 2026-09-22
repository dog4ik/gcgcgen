use std::sync::Arc;

use axum::extract::FromRef;
use leptos::prelude::LeptosOptions;

use crate::db::Repo;
use crate::engine::platform::PlatformConfig;
use crate::engine::EngineCx;

#[derive(Debug, Clone)]
pub struct Config {
    pub callback_base: String,
    pub platform: Option<PlatformConfig>,
}

impl Config {
    pub fn callback_url(&self, integration_key: &str) -> String {
        format!(
            "{}/gw/{integration_key}/callback",
            self.callback_base.trim_end_matches('/')
        )
    }
}

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
            platform: None,
        };
        assert_eq!(
            c.callback_url("scripay"),
            "https://gw.example.com/gw/scripay/callback"
        );
    }
}
