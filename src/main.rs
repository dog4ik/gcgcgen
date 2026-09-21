#![recursion_limit = "512"]

#[cfg(feature = "ssr")]
fn database_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://gcgcgen.db".to_string())
}

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() -> std::process::ExitCode {
    use std::sync::Arc;

    use axum::Router;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};

    use gcgcgen::app::{shell, App};
    use gcgcgen::auth::{require_basic_auth, BasicAuth};
    use gcgcgen::db;
    use gcgcgen::engine::context::ContextStore;
    use gcgcgen::engine::platform::PlatformConfig;
    use gcgcgen::engine::EngineCx;
    use gcgcgen::state::{AppState, Config};

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,gcgcgen=debug".into()),
        )
        .init();

    let conf = get_configuration(Some("./Cargo.toml")).expect("leptos configuration");
    let leptos_options = conf.leptos_options;
    let addr = leptos_options.site_addr;
    let routes = generate_route_list(App);

    let database_url = database_url();
    let pool = db::connect(&database_url)
        .await
        .unwrap_or_else(|e| panic!("could not open {database_url}: {e}"));
    tracing::info!(%database_url, "storage ready");

    // Where gateways are told to send callbacks. Defaults to the local site so
    // a dev run is self-consistent; set it to the public origin in deployment.
    let callback_base = std::env::var("CALLBACK_BASE").unwrap_or_else(|_| format!("http://{addr}"));

    let business_url =
        std::env::var("BUSINESS_URL").unwrap_or_else(|_| "http://business:4000".to_string());
    let platform = match std::env::var("SIGN_KEY").map(|k| <[u8; 32]>::try_from(k.into_bytes())) {
        Ok(Ok(sign_key)) => Some(PlatformConfig {
            business_url,
            sign_key,
        }),
        Ok(Err(_)) => panic!("SIGN_KEY must be exactly 32 bytes"),
        Err(_) => {
            tracing::warn!("SIGN_KEY is not set; gateway callbacks cannot be forwarded");
            None
        }
    };
    let context_ttl = std::env::var("CALLBACK_CONTEXT_TTL_HOURS")
        .ok()
        .and_then(|h| h.parse::<u64>().ok())
        .unwrap_or(24);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!("gcgcgen/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client");

    let state = AppState {
        leptos_options: leptos_options.clone(),
        repo: db::Repo::new(pool),
        engine: Arc::new(EngineCx::new(client).with_contexts(ContextStore::new(
            std::time::Duration::from_secs(context_ttl * 60 * 60),
        ))),
        config: Arc::new(Config {
            callback_base,
            platform,
        }),
    };

    let mut editor = Router::new()
        // Server functions need the same state the pages get.
        .route(
            "/api/{*fn_name}",
            axum::routing::any({
                let state = state.clone();
                move |req| {
                    let state = state.clone();
                    leptos_axum::handle_server_fns_with_context(
                        move || provide_context(state.clone()),
                        req,
                    )
                }
            }),
        )
        .leptos_routes_with_context(
            &state,
            routes,
            {
                let state = state.clone();
                move || provide_context(state.clone())
            },
            {
                let leptos_options = leptos_options.clone();
                move || shell(leptos_options.clone())
            },
        )
        .fallback(leptos_axum::file_and_error_handler::<AppState, _>(shell));

    match BasicAuth::from_env() {
        Some(credentials) => {
            editor = editor.layer(axum::middleware::from_fn_with_state(
                Arc::new(credentials),
                require_basic_auth,
            ));
        }
        None => {
            tracing::warn!("API_PASSWORD is not set; the editor is open to anyone who can reach it")
        }
    }

    let app = Router::new()
        .nest("/gw", gcgcgen::api::router())
        .merge(editor)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    tracing::info!("listening on http://{addr}");
    axum::serve(listener, app.into_make_service())
        .await
        .expect("serve");
    std::process::ExitCode::SUCCESS
}

#[cfg(not(feature = "ssr"))]
pub fn main() {
    // The client entry point is `hydrate()` in lib.rs.
}
