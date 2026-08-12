//! Integration test: an unpublished account notebook (PRD-0064) is invisible
//! on every anonymous surface, asserted against RAW HTTP responses.
//!
//! The design claims this is free because every reader-facing surface funnels
//! through `get_mutable_notebook_core` / `mutable_access_core`. This asserts it
//! end to end instead, over real requests against a real router: a notebook
//! that exists but has never been published must produce neither a readable
//! page nor a single byte of its own text, for anonymous visitors AND for
//! signed-in strangers.
//!
//! Two rules this file exists to enforce, both learned the hard way:
//!
//! * **Status against the raw body, never the hydrated DOM.** PRD-0050 and
//!   PRD-0063 both shipped a status that was correct in the browser and wrong
//!   on the wire, because under out-of-order streaming the status commits when
//!   the shell flushes. `SsrMode::Async` on `/mutable` is what makes the 404
//!   honest, and only a raw response can see it.
//! * **Both denied identities.** PRD-0063 shipped a 404 for anonymous visitors
//!   and a 200 for signed-in non-admins from the same handler, precisely
//!   because only the signed-in path needs a session lookup before the shell
//!   flushes. Anonymous and signed-in-stranger are different code paths and get
//!   asserted separately every time.
//!
//! **Scope: the handlers that do not render `App`.** The reader page and the
//! embed are covered in `tests/e2e/account-notebooks.spec.ts` instead, against
//! the same raw responses, because they cannot be tested from here. Rendering
//! `App` calls `generate_route_list`, which runs `RouteScripts`; that component
//! is correctly `#[cfg(feature = "hydrate")]`-gated and never compiles into the
//! production server, but `ironpad-frontend` enables `ironpad-app/hydrate`, and
//! `cargo make test` runs the workspace with no `-p`, so feature unification
//! turns it on beside `ssr` and the render dies reaching `js-sys` statics on a
//! non-wasm target. A single-crate run passes and the gate does not. Keeping
//! these three here is deliberate: Playwright only runs under `uat`, so OG,
//! oEmbed and the sitemap keep their denial coverage at `ci` speed.
//!
//! The manifest — the fifth surface, and the one that does NOT funnel through
//! either core — is gated by `mutable_manifest_access_core`, which is crate
//! private to `ironpad-app`; its coverage lives beside it in
//! `ironpad_app::server_fns`.

use std::path::PathBuf;

use axum::routing::get;
use axum::Router;
use ironpad_app::db::Db;
use ironpad_common::AppConfig;
use ironpad_server::state::{AppState, WsState};
use ironpad_server::{crawl, oembed, og};
use leptos::config::LeptosOptions;

// ── Fixtures ────────────────────────────────────────────────────────────────

/// Strings that appear NOWHERE in the app shell, so finding one in a response
/// body means the notebook itself leaked rather than some boilerplate collision.
const SECRET_TITLE: &str = "Zarquon Ledger Of Unshipped Things";
const SECRET_DESCRIPTION: &str = "quibbleflange accounting, second draft";
const SECRET_SOURCE: &str = "let plutonium_count = 8675309;";

fn secret_notebook_json() -> String {
    format!(
        r#"{{
            "version": 1,
            "id": "00000000-0000-0000-0000-000000000064",
            "title": "{SECRET_TITLE}",
            "description": "{SECRET_DESCRIPTION}",
            "tags": ["test"],
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z",
            "cells": [
                {{
                    "id": "cell-1",
                    "order": 0,
                    "label": "Cell 1",
                    "cell_type": "Code",
                    "source": "{SECRET_SOURCE}",
                    "version": 0
                }}
            ]
        }}"#
    )
}

/// Every distinctive string a leak could carry.
const SECRETS: [&str; 3] = [SECRET_TITLE, SECRET_DESCRIPTION, SECRET_SOURCE];

fn app_state(data_dir: PathBuf, cache_dir: PathBuf) -> AppState {
    AppState {
        leptos_options: LeptosOptions::builder().output_name("ironpad-test").build(),
        config: AppConfig {
            data_dir,
            cache_dir,
            port: 0,
            ironpad_cell_path: PathBuf::from("/tmp"),
            compilation_proxy: None,
            public_url: "http://localhost".to_string(),
            admin_login: None,
        },
        ws: WsState::default(),
    }
}

/// The three crawler-facing handlers, wired exactly as `main.rs` wires them,
/// including the DB `Extension` the OG handler reads.
fn router(state: AppState, db: Db) -> Router {
    Router::new()
        .route("/og/{class}/{file}", get(og::notebook_card_handler))
        .route("/oembed", get(oembed::oembed_handler))
        .route("/sitemap.xml", get(crawl::sitemap_handler))
        .with_state(state)
        .layer(axum::Extension(db))
}

