mod config;

use std::net::SocketAddr;

use axum::http::{HeaderName, HeaderValue};
use axum::routing::get;
use axum::Router;
use clap::Parser;
use leptos::prelude::*;
use leptos_axum::{generate_route_list, LeptosRoutes};
use tower_http::set_header::SetResponseHeaderLayer;

use ironpad_app::*;
use ironpad_common::AppConfig;
use ironpad_server::state::{AppState, WsState};
use ironpad_server::ws;

use crate::config::CliArgs;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = CliArgs::parse();
    let config: AppConfig = args.into();

    tracing::info!(data_dir = %config.data_dir.display(), "data directory");
    tracing::info!(cache_dir = %config.cache_dir.display(), "cache directory");
    tracing::info!(ironpad_cell_path = %config.ironpad_cell_path.display(), "ironpad-cell crate path");

    let conf = get_configuration(None).expect("leptos configuration");
    let leptos_options = conf.leptos_options;

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));

    let app_state = AppState {
        leptos_options: leptos_options.clone(),
        config: config.clone(),
        ws: WsState::default(),
    };

    // Pre-warm the toolchain fingerprint / wasm-bindgen CLI version caches on a
    // blocking thread so the first cell compile doesn't block a tokio worker
    // thread on `wasm-bindgen --version` / `rustc --version` shell-outs.
    tokio::task::spawn_blocking(ironpad_app::compiler::toolchain::prewarm)
        .await
        .ok();

    // Thaw's `ToasterProvider` creates an effect that calls `spawn_local` during
    // route generation. Entering a `LocalSet` gives `spawn_local` a valid context;
    // the spawned tasks are never driven since we only need the route list.
    let local = tokio::task::LocalSet::new();
    let _guard = local.enter();
    let routes = generate_route_list(App);

    let app = Router::new()
        .route("/ws/host", get(ws::ws_host_handler))
        .route("/ws/connect", get(ws::ws_connect_handler))
        .leptos_routes_with_context(
            &app_state,
            routes,
            {
                let config = config.clone();
                let leptos_options = leptos_options.clone();
                move || {
                    provide_context(config.clone());
                    provide_context(leptos_options.clone());
                }
            },
            {
                let leptos_options = leptos_options.clone();
                move || shell(leptos_options.clone())
            },
        )
        .fallback(leptos_axum::file_and_error_handler::<AppState, _>(shell))
        .with_state(app_state)
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-embedder-policy"),
            HeaderValue::from_static("require-corp"),
        ));

    tracing::info!("listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("TCP bind");
    axum::serve(listener, app.into_make_service())
        .await
        .expect("server");
}
