#![recursion_limit = "512"]
#[cfg(feature = "ssr")]
pub mod api;
pub mod app;
#[cfg(feature = "ssr")]
pub mod auth;
#[cfg(feature = "ssr")]
pub mod db;
#[cfg(feature = "ssr")]
pub mod engine;
pub mod model;
pub mod server_fns;
pub mod spec;
#[cfg(feature = "ssr")]
pub mod state;
pub mod ui;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::App;
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
