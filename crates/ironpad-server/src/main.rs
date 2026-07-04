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
use tracing_subscriber::Layer as _;

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
    // supplied via `fly secrets set`). The exporters read the endpoint and auth
    // headers from the standard `OTEL_EXPORTER_OTLP_*` env vars, so no
    // credentials ever live in code or config. When unset this is a no-op and
    // the server logs to stdout exactly as before.
    let otel = init_otel();
    let otel_trace_layer = otel.as_ref().map(|providers| {
        use opentelemetry::trace::TracerProvider as _;
        tracing_opentelemetry::layer().with_tracer(providers.tracer.tracer("ironpad-server"))
    });
    // Bridge `tracing` events into OTLP log records. The bridge attaches the
    // active span's `trace_id`/`span_id`, so logs emitted inside a request span
    // correlate with its trace in Grafana. Its per-layer filter drops the
    // telemetry stack's own events so exporting logs can't feed back on itself.
    let otel_log_layer = otel.as_ref().map(|providers| {
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&providers.logger)
            .with_filter(otel_log_filter())
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(otel_trace_layer)
        .with(otel_log_layer)
        .init();

    if otel.is_some() {
        tracing::info!("OpenTelemetry OTLP trace + log export enabled");
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

/// Tracer + logger providers for OTLP export, held for the process lifetime so
/// their batch exporters keep flushing.
struct OtelProviders {
    tracer: opentelemetry_sdk::trace::SdkTracerProvider,
    logger: opentelemetry_sdk::logs::SdkLoggerProvider,
}

/// Build the OTLP tracer + logger providers when OpenTelemetry export is
/// configured.
///
/// Opt-in: returns `None` unless `OTEL_EXPORTER_OTLP_ENDPOINT` is set, so the
/// server keeps its plain stdout logging until an endpoint is provided. Both
/// exporters read the endpoint and auth headers from the standard
/// `OTEL_EXPORTER_OTLP_*` env vars, so no credentials live in code or config.
/// The shared `service.name` resource is what lets Grafana correlate the two
/// signals.
fn init_otel() -> Option<OtelProviders> {
    if !otel_export_enabled(std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").as_deref()) {
        return None;
    }

    // The subscriber isn't initialized yet, so stderr is the only channel for
    // exporter-build failures.
    let span_exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build()
    {
        Ok(exporter) => exporter,
        Err(err) => {
            eprintln!("OTLP span exporter init failed; export disabled: {err}");
            return None;
        }
    };
    let log_exporter = match opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .build()
    {
        Ok(exporter) => exporter,
        Err(err) => {
            eprintln!("OTLP log exporter init failed; export disabled: {err}");
            return None;
        }
    };

    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name("ironpad-server")
        .build();

    let tracer = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(span_exporter)
        .with_resource(resource.clone())
        .build();
    let logger = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_batch_exporter(log_exporter)
        .with_resource(resource)
        .build();

    Some(OtelProviders { tracer, logger })
}

/// Per-layer filter for the OTLP log bridge: forward application logs but drop
/// the telemetry stack's own events, so exporting a log can't emit a log that
/// gets exported again (a feedback loop).
fn otel_log_filter() -> tracing_subscriber::filter::Targets {
    use tracing_subscriber::filter::LevelFilter;
    tracing_subscriber::filter::Targets::new()
        .with_default(LevelFilter::TRACE)
        .with_target("opentelemetry", LevelFilter::OFF)
        .with_target("hyper", LevelFilter::OFF)
        .with_target("hyper_util", LevelFilter::OFF)
        .with_target("reqwest", LevelFilter::OFF)
        .with_target("h2", LevelFilter::OFF)
        .with_target("rustls", LevelFilter::OFF)
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
