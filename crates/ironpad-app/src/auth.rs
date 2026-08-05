//! Server-side session resolution for accounts (PRD-0053).
//!
//! The cookie name and parsing live here so the Axum auth routes
//! (ironpad-server) and the `#[server]` fns (this crate) agree on one
//! definition. Everything is ssr-only; the client learns about auth solely
//! through [`crate::server_fns::get_auth_info`].

use crate::db::{AuthUser, Db};

/// Session cookie name. `HttpOnly` + `Secure` + `SameSite=Lax`, set only by the
/// auth routes in ironpad-server.
pub const SESSION_COOKIE: &str = "ironpad_session";

/// Whether this deployment has GitHub OAuth configured at all. Provided as
/// leptos context by the server binary so `get_auth_info` can tell the client
/// to hide the sign-in surface entirely.
#[derive(Clone, Copy, Debug)]
pub struct AuthEnabled(pub bool);

/// Extract the session token from a `Cookie` request header value.
///
/// Hand-rolled on purpose: our token is plain hex (no quoting/encoding
/// concerns), and this avoids dragging a cookie crate into the app crate.
pub fn session_token_from_cookie_header(header: &str) -> Option<&str> {
    header.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == SESSION_COOKIE && !value.is_empty()).then_some(value)
    })
}

/// The `Set-Cookie` value minting (or re-issuing) a session cookie.
///
/// `Max-Age` is [`crate::db::SESSION_TTL_SECS`] — the cookie and the DB
/// record must expire together, and they were once maintained as two
/// independent 30-day constants in two crates.
pub fn session_cookie(token: &str) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; Max-Age={}; \
         HttpOnly; Secure; SameSite=Lax",
        crate::db::SESSION_TTL_SECS
    )
}

/// The `Set-Cookie` value deleting the session cookie (logout).
pub fn clear_session_cookie() -> String {
    format!("{SESSION_COOKIE}=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Lax")
}

/// Resolve the current request's signed-in user, or `None` when there is no
/// (valid) session. Infallible by design — an anonymous request is the normal
/// case, and a DB hiccup must degrade to "not signed in", never to a 500 on
/// read paths.
///
/// When the DB slid the session's expiry forward, the cookie is re-issued
/// with a fresh `Max-Age` on this response — the browser half of the sliding
/// 30-day expiry. (Without it the DB row renewed daily while the browser
/// deleted the cookie 30 days after LOGIN, signing active users out anyway.)
pub async fn current_user(db: &Db) -> Option<AuthUser> {
    let headers = leptos_axum::extract::<http::HeaderMap>().await.ok()?;
    let cookie_header = headers.get(http::header::COOKIE)?.to_str().ok()?;
    let token = session_token_from_cookie_header(cookie_header)?;
    match db.session_user(token).await {
        Ok(Some((user, renewed))) => {
            if renewed {
                refresh_session_cookie(token);
            }
            Some(user)
        }
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(error = %e, "session lookup failed; treating as anonymous");
            None
        }
    }
}

/// Re-issue the session cookie with a fresh `Max-Age` on the current leptos
/// response. A no-op outside a server-fn response context (e.g. the OG axum
/// handler): the refresh simply waits for the next server-fn call, which
/// `get_auth_info` makes on every page hydrate.
fn refresh_session_cookie(token: &str) {
    use leptos::prelude::use_context;
    let Some(response) = use_context::<leptos_axum::ResponseOptions>() else {
        return;
    };
    if let Ok(value) = http::HeaderValue::from_str(&session_cookie(token)) {
        response.append_header(http::header::SET_COOKIE, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cookie_carries_the_full_flag_set_and_the_db_ttl() {
        let cookie = session_cookie("deadbeef");
        assert!(cookie.starts_with("ironpad_session=deadbeef; "));
        for flag in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/"] {
            assert!(cookie.contains(flag), "missing {flag} in {cookie}");
        }
        // One TTL: the cookie's Max-Age IS the DB record's TTL.
        assert!(cookie.contains(&format!("Max-Age={}", crate::db::SESSION_TTL_SECS)));

        let cleared = clear_session_cookie();
        assert!(cleared.contains("Max-Age=0"));
        assert!(cleared.contains("HttpOnly"));
    }

    #[test]
    fn cookie_parsing_finds_the_session_among_others() {
        assert_eq!(
            session_token_from_cookie_header("a=1; ironpad_session=deadbeef; b=2"),
            Some("deadbeef")
        );
        assert_eq!(
            session_token_from_cookie_header("ironpad_session=abc"),
            Some("abc")
        );
        // Missing, empty, or lookalike names resolve to nothing.
        assert_eq!(session_token_from_cookie_header("a=1; b=2"), None);
        assert_eq!(session_token_from_cookie_header("ironpad_session="), None);
        assert_eq!(
            session_token_from_cookie_header("xironpad_session=abc"),
            None
        );
    }
}
