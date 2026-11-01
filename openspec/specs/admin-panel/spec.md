# admin-panel Specification

## Purpose
`/admin`, a panel for the single operator of an ironpad instance: instance state, the list of users with session revocation, and manual control of the compile-cache tiers with each action's cost stated first. The surface does not exist unless the operator names an admin, and to everyone else it is indistinguishable from an unknown route.

## Requirements
### Requirement: Opt-in admin identity
The admin surface SHALL exist only when `IRONPAD_ADMIN_LOGIN` (flag `--admin-login`) names a GitHub login. It SHALL be unset by default, an empty value SHALL match nobody, and it SHALL be allowed alongside `IRONPAD_TEST_AUTH`. (PRD-0063 T-001)

#### Scenario: Not configured
- **WHEN** the server runs without `IRONPAD_ADMIN_LOGIN`
- **THEN** `/admin` is an ordinary not-found for every visitor, signed in or not, and every admin server function returns nothing

#### Scenario: Configured
- **WHEN** the server starts with `IRONPAD_ADMIN_LOGIN=twitchax`
- **THEN** it logs "admin surface enabled" with that login

#### Scenario: Together with test login
- **WHEN** both `IRONPAD_ADMIN_LOGIN=ironpad-admin` and `IRONPAD_TEST_AUTH` are set
- **THEN** the server starts, and signing in through `/auth/test-login?login=ironpad-admin` reaches the panel

### Requirement: Who counts as the admin
A request SHALL count as the admin only when it carries a valid signed-in session whose GitHub login equals the configured login ignoring case, and whose GitHub id equals the id pinned by the first session that matched. There SHALL be no other credential, and a failure to read or record the pin SHALL deny. (PRD-0063 T-001 T-002)

#### Scenario: Case-insensitive login
- **WHEN** the configured login is `Twitchax` and user `twitchax` signs in
- **THEN** they are the admin

#### Scenario: Look-alike logins
- **WHEN** users `twitchax2`, `itchax`, or `octocat` sign in and the configured login is `twitchax`
- **THEN** none of them is the admin

#### Scenario: A renamed handle claimed by someone else
- **WHEN** the admin's GitHub id is pinned and a different GitHub account now holding the same login signs in
- **THEN** that account is denied and a warning is logged

### Requirement: Denial looks like an unknown route
`/admin` SHALL answer anyone who is not the admin with HTTP 404 and the body "Page not found.", the same as any unknown route, and none of the panel's data SHALL appear in that response. (PRD-0063 T-001 T-004)

#### Scenario: Anonymous visitor
- **WHEN** an anonymous visitor opens `/admin`
- **THEN** the status is 404, the page says "Page not found.", and no panel renders

#### Scenario: Signed-in non-admin
- **WHEN** signed-in user `octocat` opens `/admin`
- **THEN** the status is 404, never 200 or 403
- **AND** the raw response body contains none of the panel's stats markup, the label "Published notebooks", or the tier name `cargo-home`

### Requirement: Gated admin server functions
Every admin server function (`admin_overview`, `admin_list_users`, `admin_revoke_user_sessions`, `admin_wipe_cache_tier`) SHALL return an empty result to any non-admin caller, identical for anonymous and signed-in callers, and SHALL report a real failure as an error only after the caller passed the gate. (PRD-0063 T-001)

#### Scenario: Every function is gated
- **WHEN** the unit suite runs
- **THEN** it fails if any `pub async fn admin_*` server function does not check the admin gate before doing anything

#### Scenario: A broken database
- **WHEN** the admin opens the panel and instance state cannot be read
- **THEN** the panel says "Could not read instance state: " followed by the error, not "Page not found."

### Requirement: Instance overview
The panel SHALL show an "Instance" section with the number of users, a session count, a notebook count, and the on-disk size of the accounts database, all measured when the page is requested. (PRD-0063 T-004)