async fn serve(state: AppState, db: Db) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind to random port");
    let addr = listener.local_addr().expect("local addr");
    let app = router(state, db);
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .expect("server");
    });
    format!("http://{addr}")
}

/// One raw GET, optionally carrying a session cookie. Returns the status and
/// the complete body: asserting on the status alone would miss a 404 that
/// still streamed the notebook into the page it refused.
async fn get_raw(url: &str, session: Option<&str>) -> (u16, String) {
    let mut req = reqwest::Client::new().get(url);
    if let Some(token) = session {
        req = req.header(
            "Cookie",
            format!("{}={token}", ironpad_app::auth::SESSION_COOKIE),
        );
    }
    let res = req.send().await.expect("request");
    let status = res.status().as_u16();
    (status, res.text().await.expect("body"))
}

fn assert_no_leak(surface: &str, body: &str) {
    for secret in SECRETS {
        assert!(
            !body.contains(secret),
            "{surface} leaked {secret:?} for a denied viewer"
        );
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// The whole visibility claim of PRD-0064, over the wire.
///
/// Sequenced as deny-then-publish deliberately: the publish half is the
/// positive control. Without it every assertion above would also pass against
/// a router that served nothing at all, and a test that cannot distinguish
/// "correctly refused" from "broken" proves nothing.
#[tokio::test]
async fn an_unpublished_account_notebook_is_invisible_on_every_anonymous_surface() {
    let dbdir = tempfile::tempdir().expect("db dir");
    let data = tempfile::tempdir().expect("data dir");
    let cache = tempfile::tempdir().expect("cache dir");

    let db = Db::open(&dbdir.path().join("test.db"))
        .await
        .expect("open accounts db");
    db.upsert_user("1", "alice", "").await.expect("owner");
    db.upsert_user("2", "bob", "").await.expect("stranger");

    let json = secret_notebook_json();
    let id = db
        .create_account_notebook("1", &json)
        .await
        .expect("owner saves to their account");

    // A real session for the signed-in stranger: the denied-while-signed-in
    // path is a DIFFERENT path, and the one PRD-0063 got wrong.
    let bob_session = db.create_session("2").await.expect("stranger signs in");

    let base = serve(
        app_state(data.path().into(), cache.path().into()),
        db.clone(),
    )
    .await;

    for (who, session) in [
        ("anonymous", None),
        ("signed-in stranger", Some(&*bob_session)),
    ] {
        // OG card: a title in a preview card is already the leak.
        let (status, body) = get_raw(&format!("{base}/og/mutable/{id}.png"), session).await;
        assert_eq!(status, 404, "{who}: the OG card must 404");
        assert_no_leak(&format!("{who}: /og/mutable"), &body);

        // oEmbed: returns the title in JSON when it resolves at all.
        let target = format!("http://localhost/mutable/{id}");
        let (status, body) = get_raw(
            &format!("{base}/oembed?url={}", urlencode(&target)),
            session,
        )
        .await;
        assert_eq!(status, 404, "{who}: oEmbed must 404");
        assert_no_leak(&format!("{who}: /oembed"), &body);
    }

    // Enumeration, not just direct fetch: the sitemap lists `/public` only,
    // so no crawler learns the id exists in the first place. `/mutable` is
    // unlisted whether published or not (PRD-0050), which is why this holds
    // after the publish below too.
    let (status, body) = get_raw(&format!("{base}/sitemap.xml"), None).await;
    assert_eq!(status, 200, "the sitemap renders");
    assert!(
        !body.contains(&id),
        "the sitemap must not enumerate a mutable share: {body}"
    );

    // Positive control: publish it, and the same requests start working.
    // (`promote_draft` is what `push_mutable` calls once the owner grant and
    // the blob snapshot are settled; neither is under test here.)
    db.promote_draft(&id, None, json.len() as u64)
        .await
        .expect("owner publishes");

    let (status, _) = get_raw(&format!("{base}/og/mutable/{id}.png"), None).await;
    assert_eq!(status, 200, "the OG card renders once published");
    let (status, body) = get_raw(
        &format!(
            "{base}/oembed?url={}",
            urlencode(&format!("http://localhost/mutable/{id}"))
        ),
        None,
    )
    .await;
    assert_eq!(status, 200, "oEmbed resolves once published");
    assert!(body.contains(SECRET_TITLE), "oEmbed carries the title");
}

/// Percent-encode a URL for use as a query-string value. `reqwest`'s builder
/// would do this, but the assertions read better with the URL spelled out.
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
