mod config;

use std::net::SocketAddr;

use axum::http::{HeaderName, HeaderValue};
use axum::routing::get;
use axum::Router;
use clap::Parser;
use leptos::prelude::*;
use leptos_axum::{generate_route_list, LeptosRoutes};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use ironpad_app::*;
use ironpad_common::AppConfig;
use ironpad_server::state::{AppState, WsState};
use ironpad_server::ws;

use crate::config::CliArgs;

#[tokio::main]
async fn main() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    // OpenTelemetry OTLP export is opt-in: enabled only when
    // `OTEL_EXPORTER_OTLP_ENDPOINT` is set (e.g. a Grafana Cloud OTLP gateway,
    // supplied via `fly secrets set`). The exporter reads the endpoint and auth
    // headers from the standard `OTEL_EXPORTER_OTLP_*` env vars, so no
    // credentials ever live in code or config. When unset this is a no-op and
    // the server logs to stdout exactly as before.
    let otel_provider = init_otel_provider();
    let otel_layer = otel_provider.as_ref().map(|provider| {
        use opentelemetry::trace::TracerProvider as _;
        tracing_opentelemetry::layer().with_tracer(provider.tracer("ironpad-server"))
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(otel_layer)
        .init();

    if otel_provider.is_some() {
        tracing::info!("OpenTelemetry OTLP trace export enabled");
    }

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

    // Periodically sweep expired sessions so the store doesn't grow unbounded
    // when guests never explicitly end their sessions.
    {
        let ws = app_state.ws.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            interval.tick().await; // consume the immediate first tick
            loop {
                interval.tick().await;
                let removed = ws.sessions.sweep_expired().await;
                if removed > 0 {
                    tracing::debug!(removed, "swept expired sessions");
                }
            }
        });
    }

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

    // Shared across all compile requests; serializes same-cell compiles.
    let compile_locks = ironpad_app::compiler::CompileLocks::default();

    let app = Router::new()
        .route("/ws/host", get(ws::ws_host_handler))
        .route("/ws/connect", get(ws::ws_connect_handler))
        .leptos_routes_with_context(
            &app_state,
            routes,
            {
                let config = config.clone();
                let leptos_options = leptos_options.clone();
                let compile_locks = compile_locks.clone();
                move || {
                    provide_context(config.clone());
                    provide_context(leptos_options.clone());
                    provide_context(compile_locks.clone());
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
        ))
        // One span per HTTP request (at INFO so it passes the default filter),
        // which is what OpenTelemetry exports as a trace. Outermost layer so it
        // spans the whole request.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(tracing::Level::INFO)),
        );

    tracing::info!("listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("TCP bind");
    axum::serve(listener, app.into_make_service())
        .await
        .expect("server");
}

/// Build an OTLP tracer provider when OpenTelemetry export is configured.
///
/// Opt-in: returns `None` unless `OTEL_EXPORTER_OTLP_ENDPOINT` is set, so the
/// server keeps its plain stdout logging until an endpoint is provided. The
/// OTLP exporter reads the endpoint and auth headers from the standard
/// `OTEL_EXPORTER_OTLP_*` env vars, so no credentials live in code or config.
fn init_otel_provider() -> Option<opentelemetry_sdk::trace::SdkTracerProvider> {
    if !otel_export_enabled(std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").as_deref()) {
        return None;
    }

    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build()
    {
        Ok(exporter) => exporter,
        Err(err) => {
            // The subscriber isn't initialized yet, so stderr is the only channel.
            eprintln!("OTLP exporter init failed; trace export disabled: {err}");
            return None;
        }
    };

    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_service_name("ironpad-server")
                .build(),
        )
        .build();

    Some(provider)
}

/// OTLP export is opt-in on a configured endpoint. Pulled out as a pure helper
/// so the gating contract is unit-testable without touching process env.
fn otel_export_enabled(endpoint: Option<&std::ffi::OsStr>) -> bool {
    endpoint.is_some()
}

#[cfg(test)]
mod tests {
    use super::otel_export_enabled;

    #[test]
    fn otel_export_is_off_without_an_endpoint() {
        assert!(!otel_export_enabled(None));
    }

    #[test]
    fn otel_export_is_on_with_an_endpoint() {
        assert!(otel_export_enabled(Some(std::ffi::OsStr::new(
            "https://otlp-gateway.grafana.net/otlp"
        ))));
    }
}
