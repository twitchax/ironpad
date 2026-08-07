//! ironpad server binary: Axum HTTP + Leptos SSR + WebSocket relay.
//!
//! Wires the Leptos routes, static file serving, and the collaboration
//! WebSocket handlers ([`ironpad_server::ws`]) into a single Axum app, then
//! serves it. Configuration is parsed from CLI/env (see [`config`]).

mod cache_valve;
mod config;
mod http_policy;
mod otel;

use std::net::SocketAddr;

use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderName, HeaderValue};
use axum::routing::get;
use axum::Router;
use clap::Parser;
use leptos::prelude::*;
use leptos_axum::{generate_route_list, LeptosRoutes};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::Layer as _;

use ironpad_app::*;
use ironpad_common::AppConfig;
use ironpad_server::state::{AppState, WsState};
use ironpad_server::{crawl, oembed, og, ws};

use crate::cache_valve::{cache_pressure_valve, fs_usage};
use crate::config::CliArgs;
use crate::http_policy::{cache_control_header, embed_corp_header, share_blob_handler};
use crate::otel::{init_otel, otel_log_filter};

/// Framework-level cap on any request body: derived from the per-share cap
/// enforced in `share_notebook` (with 2x headroom for encoding overhead) so
/// raising one cannot silently strand the other — a body over THIS limit is
/// rejected at the router before the handler's clearer per-share error.
const MAX_REQUEST_BODY_BYTES: usize = 2 * ironpad_app::server_fns::MAX_SHARE_BYTES;

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

    let mut args = CliArgs::parse();
    // Relay knobs are server-local (they configure WsState, not the shared
    // AppConfig), so pull them out before the conversion consumes args.
    let max_guests = args.max_guests;
    let guest_idle_timeout = std::time::Duration::from_secs(args.guest_idle_timeout_secs);
    let max_concurrent_builds = args.max_concurrent_builds;
    // Auth knobs are server-local too (PRD-0053): the OAuth dance and cookie
    // minting live entirely in the auth router.
    let github_oauth = match (
        args.github_client_id.take(),
        args.github_client_secret.take(),
    ) {
        (Some(client_id), Some(client_secret)) => Some(ironpad_server::auth::GithubOauth {
            client_id,
            client_secret,
        }),
        (None, None) => None,
        _ => {
            tracing::warn!(
                "one of GITHUB_CLIENT_ID/GITHUB_CLIENT_SECRET is set without the other; \
                 sign-in disabled"
            );
            None
        }
    };
    let test_auth = args.test_auth;
    let config: AppConfig = args.into();

    tracing::info!(data_dir = %config.data_dir.display(), "data directory");
    tracing::info!(cache_dir = %config.cache_dir.display(), "cache directory");
    tracing::info!(ironpad_cell_path = %config.ironpad_cell_path.display(), "ironpad-cell crate path");

    // Startup-only so it can never race an in-flight build: no requests are
    // being served yet. Fly auto-stops the machine when idle, so restarts (and
    // therefore valve checks) happen at least once per burst of visits.
    cache_pressure_valve(&config.cache_dir, || fs_usage(&config.cache_dir));

    // Accounts database (PRD-0053). Opening SurrealKV takes ~1.5s, which is
    // paid once per boot; it must precede serving since server fns expect the
    // context. Kept OUT of AppState so WS handler tests don't each pay that
    // open — the DB travels as leptos context + the auth router's own state.
    std::fs::create_dir_all(&config.data_dir).expect("create data dir");
    let db = ironpad_app::db::Db::open(&config.data_dir.join("ironpad.db"))
        .await
        .expect("accounts database");
    tracing::info!("accounts database open");

    let auth_enabled = github_oauth.is_some();
    if test_auth {
        tracing::warn!("IRONPAD_TEST_AUTH is set: /auth/test-login is live (e2e only!)");
    }
    let auth_router = ironpad_server::auth::router(ironpad_server::auth::AuthState {
        db: db.clone(),
        github: github_oauth,
        test_auth,
        public_url: config.public_url.clone(),
        http: reqwest::Client::new(),
    });

    let conf = get_configuration(None).expect("leptos configuration");
    let leptos_options = conf.leptos_options;

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));

    let app_state = AppState {
        leptos_options: leptos_options.clone(),
        config: config.clone(),
        ws: WsState::default()
            .with_max_guests(max_guests)
            .with_guest_idle_timeout(guest_idle_timeout),
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
                // Sweep AND disconnect: guests authorize against
                // connect-time permissions, so dropping only the session
                // record would leave an active agent mutating forever.
                let removed = ws.sweep_expired_sessions().await;
                if removed > 0 {
                    tracing::debug!(removed, "swept expired sessions");
                }
            }
        });
    }

    // Pre-warm the toolchain fingerprint / wasm-bindgen CLI version caches on a
    // blocking thread so the first cell compile doesn't block a tokio worker
    // thread on `wasm-bindgen --version` / `rustc --version` shell-outs.
    //
    // Deliberately NOT awaited: this is warm-up, not a prerequisite. Awaiting it
    // put a cold `rustc --version` on the critical path to `bind`, and that call
    // demand-pages ~350 MB of libLLVM + librustc_driver — measured at 5.9s of a
    // 7.2s Fly cold start, paid by every visitor who woke the machine, on pages
    // that never compile. Detached, it overlaps the rest of boot instead. The
    // statics behind it are `LazyLock`, so a request that beats it to the punch
    // simply blocks on the lock exactly as it would have without any prewarm.
    // (On the deploy image `BAKED_VERSIONS_ENV` makes it near-instant anyway.)
    drop(tokio::task::spawn_blocking(
        ironpad_app::compiler::toolchain::prewarm,
    ));

    let routes = generate_route_list(App);

    // Shared across all compile requests; serializes same-cell compiles.
    let compile_locks = ironpad_app::compiler::CompileLocks::default();
    // Build admission (PRD-0052): global cargo-concurrency cap + per-client
    // rate limit on build starts. Cache hits never pass through it.
    let build_admission =
        ironpad_app::compiler::admission::BuildAdmission::from_env(max_concurrent_builds);
    tracing::info!(max_concurrent_builds, "build admission configured");

    let app = Router::new()
        // Sign-in surface (PRD-0053). nest_service because the auth router
        // carries its own state; paths inside are prefix-stripped.
        .nest_service("/auth", auth_router)
        .route("/ws/host", get(ws::ws_host_handler))
        .route("/ws/connect", get(ws::ws_connect_handler))
        .route(
            &format!("{}{{file}}", ironpad_common::SHARE_BLOBS_PREFIX),
            get(share_blob_handler),
        )
        // Social-preview cards and crawler files (PRD-0050). These sit outside
        // the Leptos routes because a crawler wants bytes, not an SSR page.
        .route("/og/ironpad.png", get(og::site_card_handler))
        .route("/og/{class}/{file}", get(og::notebook_card_handler))
        .route("/robots.txt", get(crawl::robots_handler))
        .route("/sitemap.xml", get(crawl::sitemap_handler))
        // oEmbed provider (PRD-0051): consumers that support discovery embed
        // the live notebook instead of the static card.
        .route("/oembed", get(oembed::oembed_handler))
        .leptos_routes_with_context(
            &app_state,
            routes,
            {
                let config = config.clone();
                let leptos_options = leptos_options.clone();
                let compile_locks = compile_locks.clone();
                let build_admission = build_admission.clone();
                let db = db.clone();
                move || {
                    provide_context(config.clone());
                    provide_context(leptos_options.clone());
                    provide_context(compile_locks.clone());
                    provide_context(build_admission.clone());
                    provide_context(db.clone());
                    provide_context(ironpad_app::auth::AuthEnabled(auth_enabled));
                }
            },
            {
                let leptos_options = leptos_options.clone();
                move || shell(leptos_options.clone())
            },
        )
        .fallback(leptos_axum::file_and_error_handler::<AppState, _>(shell))
        .with_state(app_state)
        // The accounts DB for handlers outside the leptos context (the OG
        // mutable-card path). See the boot comment for why it's not in
        // AppState.
        .layer(axum::Extension(db.clone()))
        // Framework-level request-body cap so the per-endpoint
        // `MAX_SHARE_BYTES` (4 MiB) check isn't the only guard. Sized above the
        // largest legitimate body (a max-size shared notebook) so real uploads
        // pass while a truly huge body is rejected before it reaches a handler.
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(http_policy::CONTENT_SECURITY_POLICY),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-embedder-policy"),
            HeaderValue::from_static("require-corp"),
        ))
        // Embeddable responses (/embed/* + the loader script) additionally opt
        // in to being loaded by COEP-isolated third-party pages via CORP
        // (PRD-0039 T-006); everything else keeps same-origin protection.
        .layer(axum::middleware::from_fn(embed_corp_header))
        // Cache policy: hashed pkg assets cache forever, everything else must
        // revalidate. Without this, browsers heuristically cache unhashed
        // JS/WASM across releases and old clients mis-read newer notebooks.
        .layer(axum::middleware::from_fn(cache_control_header))
        // One span per HTTP request (at INFO so it passes the default filter),
        // which is what OpenTelemetry exports as a trace. Outermost layer so it
        // spans the whole request. `otel.name` names the exported span by its
        // route template (`GET /og/{class}/{file}`) so a trace list reads as
        // endpoints instead of a wall of identical "request"s; the raw path is
        // the fallback for the leptos fallback handler, which has no template.
        // Path only, never the full URI: `/ws/connect` carries its session
        // token in the query string, and span fields are exported.
        .layer(TraceLayer::new_for_http().make_span_with(
            |request: &axum::http::Request<axum::body::Body>| {
                let route = request
                    .extensions()
                    .get::<axum::extract::MatchedPath>()
                    .map_or_else(|| request.uri().path(), axum::extract::MatchedPath::as_str);
                tracing::info_span!(
                    "request",
                    otel.name = %format!("{} {route}", request.method()),
                    method = %request.method(),
                    path = request.uri().path(),
                )
            },
        ));

    tracing::info!("listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("TCP bind");
    axum::serve(listener, app.into_make_service())
        .await
        .expect("server");
}
