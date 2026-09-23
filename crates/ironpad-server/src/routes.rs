//! The non-Leptos route tables, defined once so the binary and the
//! integration tests mount the SAME wiring.
//!
//! These used to be `.route(...)` calls inside `main.rs`, which a test cannot
//! reach, so `tests/` rebuilt them by hand and promised in a comment to keep
//! them in sync. A path or extractor changed in `main.rs` would then have left
//! the tests exercising the old wiring, green.

use axum::routing::get;
use axum::Router;

use crate::state::AppState;
use crate::{crawl, oembed, og, ws};

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
