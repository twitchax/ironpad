---
id: PRD-0063
title: "Single-admin operations panel: instance state, user list, and cache tiers"
status: draft
owner: "Aaron Roney"
created: 2026-08-11
updated: 2026-08-11

depends_on:
- PRD-0053

principles:
- "The env var names WHO is privileged, never HOW to authenticate. Admin routes require a real signed-in session (PRD-0053 cookie) whose GitHub identity matches IRONPAD_ADMIN_LOGIN. A bearer token in a header or query string was considered and rejected: it leaks into logs, browser history, and referrers, and it would be the only credential in the app that bypasses OAuth."
- "Unset means the surface does not exist. With no IRONPAD_ADMIN_LOGIN, /admin returns the ordinary not-found and no admin server fn is reachable, exactly as the sign-in surface disappears when GITHUB_CLIENT_ID is absent. Contributor and CI instances stay clean by construction rather than by remembering to lock a door."
- "Denial is a 404, not a 403. A 403 confirms the panel exists and invites attention; a non-admin sees what any visitor to an unknown route sees. This deliberately differs from PRD-0061, where an explicit denial was correct because a private share link handed to the right person must work after one sign-in. Nobody is meant to arrive at /admin by invitation."
- "One gate, called first, everywhere. `admin_user()` is the single predicate, mirroring mutable_access_core and private_share_readable. These are the most destructive server fns in the app, so the coverage test enumerates them and asserts each one rejects both anonymous and signed-in-non-admin callers, rather than trusting that the tenth function remembered."
- "Identity pins to github_id, not the login string. db.rs already notes that logins can be renamed on GitHub; a renamed handle is freed for anyone to claim, and a squatter would then match a login allowlist with a different github_id. The configured value stays a readable login, and the resolved github_id is pinned on first match so a rename fails closed instead of transferring admin."
- "IRONPAD_TEST_AUTH and IRONPAD_ADMIN_LOGIN are mutually exclusive at startup. /auth/test-login mints a session for an arbitrary user, so together they are a complete bypass of the gate. Prod never sets test auth, but that is a habit; this makes it an assertion the process refuses to start without."
- "Destructive actions state their cost before they run. Wiping `targets` (3.2GB) means every cell recompiles cold, and wiping `blobs` (166MB, 1,915 entries) is what stands between readers and a cold compile. The confirm names the tier, its measured size, and what users lose, because the panel exists to operate the instance, not to make an irreversible action one click away."

references:
- name: "Session and identity (AuthUser, current_user)"
  url: crates/ironpad-app/src/auth.rs
- name: "Accounts DB (user, session, rbac_grant, meta)"
  url: crates/ironpad-app/src/db.rs
- name: "Server fn surface + existing gate patterns"
  url: crates/ironpad-app/src/server_fns.rs
- name: "Cache tiers and the pressure valve"
  url: crates/ironpad-server/src/cache_valve.rs
- name: "Config (clap + env)"
  url: crates/ironpad-server/src/config.rs
- name: "Route table and SsrMode choices"
  url: crates/ironpad-app/src/lib.rs

acceptance_tests:
- id: uat-001
  name: "With IRONPAD_ADMIN_LOGIN unset, /admin is not found and every admin server fn rejects, including for a signed-in user"
  command: cargo make uat
  uat_status: unverified
- id: uat-002
  name: "With it set, the named admin sees the panel; an anonymous visitor and a signed-in non-admin both get the ordinary not-found, never a 403"
  command: cargo make uat
  uat_status: unverified
- id: uat-003
  name: "Every admin server fn rejects anonymous and non-admin callers (enumerated, so a new fn added without the gate fails the suite)"
  command: cargo make uat
  uat_status: unverified
- id: uat-004
  name: "The server refuses to start when IRONPAD_TEST_AUTH and IRONPAD_ADMIN_LOGIN are both set"
  command: cargo make test
  uat_status: unverified
- id: uat-005
  name: "Revoking a user's sessions signs that user out on their next request and leaves other users signed in"
  command: cargo make uat
  uat_status: unverified
- id: uat-006
  name: "A cache tier wipe removes only the named tier, reports the bytes freed, and leaves the others intact"
  command: cargo make test
  uat_status: unverified

tasks:
- id: T-001
  title: "Config + the one gate"
  priority: 1
  status: todo
  notes: "AppConfig.admin_login: Option<String> (IRONPAD_ADMIN_LOGIN). admin_user(db, config) -> Option<AuthUser> in auth.rs: requires current_user AND a case-insensitive login match (GitHub logins are case-insensitive). Startup refuses when test_auth && admin_login.is_some(). Unit tests for the predicate: unset, match, mismatch, anonymous, case difference."
- id: T-002
  title: "Pin the admin github_id on first match"
  priority: 1
  status: todo
  notes: "Store the resolved github_id in the `meta` table on first successful match; thereafter require BOTH login and pinned id. A renamed-away login stops matching (fail closed) rather than transferring admin to whoever claims the handle. Boot log records the pinned id so a change is visible. Test: same login + different github_id is denied once pinned."
