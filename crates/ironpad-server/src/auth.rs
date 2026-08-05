//! GitHub OAuth sign-in + session cookie routes (PRD-0053).
//!
//! Mounted under `/auth` via `nest_service` with its own state — the DB
//! travels here and in leptos context, deliberately not in `AppState` (see
//! `main.rs`). Sign-in is delegated wholesale: GitHub owns credentials, 2FA,
//! and recovery; this module owns only the authorization-code dance and the
//! session cookie.
//!
//! The `/auth/test-login` route exists ONLY when the server was started with
//! `IRONPAD_TEST_AUTH` — it mints a real user + session through the same code
//! paths as OAuth so e2e can exercise the whole RBAC surface. The production
//! config never sets it, and `router_gating` tests assert absence.

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{AppendHeaders, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;

use ironpad_app::auth::{clear_session_cookie, session_cookie, SESSION_COOKIE};
use ironpad_app::db::Db;

/// Short-lived CSRF nonce cookie for the OAuth round trip. Scoped to `/auth`
/// so it never rides along on normal page requests.
const OAUTH_STATE_COOKIE: &str = "ironpad_oauth_state";

// ── State ───────────────────────────────────────────────────────────────────

/// GitHub OAuth app credentials. Absent (in [`AuthState`]) means sign-in is
/// hidden and this instance runs anonymous-only.
#[derive(Clone)]
pub struct GithubOauth {
    pub client_id: String,
    pub client_secret: String,
}

/// State for the auth router.
#[derive(Clone)]
pub struct AuthState {
    pub db: Db,
    pub github: Option<GithubOauth>,
    /// Registers `/test-login` when true. e2e only.
    pub test_auth: bool,
    /// Public origin for the OAuth `redirect_uri`.
    pub public_url: String,
    pub http: reqwest::Client,
}

/// Build the auth router (state applied). Mount with
/// `.nest_service("/auth", ...)` — paths here are prefix-stripped.
pub fn router(state: AuthState) -> Router {
    let mut router = Router::new()
        .route("/github", get(github_redirect))
        .route("/callback", get(github_callback))
        .route("/logout", get(logout));
    if state.test_auth {
        router = router.route("/test-login", get(test_login));
    }
    router.with_state(state)
}

// ── Handlers ────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct RedirectParams {
    redirect_to: Option<String>,
}

