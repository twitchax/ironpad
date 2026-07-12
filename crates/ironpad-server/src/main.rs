//! ironpad server binary: Axum HTTP + Leptos SSR + WebSocket relay.
//!
//! Wires the Leptos routes, static file serving, and the collaboration
//! WebSocket handlers ([`ironpad_server::ws`]) into a single Axum app, then
//! serves it. Configuration is parsed from CLI/env (see [`config`]).

mod config;

use std::net::SocketAddr;

use axum::extract::DefaultBodyLimit;
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

/// Framework-level cap on any request body. Comfortably above the 4 MiB
/// per-share cap enforced in `share_notebook` so legitimate max-size uploads
/// pass, while bounding truly oversized bodies at the router layer.
const MAX_REQUEST_BODY_BYTES: usize = 8 * 1024 * 1024;

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

    // Startup-only so it can never race an in-flight build: no requests are
    // being served yet. Fly auto-stops the machine when idle, so restarts (and
    // therefore valve checks) happen at least once per burst of visits.
    cache_pressure_valve(&config.cache_dir, || fs_usage(&config.cache_dir));

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
        // Framework-level request-body cap so the per-endpoint
        // `MAX_SHARE_BYTES` (4 MiB) check isn't the only guard. Sized above the
        // largest legitimate body (a max-size shared notebook) so real uploads
        // pass while a truly huge body is rejected before it reaches a handler.
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
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

/// Maximum percentage of the cache filesystem that may be in use before the
/// startup pressure valve clears the rebuildable caches. The compile cache
/// shares its volume with the share store, and a full disk fails cell
/// compiles AND share writes — trading cold rebuilds for headroom is always
/// the right side of that bargain.
const CACHE_PRESSURE_MAX_USED_PCT: u8 = 80;

/// Absolute free-space floor for the pressure valve: a volume with at least
/// this much headroom is not under pressure no matter what the percentage
/// says. Percentage alone misfires on big disks — a dev box at 86% of 3TB
/// still has hundreds of GB free, and wiping its caches on every server
/// start makes the first live check of each e2e run minutes-cold. On the
/// 5GB Fly volume, available space can never reach this floor, so prod
/// behavior is decided by the percentage exactly as before.
const CACHE_PRESSURE_MIN_FREE_BYTES: u64 = 20 * 1024 * 1024 * 1024;

/// Usage of the filesystem holding the cache dir.
#[derive(Clone, Copy, Debug)]
struct FsUsage {
    used_pct: u8,
    available_bytes: u64,
}

impl FsUsage {
    /// Under pressure only when the volume is BOTH proportionally full and
    /// short on absolute headroom.
    fn under_pressure(self) -> bool {
        self.used_pct >= CACHE_PRESSURE_MAX_USED_PCT
            && self.available_bytes < CACHE_PRESSURE_MIN_FREE_BYTES
    }
}

/// Usage of the filesystem holding `path`, or `None` when it can't be
/// measured (non-unix, or `statvfs` failure).
#[cfg(unix)]
fn fs_usage(path: &std::path::Path) -> Option<FsUsage> {
    use std::os::unix::ffi::OsStrExt as _;

    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid NUL-terminated path and `stat` is a valid
    // out-pointer for the duration of the call.
    if unsafe { libc::statvfs(c_path.as_ptr(), &raw mut stat) } != 0 {
        return None;
    }
    if stat.f_blocks == 0 {
        return None;
    }
    let used = u128::from(stat.f_blocks.saturating_sub(stat.f_bavail));
    let pct = used * 100 / u128::from(stat.f_blocks);
    let available = u64::try_from(u128::from(stat.f_bavail) * u128::from(stat.f_frsize)).ok()?;
    Some(FsUsage {
        used_pct: u8::try_from(pct).ok()?,
        available_bytes: available,
    })
}

#[cfg(not(unix))]
fn fs_usage(_path: &std::path::Path) -> Option<FsUsage> {
    None
}

