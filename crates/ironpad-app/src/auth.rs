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

/// Resolve the current request's signed-in user, or `None` when there is no
/// (valid) session. Infallible by design — an anonymous request is the normal
/// case, and a DB hiccup must degrade to "not signed in", never to a 500 on
/// read paths.
pub async fn current_user(db: &Db) -> Option<AuthUser> {
    let headers = leptos_axum::extract::<http::HeaderMap>().await.ok()?;
    let cookie_header = headers.get(http::header::COOKIE)?.to_str().ok()?;
    let token = session_token_from_cookie_header(cookie_header)?;
    match db.session_user(token).await {
        Ok(user) => user,
        Err(e) => {
            tracing::warn!(error = %e, "session lookup failed; treating as anonymous");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