/// Kick off the OAuth dance: set the CSRF nonce cookie and bounce to GitHub.
#[tracing::instrument(name = "auth_github", level = "info", skip_all)]
async fn github_redirect(
    State(state): State<AuthState>,
    Query(params): Query<RedirectParams>,
) -> Response {
    let Some(github) = &state.github else {
        return (StatusCode::NOT_FOUND, "sign-in is not configured").into_response();
    };

    let nonce = random_nonce();
    let return_path = safe_redirect_path(params.redirect_to.as_deref());
    // The nonce travels in BOTH the state param and a cookie; the callback
    // requires them to match, which is what defeats login CSRF. The return
    // path rides inside the state param (server-validated on the way back).
    let state_param = format!("{nonce}|{return_path}");

    let url = match reqwest::Url::parse_with_params(
        "https://github.com/login/oauth/authorize",
        &[
            ("client_id", github.client_id.as_str()),
            (
                "redirect_uri",
                &format!("{}/auth/callback", state.public_url),
            ),
            ("state", &state_param),
        ],
    ) {
        Ok(url) => url,
        Err(e) => {
            tracing::error!(error = %e, "failed to build authorize URL");
            return (StatusCode::INTERNAL_SERVER_ERROR, "authorize URL").into_response();
        }
    };

    (
        AppendHeaders([(header::SET_COOKIE, oauth_state_cookie(&nonce, 600))]),
        Redirect::to(url.as_str()),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error_description: Option<String>,
}

#[derive(serde::Deserialize)]
struct GithubUser {
    id: u64,
    login: String,
    avatar_url: Option<String>,
}

/// OAuth callback: verify the CSRF nonce, exchange the code, upsert the user,
/// mint a session, and land back where the user started.
#[tracing::instrument(name = "auth_callback", level = "info", skip_all)]
async fn github_callback(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Response {
    let Some(github) = &state.github else {
        return (StatusCode::NOT_FOUND, "sign-in is not configured").into_response();
    };
    let (Some(code), Some(state_param)) = (params.code, params.state) else {
        return (StatusCode::BAD_REQUEST, "missing code or state").into_response();
    };

    // CSRF check: the state param's nonce must match the cookie we set.
    let Some((nonce, return_path)) = state_param.split_once('|') else {
        return (StatusCode::BAD_REQUEST, "malformed state").into_response();
    };
    if cookie_value(&headers, OAUTH_STATE_COOKIE) != Some(nonce) {
        return (StatusCode::BAD_REQUEST, "state mismatch").into_response();
    }
    let return_path = safe_redirect_path(Some(return_path));

    // Exchange the code for an access token.
    let token: TokenResponse = match state
        .http
        .post("https://github.com/login/oauth/access_token")
        .header(header::ACCEPT, "application/json")
        .form(&[
            ("client_id", github.client_id.as_str()),
            ("client_secret", github.client_secret.as_str()),
            ("code", &code),
        ])
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
    {
        Ok(res) => match res.json().await {
            Ok(token) => token,
            Err(e) => {
                tracing::error!(error = %e, "token response malformed");
                return (StatusCode::BAD_GATEWAY, "GitHub token exchange failed").into_response();
            }
        },
        Err(e) => {
            tracing::error!(error = %e, "token exchange request failed");
            return (StatusCode::BAD_GATEWAY, "GitHub token exchange failed").into_response();
        }
    };
    let Some(access_token) = token.access_token else {
        tracing::warn!(
            detail = token.error_description.as_deref().unwrap_or("unknown"),
            "GitHub declined the code exchange"
        );
        return (StatusCode::BAD_REQUEST, "GitHub declined the sign-in").into_response();
    };

    // Fetch the identity. User-Agent is mandatory on api.github.com.
    let user: GithubUser = match state
        .http
        .get("https://api.github.com/user")
        .header(header::ACCEPT, "application/vnd.github+json")
        .header(header::USER_AGENT, "ironpad")
        .bearer_auth(&access_token)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
    {
        Ok(res) => match res.json().await {
            Ok(user) => user,
            Err(e) => {
                tracing::error!(error = %e, "user response malformed");
                return (StatusCode::BAD_GATEWAY, "GitHub user lookup failed").into_response();
            }
        },
        Err(e) => {
            tracing::error!(error = %e, "user lookup request failed");
            return (StatusCode::BAD_GATEWAY, "GitHub user lookup failed").into_response();
        }
    };

    finish_login(
        &state.db,
        &user.id.to_string(),
        &user.login,
        user.avatar_url.as_deref().unwrap_or(""),
        &return_path,
    )
    .await
}

/// Delete the session and clear the cookie.
#[tracing::instrument(name = "auth_logout", level = "info", skip_all)]
async fn logout(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie_value(&headers, SESSION_COOKIE) {
        if let Err(e) = state.db.delete_session(token).await {
            tracing::warn!(error = %e, "session delete failed during logout");
        }
    }
    (
        AppendHeaders([(header::SET_COOKIE, clear_session_cookie())]),
        Redirect::to("/"),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
struct TestLoginParams {
    login: Option<String>,
    redirect_to: Option<String>,
}

/// e2e-only login: mints a real user + session through the same paths as
/// OAuth. Registered ONLY when [`AuthState::test_auth`] is set.
#[tracing::instrument(name = "auth_test_login", level = "info", skip_all)]
async fn test_login(
    State(state): State<AuthState>,
    Query(params): Query<TestLoginParams>,
) -> Response {
    let Some(login) = params.login else {
        return (StatusCode::BAD_REQUEST, "missing login").into_response();
    };
    // GitHub's own username rule; also keeps the synthetic record key tame.
    let valid = !login.is_empty()
        && login.len() <= 39
        && login
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if !valid {
        return (StatusCode::BAD_REQUEST, "invalid login").into_response();
    }

    // A `test-` prefixed id can never collide with a real (numeric) GitHub id.
    let github_id = format!("test-{login}");
    let return_path = safe_redirect_path(params.redirect_to.as_deref());
    finish_login(&state.db, &github_id, &login, "", &return_path).await
}

/// Shared tail of every login: upsert the user, mint a session, set the
/// cookie (clearing any OAuth nonce), and redirect into the app.
async fn finish_login(
    db: &Db,
    github_id: &str,
    login: &str,
    avatar_url: &str,
    return_path: &str,
) -> Response {
    if let Err(e) = db.upsert_user(github_id, login, avatar_url).await {
        tracing::error!(error = %e, "user upsert failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed").into_response();
    }
    let token = match db.create_session(github_id).await {
        Ok(token) => token,
        Err(e) => {
            tracing::error!(error = %e, "session create failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed").into_response();
        }
    };
    tracing::info!(login = %login, "signed in");
    (
        AppendHeaders([
            (header::SET_COOKIE, session_cookie(&token)),
            (header::SET_COOKIE, oauth_state_cookie("", 0)),
        ]),
        Redirect::to(return_path),
    )
        .into_response()
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Constrain a post-login redirect to a local path: absolute-path only, and
/// not scheme-relative (`//evil.example`). Anything else lands home.
fn safe_redirect_path(path: Option<&str>) -> String {
    match path {
        Some(p)
            if p.starts_with('/')
                && !p.starts_with("//")
                && !p.contains('\\')
                && !p.contains("/\\") =>
        {
            p.to_string()
        }
        _ => "/".to_string(),
    }
}

// The session cookie itself (mint/clear) lives in `ironpad_app::auth` — one
// definition shared with the sliding-expiry re-issue path in `current_user`.
// `Secure` is safe on localhost too: every modern browser treats it as a
// trustworthy origin.

fn oauth_state_cookie(nonce: &str, max_age_secs: u32) -> String {
    format!(
        "{OAUTH_STATE_COOKIE}={nonce}; Path=/auth; Max-Age={max_age_secs}; \
         HttpOnly; Secure; SameSite=Lax"
    )
}

/// Read one cookie's value from request headers.
fn cookie_value<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|pair| {
        let (n, v) = pair.trim().split_once('=')?;
        (n == name && !v.is_empty()).then_some(v)
    })
}

/// 128-bit random nonce as hex for the OAuth state.
fn random_nonce() -> String {
    use rand::RngCore as _;
    use std::fmt::Write as _;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().fold(String::with_capacity(32), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt as _;

    #[test]
    fn safe_redirect_rejects_offsite_targets() {
        assert_eq!(safe_redirect_path(Some("/local/abc")), "/local/abc");
        assert_eq!(safe_redirect_path(Some("/")), "/");
        assert_eq!(safe_redirect_path(None), "/");
        assert_eq!(safe_redirect_path(Some("")), "/");
        assert_eq!(safe_redirect_path(Some("https://evil.example")), "/");
        assert_eq!(safe_redirect_path(Some("//evil.example")), "/");
        assert_eq!(safe_redirect_path(Some("/\\evil.example")), "/");
        assert_eq!(safe_redirect_path(Some("javascript:alert(1)")), "/");
    }

    #[test]
    fn cookie_helpers_carry_the_full_flag_set() {
        let c = session_cookie("tok");
        for flag in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/"] {
            assert!(c.contains(flag), "session cookie missing {flag}: {c}");
        }
        assert!(clear_session_cookie().contains("Max-Age=0"));
        assert!(oauth_state_cookie("n", 600).contains("Path=/auth"));
    }

    async fn test_state(test_auth: bool, github: bool) -> (tempfile::TempDir, AuthState) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("auth-test.db")).await.unwrap();
        let state = AuthState {
            db,
            github: github.then(|| GithubOauth {
                client_id: "id".into(),
                client_secret: "secret".into(),
            }),
            test_auth,
            public_url: "http://localhost:3111".to_string(),
            http: reqwest::Client::new(),
        };
        (dir, state)
    }

    async fn get_response(router: Router, uri: &str) -> axum::response::Response {
        router
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// The uat-006 anchor: `/auth/test-login` must not exist unless the env
    /// gate was set, and when it does exist it mints a real session cookie
    /// with the full flag set.
    #[tokio::test]
    async fn test_login_route_is_env_gated() {
        // Gate off: the route is absent entirely (404 from the router).
        let (_dir, state) = test_state(false, false).await;
        let res = get_response(router(state), "/test-login?login=alice").await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        // Gate on: a valid login mints a session and redirects.
        let (_dir2, state) = test_state(true, false).await;
        let r = router(state.clone());
        let res = get_response(r.clone(), "/test-login?login=alice").await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        let cookie = res
            .headers()
            .get(header::SET_COOKIE)
            .expect("session cookie set")
            .to_str()
            .unwrap();
        assert!(cookie.starts_with("ironpad_session="));
        for flag in ["HttpOnly", "Secure", "SameSite=Lax"] {
            assert!(cookie.contains(flag), "missing {flag}: {cookie}");
        }
        // The minted session resolves to the synthetic user.
        let token = cookie
            .strip_prefix("ironpad_session=")
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        let (user, _) = state.db.session_user(token).await.unwrap().unwrap();
        assert_eq!(user.login, "alice");
        assert_eq!(user.github_id, "test-alice");

        // Garbage logins are rejected before touching the DB.
        let res = get_response(r.clone(), "/test-login?login=..%2Fetc").await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let res = get_response(r, "/test-login").await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn github_redirect_requires_configuration() {
        // No OAuth credentials: the sign-in surface answers 404.
        let (_dir, state) = test_state(false, false).await;
        let res = get_response(router(state), "/github").await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        // Configured: bounce to GitHub with the nonce cookie set.
        let (_dir2, state) = test_state(false, true).await;
        let res = get_response(router(state), "/github?redirect_to=/local/abc").await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        let location = res
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.starts_with("https://github.com/login/oauth/authorize"));
        assert!(location.contains("client_id=id"));
        let cookie = res
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.starts_with("ironpad_oauth_state="));
        assert!(cookie.contains("Path=/auth"));
    }

    #[tokio::test]
    async fn callback_rejects_a_state_mismatch() {
        let (_dir, state) = test_state(false, true).await;
        // No nonce cookie at all → mismatch → 400 before any network I/O.
        let res = get_response(
            router(state),
            "/callback?code=abc&state=deadbeef%7C%2Flocal%2Fx",
        )
        .await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn logout_clears_the_cookie_even_without_a_session() {
        let (_dir, state) = test_state(false, false).await;
        let res = get_response(router(state), "/logout").await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        let cookie = res
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("Max-Age=0"));
    }
}