/// Disk-pressure valve for the compile cache: when the cache volume is at or
/// above [`CACHE_PRESSURE_MAX_USED_PCT`], clear the rebuildable caches in two
/// tiers — `targets/` + `workspaces/` (pure compile-speed caches) first, then
/// `cargo-home/` (crates.io registry cache) only if pressure persists.
///
/// `blobs/` is never touched: it holds the content-addressed compiled cells,
/// so unchanged cells stay warm across a wipe and only the next *novel*
/// compile pays a cold build.
///
/// `usage_probe` measures the volume's usage; it is called once up front
/// and again after the first tier to decide on escalation (injected so tests
/// can drive both decisions without a real full disk).
fn cache_pressure_valve(cache_dir: &std::path::Path, usage_probe: impl Fn() -> Option<FsUsage>) {
    let Some(usage) = usage_probe() else {
        tracing::warn!("cache volume usage unmeasurable; pressure valve skipped");
        return;
    };
    if !usage.under_pressure() {
        tracing::info!(
            used_pct = usage.used_pct,
            available_bytes = usage.available_bytes,
            "cache volume below pressure threshold"
        );
        return;
    }

    tracing::warn!(
        used_pct = usage.used_pct,
        available_bytes = usage.available_bytes,
        "cache volume under disk pressure — clearing rebuildable caches"
    );
    clear_cache_tier(cache_dir, &["targets", "workspaces"]);

    // Re-measure: only escalate to the registry cache if still under pressure.
    match usage_probe() {
        Some(still) if still.under_pressure() => {
            tracing::warn!(
                used_pct = still.used_pct,
                "pressure persists — clearing the cargo registry cache too"
            );
            clear_cache_tier(cache_dir, &["cargo-home"]);
        }
        Some(still) => tracing::info!(used_pct = still.used_pct, "pressure relieved"),
        None => {}
    }
}

/// Remove the given subdirectories of `cache_dir`, ignoring ones that don't
/// exist and logging (but not failing on) other errors — the valve must never
/// prevent startup.
fn clear_cache_tier(cache_dir: &std::path::Path, subdirs: &[&str]) {
    for sub in subdirs {
        let dir = cache_dir.join(sub);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => tracing::info!(dir = %dir.display(), "cleared cache tier"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "failed to clear cache tier");
            }
        }
    }
}

#[cfg(test)]
mod cache_pressure_tests {
    use super::{
        cache_pressure_valve, fs_usage, FsUsage, CACHE_PRESSURE_MAX_USED_PCT,
        CACHE_PRESSURE_MIN_FREE_BYTES,
    };

    /// A volume that is proportionally full AND short on headroom.
    fn pressured(used_pct: u8) -> FsUsage {
        FsUsage {
            used_pct,
            available_bytes: 1024 * 1024 * 1024,
        }
    }

    fn seed_cache(root: &std::path::Path) {
        for sub in ["targets/a", "workspaces/b", "blobs", "cargo-home/reg"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        std::fs::write(root.join("targets/a/artifact.rlib"), b"x").unwrap();
        std::fs::write(root.join("blobs/deadbeef.wasm"), b"\0asm").unwrap();
    }

    #[test]
    fn below_threshold_wipes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        seed_cache(tmp.path());
        cache_pressure_valve(tmp.path(), || {
            Some(pressured(CACHE_PRESSURE_MAX_USED_PCT - 1))
        });
        assert!(tmp.path().join("targets/a/artifact.rlib").exists());
        assert!(tmp.path().join("workspaces/b").exists());
    }

    #[test]
    fn at_threshold_wipes_rebuildable_tiers_but_never_blobs() {
        let tmp = tempfile::tempdir().unwrap();
        seed_cache(tmp.path());
        // First measurement: under pressure. Re-measurement: relieved.
        let calls = std::cell::Cell::new(0u8);
        cache_pressure_valve(tmp.path(), || {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Some(pressured(CACHE_PRESSURE_MAX_USED_PCT))
            } else {
                Some(pressured(40))
            }
        });
        assert!(!tmp.path().join("targets").exists());
        assert!(!tmp.path().join("workspaces").exists());
        // The warm-cell cache survives every tier.
        assert!(tmp.path().join("blobs/deadbeef.wasm").exists());
        // Pressure relieved after tier one, so the registry tier is spared.
        assert!(tmp.path().join("cargo-home/reg").exists());
    }

    #[test]
    fn persistent_pressure_escalates_to_the_registry_tier() {
        let tmp = tempfile::tempdir().unwrap();
        seed_cache(tmp.path());
        cache_pressure_valve(tmp.path(), || Some(pressured(95)));
        assert!(!tmp.path().join("targets").exists());
        assert!(!tmp.path().join("cargo-home").exists());
        // Blobs survive even full escalation.
        assert!(tmp.path().join("blobs/deadbeef.wasm").exists());
    }

    #[test]
    fn unmeasurable_usage_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        seed_cache(tmp.path());
        cache_pressure_valve(tmp.path(), || None);
        assert!(tmp.path().join("targets/a/artifact.rlib").exists());
    }

    #[test]
    fn high_percentage_with_ample_headroom_wipes_nothing() {
        // The dev-box case: a big disk past the percentage threshold but with
        // hundreds of GB free is NOT under pressure (the wipe would only slow
        // the next runs down; see CACHE_PRESSURE_MIN_FREE_BYTES).
        let tmp = tempfile::tempdir().unwrap();
        seed_cache(tmp.path());
        cache_pressure_valve(tmp.path(), || {
            Some(FsUsage {
                used_pct: 95,
                available_bytes: CACHE_PRESSURE_MIN_FREE_BYTES * 20,
            })
        });
        assert!(tmp.path().join("targets/a/artifact.rlib").exists());
        assert!(tmp.path().join("cargo-home/reg").exists());
    }

    #[test]
    fn fs_usage_measures_real_filesystems() {
        let tmp = tempfile::tempdir().unwrap();
        let usage = fs_usage(tmp.path()).expect("statvfs should work on a tempdir");
        assert!(usage.used_pct <= 100);
        assert!(usage.available_bytes > 0);
    }
}