- id: T-003
  title: "Move cache tier logic out of the server binary"
  priority: 2
  status: todo
  notes: "cache_valve.rs is pub(crate) in the bin, but server fns live in ironpad-app. Move tier names, fs_usage, and clear_cache_tier into a shared ssr-gated module so the valve and the panel cannot disagree about what a tier is. Keep the existing valve tests passing unchanged."
- id: T-004
  title: "/admin route + read-only overview"
  priority: 2
  status: todo
  notes: "SSR page, noindex, robots.txt disallow. admin_overview(): user/session/share counts, per-tier disk usage, DB + WAL bytes, build admission queue depth. Non-admin renders the standard not-found."
- id: T-005
  title: "User list + session revoke"
  priority: 2
  status: todo
  notes: "admin_list_users(): login, avatar, created_at, session count, grant count, owned share count. admin_revoke_user_sessions(github_id). Read-only otherwise: no role editing (rbac_grant only ever mints OWNER and READ from existing flows) and no user deletion (cascade to shares loses data irrecoverably). Both are explicit non-goals."
- id: T-006
  title: "Cache tier operations with cost-stating confirms"
  priority: 3
  status: todo
  notes: "admin_wipe_cache_tier(tier), admin_run_pressure_valve(). Confirm dialog names the tier, its measured size, and the user-visible consequence; returns bytes freed. Never wipes blobs without saying it means cold compiles for readers."
- id: T-007
  title: "Gate coverage test + e2e"
  priority: 1
  status: todo
  notes: "A test enumerating every admin server fn asserting anonymous and non-admin rejection, mirroring server_fns::tests::private_shares_gate_every_read_surface so a new fn without the gate fails the suite. Playwright: admin sees the panel, non-admin gets not-found, unset means not-found (IRONPAD_ADMIN_LOGIN cannot be set in the shared webServer env, so the e2e needs its own server or a route that reads config per-request)."
---

# Summary

A `/admin` page for the single operator of an ironpad instance, gated by
`IRONPAD_ADMIN_LOGIN` plus a real signed-in session. It shows instance state,
lists users with a session-revoke action, and operates the compile cache tiers.

# Problem

Operating the deployed instance currently means `fly ssh console` and reading
the filesystem by hand. Everything learned about this instance recently came
that way: the volume sitting at 4.0G of 9.9G, the per-tier split, the WAL at
64,227 batches. None of it is visible from the app, so it is only ever
discovered by going looking, which means it is discovered late.

The cache pressure valve wipes rebuildable tiers at 80% usage with no way to
see how close it is, to inspect what it would remove, or to run it deliberately
before a deploy rather than having it fire during traffic.

# Goals

1. Instance state is visible from the app: users, sessions, shares, per-tier
   disk usage, DB and WAL size, admission queue depth.
2. A user's sessions can be revoked without SSH.
3. Cache tiers can be inspected and cleared deliberately, with the cost stated
   before the action runs.
4. The admin surface does not exist on instances that did not opt in.

# Technical Approach

One predicate gates everything:

```
request -> session cookie -> current_user() -> Some(AuthUser)
                                                   |
                          IRONPAD_ADMIN_LOGIN ------+--> admin_user() -> Option<AuthUser>
                          pinned github_id     ----/
```

`admin_user()` returns `Some` only when all three agree: a valid session, a
case-insensitive login match against the configured value, and a match against
the `github_id` pinned on first use. Every admin server fn calls it first and
returns the not-found-shaped error otherwise.

Cache tier logic moves from `cache_valve.rs` (a `pub(crate)` module of the
server binary) into a shared ssr-gated module, so the automatic valve and the
manual panel read the same tier definitions.

# Assumptions

- The instance has GitHub OAuth configured. Without it there are no sessions,
  so there is no way to be an admin; the panel is absent by the same mechanism
  that hides sign-in.
- One admin. The value is a single login, not a list. A list is a later change
  and does not affect the gate's shape.

# Constraints

- `rbac_grant` only ever mints `OWNER` and `READ`, both from existing flows, so
  role management is out of scope rather than partially built.
- Playwright's `webServer` is shared across the suite and sets
  `IRONPAD_TEST_AUTH`, which T-001 makes mutually exclusive with an admin
  login. The e2e for the panel therefore needs its own server instance.
- The panel reads live filesystem state, so its numbers are only as fresh as
  the request; nothing is cached.

# References to Code

- `crates/ironpad-app/src/auth.rs` — `current_user`, `AuthEnabled`
- `crates/ironpad-app/src/db.rs` — `AuthUser`, `meta` table, session rows
- `crates/ironpad-app/src/server_fns.rs` — gate patterns, coverage test
- `crates/ironpad-server/src/cache_valve.rs` — tiers, `fs_usage`, valve
- `crates/ironpad-server/src/config.rs` — clap + env config

# Non-Goals (MVP)

- Content moderation: listing, viewing, or deleting other people's shares.
- RBAC role granting or revocation from the panel.
- User deletion.
- Multiple admins.
- Any write path to notebook content.
- Metrics history or charts; the panel reports current state only.

# History

- 2026-08-11: Drafted. Scope set to instance state, read-only users plus
  session revoke, and cache tier operations; content moderation, role editing,
  and user deletion excluded. Auth model chosen as a GitHub login allowlist
  over a bearer token. The login-rename escalation (a freed handle claimed by
  someone else) was raised during design and became T-002.
