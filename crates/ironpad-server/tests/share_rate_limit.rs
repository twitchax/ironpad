//! Integration test: `share_notebook` charges each NEW share's bytes against
//! a per-client budget (review bug-security-8), asserted over raw HTTP
//! through the real server-fn handler.
//!
//! The share store never evicts, because an immutable link is frozen by
//! design, so without a price one anonymous client could fill it with
//! distinct uploads and refuse every later share for everyone. The unit tests
//! beside `share_notebook_core_capped` cover the seam; this covers the
//! wrapper: the admission read from context, the client identity read from
//! the headers, and that a harness providing ONLY an `AppConfig` (as the
//! body-limit test does) still shares instead of panicking.

use std::path::Path;

use axum::extract::Request;
use axum::routing::post;
use axum::Router;
use ironpad_app::compiler::admission::BuildAdmission;
use ironpad_app::server_fns::ShareNotebook;
use ironpad_common::{AppConfig, IronpadNotebook};
use leptos::server_fn::ServerFn as _;

/// The share endpoint mounted the way `leptos_routes_with_context` mounts
/// server fns, with an `AppConfig` and, when given, a `BuildAdmission`.
async fn serve(data_dir: &Path, admission: Option<BuildAdmission>) -> String {
    let config = AppConfig::for_tests(data_dir);
    let handler = move |req: Request| {
        let config = config.clone();
        let admission = admission.clone();
        async move {
            leptos_axum::handle_server_fns_with_context(
                move || {
                    leptos::prelude::provide_context(config.clone());
                    if let Some(admission) = admission.clone() {
                        leptos::prelude::provide_context(admission);
                    }
                },
                req,
            )
            .await
        }
    };
    let app = Router::new().route(ShareNotebook::PATH, post(handler));

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

fn notebook_json(title: &str) -> String {
    serde_json::to_string(&IronpadNotebook::new(title)).expect("notebook serializes")
}

/// POST one share as `client_ip` and return the status and body.
async fn share(base: &str, client_ip: &str, json: &str) -> (u16, String) {
    let res = reqwest::Client::new()
        .post(format!("{base}{}", ShareNotebook::PATH))
        .header("Fly-Client-IP", client_ip)
        .form(&[("notebook_json", json)])
        .send()
        .await
        .expect("request");
    let status = res.status().as_u16();
    (status, res.text().await.expect("body"))
}

#[tokio::test]
async fn a_share_without_admission_in_context_is_unmetered() {
    let data = tempfile::tempdir().expect("tempdir");
    let base = serve(data.path(), None).await;

    for title in ["One", "Two", "Three"] {
        let (status, body) = share(&base, "1.2.3.4", &notebook_json(title)).await;
        assert_eq!(status, 200, "{title}: {body}");
    }
}

#[tokio::test]
async fn new_shares_past_the_budget_are_refused_per_client() {
    let data = tempfile::tempdir().expect("tempdir");
    let first = notebook_json("First");
    let second = notebook_json("Second");
    // Room for one notebook's bytes, refilling at effectively nothing.
    #[allow(clippy::cast_precision_loss)]
    let burst = (first.len() + first.len() / 2) as f64;
    let admission = BuildAdmission::new(1, 1.0, 1.0, std::time::Duration::from_millis(50))
        .with_share_rate(burst, 0.0001);
    let base = serve(data.path(), Some(admission)).await;

    let (status, body) = share(&base, "1.2.3.4", &first).await;
    assert_eq!(status, 200, "the first share fits the budget: {body}");

    let (status, body) = share(&base, "1.2.3.4", &second).await;
    assert_ne!(status, 200, "a second new share is over budget");
    assert!(body.contains("rate limited"), "{body}");

    let (status, body) = share(&base, "1.2.3.4", &first).await;
    assert_eq!(status, 200, "a re-share adds nothing and is free: {body}");

    let (status, body) = share(&base, "5.6.7.8", &second).await;
    assert_eq!(status, 200, "another client has its own budget: {body}");
}
