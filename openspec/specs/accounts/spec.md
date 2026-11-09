# accounts Specification

## Purpose
GitHub sign-in for an ironpad instance, which exists so notebooks stored on the server can have an owner. Covers the OAuth login and logout routes, the session cookie and its sliding expiry, the anonymous-only mode an instance runs in when OAuth is not configured, the env-gated test login used by e2e suites, the header's identity display, and the boundary of what stays fully anonymous.

## Requirements
### Requirement: Sign-in exists only when OAuth is configured
Sign-in SHALL exist only when both `GITHUB_CLIENT_ID` and `GITHUB_CLIENT_SECRET` (flags `--github-client-id`, `--github-client-secret`) are set. Otherwise the instance SHALL run anonymous-only: no sign-in control renders, and `/auth/github` and `/auth/callback` answer 404 "sign-in is not configured". (PRD-0053)

#### Scenario: Neither credential set
- **WHEN** the server starts with no GitHub OAuth credentials
- **THEN** `get_auth_info` reports sign-in disabled and the header shows no sign-in control
- **AND** `GET /auth/github` answers 404

#### Scenario: Only one credential set
- **WHEN** exactly one of the two credentials is set
- **THEN** the server logs "one of GITHUB_CLIENT_ID/GITHUB_CLIENT_SECRET is set without the other; sign-in disabled" and runs anonymous-only

### Requirement: Starting the OAuth flow
`GET /auth/github?redirect_to={path}` SHALL answer 303 to `https://github.com/login/oauth/authorize` with the app's client id, `redirect_uri` `{public_url}/auth/callback`, and a `state` holding a fresh 128-bit hex nonce plus the return path. It SHALL set that nonce in an `ironpad_oauth_state` cookie with `Path=/auth; Max-Age=600; HttpOnly; Secure; SameSite=Lax`. (PRD-0053)

#### Scenario: Sign-in link
- **WHEN** a client requests `/auth/github?redirect_to=/local/abc`
- **THEN** the response is 303 to the GitHub authorize URL with `client_id` set
- **AND** an `ironpad_oauth_state` cookie scoped to `/auth` is set

#### Scenario: Public URL with a trailing slash
- **WHEN** the public URL is configured as `http://localhost:3111/`
- **THEN** the `redirect_uri` sent to GitHub is exactly `http://localhost:3111/auth/callback`

### Requirement: Completing the OAuth flow
`GET /auth/callback` SHALL require the nonce in `state` to equal the `ironpad_oauth_state` cookie, exchange the code with GitHub, and fetch the identity; it SHALL then store only the GitHub user id, login, and avatar URL, mint a session, clear the nonce cookie, and redirect 303 to the return path. (PRD-0053)

#### Scenario: Request errors
- **WHEN** the callback lacks `code` or `state`, carries a `state` with no `|` separator, or has a nonce that does not match the cookie
- **THEN** it answers 400 "missing code or state", "malformed state", or "state mismatch" respectively, before contacting GitHub

#### Scenario: GitHub refuses or fails
- **WHEN** GitHub declines the code exchange
- **THEN** the callback answers 400 "GitHub declined the sign-in"
- **AND** when GitHub cannot be reached or returns garbage, it answers 502 "GitHub token exchange failed" or "GitHub user lookup failed"

#### Scenario: A returning user
- **WHEN** a user who renamed their GitHub account or changed their avatar signs in again
- **THEN** their stored login and avatar update and their original sign-up time is kept

### Requirement: Post-login redirects stay on the site
Every sign-in SHALL redirect only to a local absolute path; any other return target SHALL land on `/`. (PRD-0053)

#### Scenario: Local path kept
- **WHEN** the return path is `/local/abc`
- **THEN** sign-in lands on `/local/abc`

#### Scenario: Off-site or malformed targets
- **WHEN** the return path is `https://evil.example`, `//evil.example`, `/\evil.example`, `javascript:alert(1)`, empty, or contains a control character such as a newline
- **THEN** sign-in lands on `/`

### Requirement: Session cookie
A successful sign-in SHALL set an `ironpad_session` cookie holding a random 256-bit token written as 64 hex characters, with `Path=/; Max-Age=2592000; HttpOnly; Secure; SameSite=Lax`. The server SHALL store only a hash of the token, so the database holds no usable cookie value. (PRD-0053)

#### Scenario: Cookie flags on the wire
- **WHEN** a sign-in completes
- **THEN** the 303 response sets `ironpad_session=` with `HttpOnly`, `Secure`, `SameSite=Lax`, and `Path=/`

#### Scenario: Unknown token
- **WHEN** a request carries an `ironpad_session` value that no session matches
- **THEN** the request is treated as signed out

### Requirement: Sliding 30-day expiry
A session SHALL expire 30 days after its last renewal. It SHALL renew at most once per 12 hours of use and SHALL re-issue the cookie with a fresh `Max-Age` when it does; an expired session SHALL resolve as signed out and be deleted, and a failure looking a session up SHALL degrade to signed out rather than an error. (PRD-0053)

#### Scenario: Renewal
- **WHEN** a request arrives with a session more than 12 hours past its last renewal
- **THEN** its expiry moves to 30 days from now and the response re-sets the cookie
- **AND** an immediately following request does not renew it again

