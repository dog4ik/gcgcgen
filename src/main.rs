#![recursion_limit = "512"]

#[cfg(feature = "ssr")]
fn database_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://gcgcgen.db".to_string())
}

/// `gcgcgen --import a.json b.json` stores documents and exits, so fixtures can
/// be seeded without clicking through the editor. Runs before the Leptos
/// configuration is read, since importing needs none of it.
#[cfg(feature = "ssr")]
async fn import(files: &[String]) -> std::process::ExitCode {
    use gcgcgen::db;

    let pool = match db::connect(&database_url()).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("could not open {}: {e}", database_url());
            return std::process::ExitCode::FAILURE;
        }
    };
    let repo = db::Repo::new(pool);

    for file in files {
        let text = match std::fs::read_to_string(file) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{file}: {e}");
                return std::process::ExitCode::FAILURE;
            }
        };
        let doc: gcgcgen::spec::Integration = match serde_json::from_str(&text) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{file} is not an integration document: {e}");
                return std::process::ExitCode::FAILURE;
            }
        };
        match repo
            .save(&doc, Some(&format!("imported from {file}")))
            .await
        {
            Ok(v) => println!("imported `{}` as v{v}", doc.key),
            Err(e) => {
                eprintln!("{file}: {e}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() -> std::process::ExitCode {
    use std::sync::Arc;

    use axum::Router;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};

    use gcgcgen::app::{shell, App};
    use gcgcgen::db;
    use gcgcgen::engine::EngineCx;
    use gcgcgen::state::{AppState, Config};

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,gcgcgen=debug".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((flag, files)) if flag == "--import" && !files.is_empty() => {
            return import(files).await
        }
        Some((flag, _)) => {
            eprintln!("usage: gcgcgen [--import <file>...]\nunrecognised argument `{flag}`");
            return std::process::ExitCode::FAILURE;
        }
        None => {}
    }

    let conf = get_configuration(None).expect("leptos configuration");
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

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!("gcgcgen/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client");

    let state = AppState {
        leptos_options: leptos_options.clone(),
        repo: db::Repo::new(pool),
        engine: Arc::new(EngineCx::new(client)),
        config: Arc::new(Config { callback_base }),
    };

    let app = Router::new()
        // The reactivepay-facing surface.
        .nest("/gw", gcgcgen::api::router())
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
        .fallback(leptos_axum::file_and_error_handler::<AppState, _>(shell))
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