#### Scenario: The admin opens the panel
- **WHEN** the configured admin opens `/admin`
- **THEN** an "Instance" heading shows a user count, a session count, a notebook count, and the accounts database size

#### Scenario: Byte sizes
- **WHEN** a size is displayed
- **THEN** 0 reads "0 B", 1023 reads "1023 B", 1024 reads "1.0 KB", 1,048,575 reads "1.0 MB", 3,435,973,836 reads "3.2 GB", and 166 MiB reads "166 MB"

### Requirement: Compile cache tiers
The panel SHALL list the four compile-cache tiers, `targets`, `workspaces`, `cargo-home`, and `blobs`, each with its current size and whether the boot-time pressure valve may clear it: "under disk pressure", or "never" for `blobs`. (PRD-0063 T-006)

#### Scenario: Tier table
- **WHEN** the admin opens the panel
- **THEN** the cache table has exactly four rows, and the `blobs` row says "never"

### Requirement: Clearing a tier
Each tier's "Clear" button SHALL first confirm with the tier's name, measured size, what is lost, and "This cannot be undone.", then delete only that tier under the cache directory, report the bytes freed, and refresh the sizes. It SHALL be disabled, titled "Nothing to clear", when the tier is empty. (PRD-0063 T-006)

#### Scenario: Confirm states the cost
- **WHEN** the admin clicks Clear on `workspaces`
- **THEN** the confirm reads "Clear the workspaces cache ({size})?", says "Cells will recompile from scratch until it is rebuilt.", and ends "This cannot be undone."
- **AND** dismissing it clears nothing

#### Scenario: Blobs warns about readers
- **WHEN** the admin clicks Clear on a non-empty `blobs` tier
- **THEN** the confirm says "Every compiled cell is discarded. Readers will wait for a cold compile on notebooks that are currently instant."

#### Scenario: Only the named tier
- **WHEN** the admin clears `targets`
- **THEN** the `targets` tier is empty, the bytes freed are reported, and `blobs` keeps its contents
- **AND** clearing a tier that does not exist frees 0 bytes without an error

#### Scenario: No arbitrary paths
- **WHEN** a caller sends a tier that is not one of the four names
- **THEN** the request is rejected, since a tier is chosen from a fixed set and never names a path

### Requirement: User list
The panel SHALL list every user who has signed in, oldest first, with their login, session count, a count of their notebooks, and their sign-up date as YYYY-MM-DD; with no users it SHALL say "No one has signed in yet." (PRD-0063 T-005)

#### Scenario: A user with nothing
- **WHEN** a user has signed in once, then signed out, and owns no notebooks
- **THEN** their row still appears, with 0 sessions and 0 notebooks

#### Scenario: Counts
- **WHEN** user `revoke-me` holds one live session
- **THEN** their row shows 1 session

### Requirement: Revoking a user's sessions
Each user row's "Revoke sessions" button SHALL, after the confirm "Sign {login} out of {n} session(s)?" noting that notebooks and shares are untouched, delete every session of that user, keyed by GitHub id rather than login, and refresh the list. It SHALL be disabled, titled "No active sessions", when the user has none. (PRD-0063 T-005)

#### Scenario: Revoke
- **WHEN** the admin revokes `revoke-me`'s sessions
- **THEN** the row's session count becomes 0
- **AND** `revoke-me`'s next request is signed out, while other users stay signed in and `revoke-me`'s notebooks and grants are untouched

#### Scenario: The admin's own sessions
- **WHEN** the admin revokes their own sessions
- **THEN** they are signed out of the panel like anyone else

### Requirement: Kept out of search
`/admin` SHALL carry the title "Admin · ironpad" and `<meta name="robots" content="noindex, nofollow">`; `robots.txt` disallows it as `social-previews` describes. (PRD-0063 T-004)

#### Scenario: Crawlers
- **WHEN** the admin opens `/admin`
- **THEN** the page's robots meta contains `noindex`
