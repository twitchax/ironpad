mod config;

use std::net::SocketAddr;

use axum::Router;
use clap::Parser;
use leptos::prelude::*;
use leptos_axum::{generate_route_list, LeptosRoutes};

use ironpad_app::*;
use ironpad_common::AppConfig;

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

    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));

    let routes = generate_route_list(App);

    let app = Router::new()
        .leptos_routes_with_context(
            &leptos_options,
            routes,
            {
                let config = config.clone();
                move || {
                    provide_context(config.clone());
                }
            },
            {
                let leptos_options = leptos_options.clone();
                move || shell(leptos_options.clone())
            },
        )
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options);

    tracing::info!("listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}