#### Scenario: Expired
- **WHEN** a request carries a session past its expiry
- **THEN** it is treated as signed out and the session is removed
- **AND** expired sessions are also swept when the server boots

### Requirement: Sign out
`GET /auth/logout` SHALL delete the caller's session, clear the cookie with `Max-Age=0`, and redirect 303 to `/`, also when the request carries no session. Only a top-level navigation may sign out: a request whose `Sec-Fetch-Dest` is present and not `document`, or whose `Sec-Fetch-Site` is present and not `same-origin` or `none`, SHALL get 403 with no session deleted and no `Set-Cookie`. A request carrying neither header (a non-browser client) SHALL sign out as before. (PRD-0053)

#### Scenario: Header sign-out
- **WHEN** a signed-in user clicks "Sign out" in the header
- **THEN** they land on the home page signed out, with the "Sign in" control back

#### Scenario: No session
- **WHEN** `/auth/logout` is requested without a session cookie
- **THEN** the response is still 303 with a clearing `Set-Cookie` carrying `Max-Age=0`

#### Scenario: A notebook cannot sign its reader out
- **WHEN** a signed-in user views a shared notebook whose markdown embeds `![](/auth/logout)`
- **THEN** the image request gets 403 and the user is still signed in after a reload
- **AND** a `fetch` (`Sec-Fetch-Dest: empty`) or a cross-site navigation to `/auth/logout` gets 403 too

### Requirement: Env-gated test login
When `IRONPAD_TEST_AUTH` (flag `--test-auth`) is truthy, `GET /auth/test-login?login={name}` SHALL sign in a synthetic user through the same session path as OAuth and honour `redirect_to`; when it is not set, the route SHALL not exist. The server SHALL log a warning at startup whenever it is set. (PRD-0053, PRD-0063 T-001)

#### Scenario: Gate off
- **WHEN** the server runs without `IRONPAD_TEST_AUTH`
- **THEN** `/auth/test-login?login=alice` answers 404

#### Scenario: Gate on
- **WHEN** the server runs with `IRONPAD_TEST_AUTH=1` and a client requests `/auth/test-login?login=alice`
- **THEN** the response is 303 with an `ironpad_session` cookie carrying the full flag set
- **AND** the session belongs to login `alice` with GitHub id `test-alice` and no avatar

#### Scenario: Bad logins
- **WHEN** `login` is missing, longer than 39 characters, or contains anything but ASCII letters, digits, and `-`
- **THEN** the route answers 400 "missing login" or "invalid login"

#### Scenario: Startup warning
- **WHEN** the server starts with `IRONPAD_TEST_AUTH` set
- **THEN** it warns that `/auth/test-login` will mint a session for any user, including the admin, and must never run in production

### Requirement: Auth info for the client
`get_auth_info` SHALL report whether sign-in is configured on this instance and, when the request carries a valid session, the user's login and avatar URL, never their GitHub id. (PRD-0053)

#### Scenario: Signed in
- **WHEN** a signed-in page hydrates
- **THEN** `get_auth_info` returns sign-in enabled with the user's login and avatar URL

#### Scenario: Anonymous
- **WHEN** an anonymous page hydrates on an instance with OAuth configured
- **THEN** `get_auth_info` returns sign-in enabled with no user

### Requirement: Header identity display
The page header SHALL show, on every page including home, a "Sign in" link when signed out and the user's portrait over a "Sign out" link when signed in, and SHALL show nothing while auth is resolving or when sign-in is not configured. (PRD-0053)

#### Scenario: Signed out
- **WHEN** an anonymous visitor views a page on an instance with OAuth configured
- **THEN** the header shows a silhouette over "Sign in", titled "Sign in with GitHub", linking to `/auth/github?redirect_to={current path}`

#### Scenario: Signed in
- **WHEN** user `carol` is signed in
- **THEN** the header shows her avatar, or a silhouette when she has none, over "Sign out", linking to `/auth/logout`
- **AND** the link's tooltip is "@carol" and its accessible name is "Sign out of @carol"

### Requirement: What stays anonymous
Local notebooks, immutable shares, public notebooks, agent sessions, and compilation SHALL work without signing in. Only storing a notebook in an account, publishing it, and managing its access SHALL require a session. (PRD-0053)

#### Scenario: Signed out
- **WHEN** a signed-out user creates a notebook and adds a cell
- **THEN** both work with no sign-in prompt

#### Scenario: The account boundary
- **WHEN** a signed-out user picks Share Mutable
- **THEN** a toast says "sign in with GitHub to save or publish notebooks"
- **AND** Share Immutable from the same notebook succeeds

### Requirement: Accounts database
The server SHALL keep users, sessions, account notebooks, and access grants in one embedded database at `{data_dir}/ironpad.db`, opened and schema-checked once at boot; it SHALL store no GitHub credential or access token. (PRD-0053)

#### Scenario: First boot
- **WHEN** the server starts against an empty data directory
- **THEN** it creates `{data_dir}/ironpad.db` and logs "accounts database open" before serving

#### Scenario: Restart
- **WHEN** the server restarts against an existing database
- **THEN** existing users, sessions, and notebooks are still there
