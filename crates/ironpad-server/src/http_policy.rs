//! HTTP serving policy: embed CORP headers, browser cache-control tiers
//! (immutable `/pkg/`, no-cache everything else — CLAUDE.md pitfall 6), and
//! the immutable `/share-blobs/{file}` handler with its name validation.
//!
//! Split out of `main.rs` (PRD-0055 T-003); behavior unchanged.

use axum::http::{HeaderName, HeaderValue};

use ironpad_server::state::AppState;

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
pub(crate) const CONTENT_SECURITY_POLICY: &str =
    "object-src 'none'; base-uri 'self'; form-action 'self'";

/// Is this a path we deliberately serve to third-party embedders? The embed
/// routes themselves plus the two loader scripts a host page pulls directly.
fn is_embeddable_path(path: &str) -> bool {
    path.starts_with("/embed/") || path == "/embed.js" || path == "/embed-frame.js"
}

/// Stamp `Cross-Origin-Resource-Policy: cross-origin` onto embeddable
/// responses so pages that are themselves COEP-isolated can still frame the
/// notebook and load `embed.js` (PRD-0039 T-006).
pub(crate) async fn embed_corp_header(
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
pub(crate) async fn share_blob_handler(
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
pub(crate) async fn cache_control_header(
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