/// Is this a path we deliberately serve to third-party embedders? The embed
/// routes themselves plus the two loader scripts a host page pulls directly.
fn is_embeddable_path(path: &str) -> bool {
    path.starts_with("/embed/") || path == "/embed.js" || path == "/embed-frame.js"
}

/// Stamp `Cross-Origin-Resource-Policy: cross-origin` onto embeddable
/// responses so pages that are themselves COEP-isolated can still frame the
/// notebook and load `embed.js` (PRD-0039 T-006).
async fn embed_corp_header(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let embeddable = is_embeddable_path(req.uri().path());
    let mut res = next.run(req).await;
    if embeddable {
        res.headers_mut().insert(
            HeaderName::from_static("cross-origin-resource-policy"),
            HeaderValue::from_static("cross-origin"),
        );
    }
    res
}

/// Cache-Control value for a request path.
///
/// The `/pkg/` bundle carries a content hash in its filename (cargo-leptos
/// `hash-files`), so it can be cached forever: a new release references new
/// URLs. Everything else (Monaco, executor/storage JS, notebooks, SSR pages)
/// is served under URL-stable paths, so browsers must revalidate on each use
/// (`no-cache` still allows conditional 304s via `Last-Modified`).
fn cache_control_value(path: &str) -> HeaderValue {
    if path.starts_with("/pkg/") {
        HeaderValue::from_static("public, max-age=31536000, immutable")
    } else {
        HeaderValue::from_static("no-cache")
    }
}

/// Sets the cache policy from [`cache_control_value`] on every response that
/// doesn't already declare one.
async fn cache_control_header(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let value = cache_control_value(req.uri().path());
    let mut res = next.run(req).await;
    if !res
        .headers()
        .contains_key(axum::http::header::CACHE_CONTROL)
    {
        res.headers_mut()
            .insert(axum::http::header::CACHE_CONTROL, value);
    }
    res
}

#[cfg(test)]
mod embed_header_tests {
    use super::is_embeddable_path;

    #[test]
    fn embeddable_paths_are_exactly_the_embed_surface() {
        assert!(is_embeddable_path("/embed/shared/abc123"));
        assert!(is_embeddable_path("/embed/public/welcome.ironpad"));
        assert!(is_embeddable_path("/embed.js"));
        assert!(is_embeddable_path("/embed-frame.js"));

        assert!(!is_embeddable_path("/"));
        assert!(!is_embeddable_path("/shared/abc123"));
        assert!(!is_embeddable_path("/notebook/public/welcome.ironpad"));
        assert!(!is_embeddable_path("/embedx"));
        assert!(!is_embeddable_path("/api/embed/whatever"));
    }
}

#[cfg(test)]
mod cache_header_tests {
    use super::cache_control_value;

    #[test]
    fn hashed_pkg_assets_are_immutable_everything_else_revalidates() {
        assert_eq!(
            cache_control_value("/pkg/ironpad.abc123.wasm"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            cache_control_value("/pkg/ironpad.abc123.js"),
            "public, max-age=31536000, immutable"
        );

        // URL-stable assets and pages must revalidate every use: a stale
        // cached bundle silently drops notebook fields it predates.
        assert_eq!(cache_control_value("/"), "no-cache");
        assert_eq!(cache_control_value("/executor-bridge.js"), "no-cache");
        assert_eq!(cache_control_value("/monaco/vs/loader.js"), "no-cache");
        assert_eq!(cache_control_value("/notebooks/cannon.ironpad"), "no-cache");
        assert_eq!(cache_control_value("/pkgx/evil.js"), "no-cache");
    }
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
