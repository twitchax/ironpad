//! Integration test: the app-wide request-body cap binds a server-function
//! upload, asserted over raw HTTP against the production layer.
//!
//! `DefaultBodyLimit` only records the limit on the request; the code that
//! reads the body has to honour it. Every route but the server functions is a
//! GET, so the cap rests entirely on the server-fn request reader, and through
//! server_fn 0.8.11 that reader collected the raw body and ignored the limit.
//! The 8 MiB cap bound nothing; only `share_notebook`'s own 4 MiB check, run
//! AFTER the whole body was buffered, stood in the way. These tests pin both
//! sides of the line on the endpoint the cap is sized for: one byte over, the
//! request dies in the reader with the length-limit error and the function
//! never runs; exactly at the cap, it reaches the function.

use std::path::Path;

use axum::extract::Request;
use axum::routing::post;
use axum::Router;
use ironpad_app::server_fns::{ShareNotebook, MAX_SHARE_BYTES};
use ironpad_common::AppConfig;
use ironpad_server::routes::{request_body_limit, MAX_REQUEST_BODY_BYTES};
use leptos::server_fn::ServerFn as _;

/// The share endpoint behind the production body cap, serving server fns the
/// way `leptos_routes_with_context` mounts them, with the `AppConfig` context
/// `share_notebook` reads.
async fn serve(data_dir: &Path) -> String {
    let config = AppConfig::for_tests(data_dir);
    let handler = move |req: Request| {
        let config = config.clone();
        async move {
            leptos_axum::handle_server_fns_with_context(
                move || leptos::prelude::provide_context(config.clone()),
                req,
            )
            .await
        }
    };
    let app = Router::new()
        .route(ShareNotebook::PATH, post(handler))
        .layer(request_body_limit());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind to random port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .expect("server");
    });
    format!("http://{addr}")
}

/// A URL-encoded `share_notebook` body (the default `#[server]` encoding) of
/// exactly `total` bytes. The filler is plain ASCII, so nothing is escaped and
/// the body length is the byte count on the wire.
fn share_body(total: usize) -> String {
    const FIELD: &str = "notebook_json=";
    format!("{FIELD}{}", "x".repeat(total - FIELD.len()))
}

async fn post_share(base: &str, body: String) -> (reqwest::StatusCode, String) {
    let res = reqwest::Client::new()
        .post(format!("{base}{}", ShareNotebook::PATH))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .expect("request");
    let status = res.status();
    (status, res.text().await.expect("body"))
}

fn no_share_written(data_dir: &Path) -> bool {
    std::fs::read_dir(data_dir.join("shares")).map_or(true, |mut dir| dir.next().is_none())
}

#[tokio::test]
async fn a_body_over_the_cap_is_refused_before_the_function_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = serve(dir.path()).await;

    let (status, body) = post_share(&base, share_body(MAX_REQUEST_BODY_BYTES + 1)).await;

    assert!(
        !status.is_success(),
        "an over-cap upload succeeded: {status}"
    );
    assert!(
        body.contains("length limit exceeded"),
        "an over-cap body must die in the request reader, not in a handler; got {status}: {body}"
    );
    assert!(
        !body.contains("notebook too large"),
        "the share handler ran on an over-cap body, so the cap bound nothing: {body}"
    );
    assert!(
        no_share_written(dir.path()),
        "an over-cap upload wrote a share"
    );
}

#[tokio::test]
async fn a_body_at_the_cap_reaches_the_function() {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = serve(dir.path()).await;

    // The cap is inclusive, and a body this size still carries a notebook
    // over the per-share cap, so the function's own refusal proves the request
    // got through the reader rather than being refused by it.
    let (status, body) = post_share(&base, share_body(MAX_REQUEST_BODY_BYTES)).await;

    assert!(
        !status.is_success(),
        "an over-size notebook was shared: {status}"
    );
    assert!(
        body.contains(&format!(
            "notebook too large: {} bytes (max {MAX_SHARE_BYTES})",
            MAX_REQUEST_BODY_BYTES - "notebook_json=".len()
        )),
        "a body at the cap must reach share_notebook's own size check; got {status}: {body}"
    );
    assert!(
        no_share_written(dir.path()),
        "an over-size notebook wrote a share"
    );
}
