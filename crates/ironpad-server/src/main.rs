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
use tower_http::trace::TraceLayer;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::Layer as _;

use ironpad_app::*;
use ironpad_common::AppConfig;
use ironpad_server::state::{AppState, WsState};
use ironpad_server::{crawl, oembed, og, ws};

use crate::config::CliArgs;

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
        .route("/share-blobs/{file}", get(share_blob_handler))
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
            HeaderValue::from_static(CONTENT_SECURITY_POLICY),
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

/// Content Security Policy applied to every response.
///
/// Deliberately says nothing about `script-src`. Leptos hydration emits an
/// inline module script and Monaco ships its own AMD loader, so a script
/// policy here means either `'unsafe-inline'`, which would have permitted the
/// exact injection this is meant to blunt, or per-request nonce plumbing
/// through `leptos_meta` and the Monaco bootstrap. The second is the right
/// answer and is worth its own change; shipping the first would be theatre.
///
/// What is here is the set that costs nothing and closes real amplification
/// paths for an injection that does land:
///
/// - `object-src 'none'` retires the `<object>`/`<embed>` script vectors.
/// - `base-uri 'self'` stops an injected `<base href>` from silently
///   repointing every relative URL on the page, including the pkg bundle.
/// - `form-action 'self'` stops an injected form from posting elsewhere,
///   which is how a stolen mutable-share key would actually leave the page.
/// - `frame-ancestors` is **intentionally absent**: `/embed/*` exists to be
///   framed by third parties (PRD-0039), and any value here would break it.
const CONTENT_SECURITY_POLICY: &str = "object-src 'none'; base-uri 'self'; form-action 'self'";

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
/// URLs. `/share-blobs/` files are content-addressed by cache key (PRD-0047),
/// so they are likewise immutable — the CDN and browser absorb repeat viewer
/// traffic. Everything else (Monaco, executor/storage JS, notebooks, SSR
/// pages) is served under URL-stable paths, so browsers must revalidate on
/// each use (`no-cache` still allows conditional 304s via `Last-Modified`).
fn cache_control_value(path: &str) -> HeaderValue {
    if path.starts_with("/pkg/") || path.starts_with("/share-blobs/") {
        HeaderValue::from_static("public, max-age=31536000, immutable")
    } else {
        HeaderValue::from_static("no-cache")
    }
}

/// Is `file` a well-formed share-blob filename: a 64-hex content hash plus a
/// `.wasm` or `.js` extension? Anything else (traversal, other extensions,
/// stray temp files) is rejected before touching the filesystem.
fn is_valid_share_blob_name(file: &str) -> bool {
    let hash = if let Some(h) = file.strip_suffix(".wasm") {
        h
    } else if let Some(h) = file.strip_suffix(".js") {
        h
    } else {
        return false;
    };
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Serve a snapshotted share blob (PRD-0047) from `{data_dir}/shares/blobs/`.
///
/// Content-addressed and immutable — the cache-control middleware stamps the
/// forever policy from [`cache_control_value`].
async fn share_blob_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(file): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;

    if !is_valid_share_blob_name(&file) {
        return (axum::http::StatusCode::NOT_FOUND, "not found").into_response();
    }

    let path = state
        .config
        .data_dir
        .join("shares")
        .join("blobs")
        .join(&file);
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            // Validation above admits exactly two suffixes.
            let content_type = if file.strip_suffix(".wasm").is_some() {
                "application/wasm"
            } else {
                "text/javascript"
            };
            ([(axum::http::header::CONTENT_TYPE, content_type)], bytes).into_response()
        }
        Err(_) => (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
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
mod csp_tests {
    use super::CONTENT_SECURITY_POLICY;

    #[test]
    fn the_policy_is_a_valid_header_value() {
        assert!(
            axum::http::HeaderValue::from_static(CONTENT_SECURITY_POLICY)
                .to_str()
                .is_ok()
        );
    }

    #[test]
    fn the_policy_closes_the_amplification_paths() {
        for directive in ["object-src 'none'", "base-uri 'self'", "form-action 'self'"] {
            assert!(
                CONTENT_SECURITY_POLICY.contains(directive),
                "missing {directive}"
            );
        }
    }

    #[test]
    fn the_policy_does_not_restrict_framing_or_scripts() {
        // `/embed/*` exists to be framed by third parties (PRD-0039), so any
        // frame-ancestors value breaks the feature outright.
        assert!(
            !CONTENT_SECURITY_POLICY.contains("frame-ancestors"),
            "this would break embedding"
        );
        // `script-src` without a nonce would mean 'unsafe-inline', which
        // permits exactly the injection a CSP is supposed to blunt. Left out
        // on purpose until the nonce work happens.
        assert!(!CONTENT_SECURITY_POLICY.contains("unsafe-inline"));
        assert!(!CONTENT_SECURITY_POLICY.contains("script-src"));
    }
}

#[cfg(test)]
mod cache_header_tests {
    use super::{cache_control_value, is_valid_share_blob_name};

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

        // Share blobs are content-addressed (PRD-0047): immutable forever.
        assert_eq!(
            cache_control_value(&format!("/share-blobs/{}.wasm", "a".repeat(64))),
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

    #[test]
    fn share_blob_names_are_strictly_validated() {
        let hash = "a".repeat(64);
        assert!(is_valid_share_blob_name(&format!("{hash}.wasm")));
        assert!(is_valid_share_blob_name(&format!("{hash}.js")));

        // Wrong length, case, extension, or traversal-ish input: rejected.
        assert!(!is_valid_share_blob_name("abc.wasm"));
        assert!(!is_valid_share_blob_name(&format!(
            "{}.wasm",
            "A".repeat(64)
        )));
        assert!(!is_valid_share_blob_name(&format!("{hash}.json")));
        assert!(!is_valid_share_blob_name(&hash));
        assert!(!is_valid_share_blob_name(&format!("../{hash}.wasm")));
        assert!(!is_valid_share_blob_name(&format!("{hash}.wasm.tmp.x")));
        assert!(!is_valid_share_blob_name(""));
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
