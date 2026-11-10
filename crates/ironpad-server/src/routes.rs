//! The non-Leptos route tables (and the app-wide body cap), defined once so
//! the binary and the integration tests mount the SAME wiring.
//!
//! These used to be `.route(...)` calls inside `main.rs`, which a test cannot
//! reach, so `tests/` rebuilt them by hand and promised in a comment to keep
//! them in sync. A path or extractor changed in `main.rs` would then have left
//! the tests exercising the old wiring, green.

use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use axum::Router;

use crate::state::AppState;
use crate::{crawl, oembed, og, ws};

/// Framework-level cap on any request body: derived from the per-share cap
/// enforced in `share_notebook` (with 2x headroom for encoding overhead) so
/// raising one cannot silently strand the other. A body over THIS limit is
/// rejected while it is read, before the handler's clearer per-share error.
pub const MAX_REQUEST_BODY_BYTES: usize = 2 * ironpad_app::server_fns::MAX_SHARE_BYTES;

/// The app-wide request-body cap, layered over the whole router.
///
/// `DefaultBodyLimit` only records the limit on the request; whatever reads
/// the body has to honour it. Every route but the server functions is a GET,
/// so the server-fn request reader is the one that matters, and it only reads
/// through the limit from server_fn 0.8.13 on: below that the cap bound
/// nothing. `tests/body_limit.rs` holds that line.
pub fn request_body_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES)
}

/// The collaboration WebSocket endpoints: browser hosts and CLI guests.
pub fn ws_routes() -> Router<AppState> {
    Router::new()
        .route("/ws/host", get(ws::ws_host_handler))
        .route("/ws/connect", get(ws::ws_connect_handler))
}

/// Social-preview cards and crawler files (PRD-0050), plus the oEmbed
/// provider (PRD-0051).
///
/// These sit outside the Leptos routes because a crawler wants bytes, not an
/// SSR page. oEmbed lets consumers that support discovery embed the live
/// notebook instead of the static card.
///
/// The OG card and oEmbed handlers extract the accounts DB, so callers must
/// layer `axum::Extension(Db)` over the router these are merged into.
pub fn crawler_routes() -> Router<AppState> {
    Router::new()
        .route("/og/ironpad.png", get(og::site_card_handler))
        .route("/og/{class}/{file}", get(og::notebook_card_handler))
        .route("/robots.txt", get(crawl::robots_handler))
        .route("/sitemap.xml", get(crawl::sitemap_handler))
        .route("/oembed", get(oembed::oembed_handler))
}
