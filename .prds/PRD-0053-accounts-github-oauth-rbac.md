---
id: PRD-0053
title: "Accounts: GitHub OAuth, embedded SurrealDB, and RBAC-backed mutable shares"
status: active
owner: "Aaron Roney"
created: 2026-08-04
updated: 2026-08-04

depends_on:
- PRD-0049

principles:
- "Auth gates ownership, not usage: local notebooks, immutable shares, public notebooks, agent sessions, and compilation stay fully anonymous. Login exists only so mutable shares can have an owner."
- "Identity is delegated: GitHub owns credentials, 2FA, and recovery. ironpad stores only the GitHub user id, login, and avatar URL, plus its own opaque session."
- "RBAC from day one, one role minted: a generic grant table (user, resource kind, resource id, role) with only OWNER issued, and a private flag on shares defaulting false. Private mutable shares and EDIT/READ later are data changes, not schema changes."
- "One store: embedded SurrealDB (SurrealKV file backend) on the data mount. Share content, manifest, and ownership update transactionally; there is no file/DB consistency dance."
- "Scorched earth: no migration from the key mechanism. The data dir is wiped at deploy (no real shares exist), and the user-key/notebook-key machinery is deleted, not deprecated."

references:
- name: "Mutable share server functions (rewritten onto sessions + grants)"
  url: crates/ironpad-app/src/server_fns.rs
- name: "Server config (DB path, OAuth env, test-auth gate)"
  url: crates/ironpad-server/src/config.rs
- name: "Mutable reader page (binding by ownership instead of keys)"
  url: crates/ironpad-app/src/pages/mutable_notebook.rs
- name: "IndexedDB storage (key fields dropped, binding store stays)"
  url: public/storage.js
- name: "OG extraction (mutable card data moves to a DB query)"
  url: crates/ironpad-server/src/og/mod.rs
- name: "PRD-0049 mutable shares (the key mechanism this replaces)"
  url: .prds/PRD-0049-mutable-shares.md

acceptance_tests:
- id: uat-001
  name: "Sign in (test-auth in e2e): footer shows avatar + handle; sign out clears the session and the footer reverts"
  command: cargo make uat
  uat_status: verified
- id: uat-002
  name: "Owner lifecycle: logged-in user converts a notebook to a mutable share, pushes an edit, a logged-out context reads the update at /mutable/{id}; a different logged-in user's push is rejected"
  command: cargo make uat
  uat_status: verified
- id: uat-003
  name: "Second device: the owner in a fresh browser context opens /mutable/{id}, sees Edit, clone-to-local creates a working copy, and push from it succeeds"
  command: cargo make uat
  uat_status: verified
- id: uat-004
  name: "Attribution: the /mutable reader page shows the owner's GitHub handle and avatar"
  command: cargo make uat
  uat_status: verified
- id: uat-005
  name: "Unpublish deletes the share (subsequent GET is a miss) and the home Published group lists shares by session, not key"
  command: cargo make uat
  uat_status: verified
- id: uat-006
  name: "Security: /auth/test-login returns 404 when IRONPAD_TEST_AUTH is unset; session cookie is httponly/secure/SameSite=Lax"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "DB layer: embedded SurrealDB + schema"
  priority: 1
  status: done
  notes: "New db module in ironpad-server: surrealdb crate with the SurrealKV backend, file at {data_dir}/ironpad.db, connection in AppState. Tables: user (id = github user id; login, avatar_url, created_at), session (opaque 32-byte-hex id, user record link, created_at, expires_at with sliding ~30-day TTL), mutable_share (16-hex id kept for URL compatibility; notebook JSON, manifest, private: bool default false, pushed_at, created_at), grant (user, resource_kind, resource_id, role; only OWNER minted, uniqueness enforced at create). DEFINE TABLE/INDEX on boot, idempotent. Unit tests against a tempdir DB. Blobs stay on disk in shares/blobs (they are large binary wasm; only pointers live in the DB)."
- id: T-002
  title: "GitHub OAuth flow + sessions"
  priority: 1
  status: done
  notes: "Axum routes /auth/github (redirect with random state in a short-lived cookie, plus a redirect_to for returning to the origin page), /auth/callback (state check, code exchange via reqwest, fetch user via the GitHub API, upsert user, mint session, set ironpad_session cookie httponly/secure/SameSite=Lax, redirect back), /auth/logout (delete session, clear cookie). No scopes beyond default read:user. GITHUB_CLIENT_ID/GITHUB_CLIENT_SECRET via env (Fly secrets in prod, a second localhost OAuth app for dev); when unset the sign-in button is hidden and the server runs fine anonymous-only. Server fn get_current_user() -> Option<UserInfo{login, avatar_url}> reads the cookie via leptos_axum extract. Helpers require_user and require_role(user, kind, id, role) for the write paths."
- id: T-003
  title: "Env-gated test login"
  priority: 1
  status: done
  notes: "IRONPAD_TEST_AUTH=1 registers /auth/test-login?login={name}: upserts a synthetic user and mints a real session, exercising the same session/RBAC code as OAuth. The route is not registered at all when the env is unset (config test asserts absence; uat-006). Playwright webServer env sets it; prod never does. Same stale-server trap as the rate-limit overrides: a manually started server on :3111 lacks the env and silently breaks auth specs, so kill it before e2e reruns."
- id: T-004
  title: "Mutable shares on sessions + grants; delete the key mechanism"
  priority: 1
  status: done
  notes: "Rewrite server fns: create_mutable_share(notebook_json) requires a session, mints the share and an OWNER grant transactionally; push_mutable(id, notebook_json), delete_mutable_share(id) require the OWNER grant; list_mutable_shares() enumerates by session; get_mutable_notebook(id) gains owner attribution (login, avatar_url) and an is_owner bool for the caller. verify_mutable_key is deleted along with derive_key, the subtle dependency, and both key hashes. Notebook JSON + manifest move into the mutable_share record; the PRD-0047 snapshot recipe is unchanged (blobs on disk, manifest REPLACED per push). OG extraction for /mutable switches from reading {data_dir}/mutable/{id}.json to a DB query. The {data_dir}/mutable directory and its atomic-write path are removed."
- id: T-005
  title: "Client: sign-in UI, ownership binding, clone-to-local"
  priority: 1
  status: done
  notes: "Footer: sign-in-with-GitHub button when logged out; avatar + handle + sign-out when logged in (replaces the user-key widget). Reader page binding is now is_owner from get_mutable_notebook: owner with a local working copy gets the existing Edit shortcut; owner WITHOUT one gets Edit = clone the published copy into the mutable IndexedDB store and navigate (this replaces rebind on every new device). The rebind form, footer key UI, and notebook-key share-panel row are deleted. storage.js DB_VERSION 4: drop the user key from meta and notebook_key from mutable records; the share-id-keyed working-copy binding store stays as-is. Editor push/pull/unpublish flows keep their UX; they just stop sending keys."
- id: T-006
  title: "Attribution on the reader page"
  priority: 2
  status: done
  notes: "Published by @{login} with avatar on /mutable/{id}, near the title, linking to github.com/{login}. Always on (owner toggle is a later feature with the private flag). OG card attribution is a non-goal for now."
- id: T-007
  title: "e2e rewrite + auth spec"
  priority: 2
  status: done
  notes: "helpers/auth.ts with loginTestUser(page, login) hitting /auth/test-login. mutable-shares.spec.ts rewritten around login: owner lifecycle, cross-context read, non-owner push rejected (assert the error toast), second-device clone-to-local, unpublish, Published-group listing. New auth.spec.ts: login/logout footer states, test-login 404 when env unset (spawn a server without the env or assert against a config unit test), cookie flags."
- id: T-008
  title: "Deploy: secrets, scorched earth, docs"
  priority: 3
  status: todo
  notes: "Create the two GitHub OAuth apps (prod callback https://ironpad.twitchax.com/auth/callback, dev localhost); fly secrets set GITHUB_CLIENT_ID/GITHUB_CLIENT_SECRET; wipe the data dir contents on the Fly volume before first boot (mutable/, shares/, og/ all regenerate or start empty). CLAUDE.md mutable-shares paragraph + server-fn list + routes; DEVELOPMENT.md auth section."
---

# Summary

Accounts arrive, scoped tightly: **GitHub OAuth is the only login**, an **embedded SurrealDB** file on the Fly data mount is the only database, and **RBAC replaces the key mechanism** for mutable shares. A mutable share ties to the GitHub identity of its creator, who holds an OWNER grant; the share link stays public and anyone can read it. Immutable shares, local notebooks, public notebooks, and agent sessions are untouched and remain anonymous.

# Problem

The PRD-0049 key model works but scales badly to humans: two keys per user to custody, a rebind form on every new device, and no recovery when both keys are lost. It also blocks every future social feature (attribution, private shares, collaboration roles) because the server has no notion of *who* anyone is. GitHub OAuth delegates credentials, 2FA, and recovery to an account every ironpad user already has, and a real database gives ownership a place to live.

# Goals

1. Sign in with GitHub; the server stores only id, login, and avatar.
2. Mutable shares owned by an account: create/push/delete/list gated by an OWNER grant, readable by anyone with the link.
3. The second-device story collapses to "log in": Edit on a fresh device clones the published copy into a local working copy.
4. Owner attribution (handle + avatar) on the public reader page.
5. A schema that makes private shares and EDIT/READ roles later a data change, not a redesign.
6. Full e2e coverage of the RBAC surface via an env-gated test login that prod can never expose.

# Technical Approach

**Database.** The `surrealdb` crate embedded in `ironpad-server` with the SurrealKV file backend at `{data_dir}/ironpad.db`; no separate process. Four tables: `user`, `session`, `mutable_share`, `grant`. Notebook JSON and the blob manifest move into the share record so content and ownership commit together; the content-addressed wasm/js blobs stay as files behind the immutable `/share-blobs/` route exactly as in PRD-0047.

**Auth.** Standard OAuth authorization-code flow with a state cookie, then an opaque session id in an `ironpad_session` httponly/secure/SameSite=Lax cookie backed by a sessions table with a sliding ~30-day expiry. Leptos server fns read the cookie to resolve the user; write paths check the grant table. When the OAuth env vars are unset the whole surface disappears and the server runs anonymous-only, which keeps contributor setups and CI trivial.

**RBAC.** `grant(user, resource_kind, resource_id, role)` with only `OWNER` minted and one OWNER per share. `mutable_share.private` defaults false; the reader route today checks nothing, and the private feature later is: flip the flag, have the reader require a READ grant. EDIT is the same shape.

**Key mechanism removal.** `verify_mutable_key`, `derive_key`, the `subtle` dependency, both key hashes, the footer user-key widget, the notebook-key panel row, and the rebind form are all deleted. The IndexedDB working-copy binding (share id → local notebook) stays, because a device still needs to know which local copy backs which share; it just no longer carries keys.

**Test auth.** `IRONPAD_TEST_AUTH=1` registers `/auth/test-login?login={name}`, which mints a real user and session through the same code paths as OAuth. The route does not exist otherwise, asserted by test. Playwright sets the env in its webServer config.

# Assumptions

- No production shares exist worth migrating; the data dir is wiped at deploy.
- The `surrealdb` embedded dependency is acceptable build-time weight (cargo-chef caches it after the first Docker build).
- GitHub is an acceptable sole identity provider for the audience; passkeys/TOTP/email are explicitly future work.

# Constraints

- Session cookies mean the reader page's `is_owner` check is a server round trip on hydrate, replacing the current IndexedDB binding check; it must degrade gracefully when logged out (plain reader, no flash of owner UI).
- The OG card handler and `get_mutable_notebook` both read share content; both must go through the DB module (no lingering file reads).
- `/mutable` responses stay no-cache (the id is a mutable pointer); nothing about PRD-0047/0048 caching semantics changes.

# References to Code

See frontmatter references. The touchpoints outside the obvious auth/server-fn files: `og/mod.rs` (mutable card extraction), `public/storage.js` (DB_VERSION 4 key-field drop), `pages/notebook_editor/mod.rs` (push/pull/unpublish stop sending keys), `components/layout.rs` footer (sign-in widget replaces the key widget).

# Non-Goals (MVP)

- EDIT or READ roles, and multi-owner shares (schema supports them; nothing mints them).
- Private mutable shares (the flag exists, defaulting false; the reader ignores it).
- Login for immutable shares, or any ownership of them.
- Passkeys, TOTP, email sending, or any second identity provider.
- Account deletion UI and server-side data export.
- Attribution on OG cards.

# History

- **2026-08-04** — T-001..T-006 implemented: embedded SurrealDB (surrealdb 3.x, SurrealKV) with idempotent boot-time schema; GitHub OAuth + hashed-at-rest sliding sessions; env-gated /auth/test-login (absence asserted by unit test); mutable shares rewritten onto session + OWNER grant with content/manifest in the DB and the two-key mechanism deleted (derive_key, subtle, rebind form, footer key widget); reader-page attribution + clone-to-local Edit; footer sign-in/avatar; storage.js DB_VERSION 4 in-place migration. Notable deviations from the plan: the DB module lives in ironpad-app (ssr-gated) rather than ironpad-server because server fns need it and the dependency points that way; the DB is NOT in AppState (SurrealKV open costs ~1.5s, which would tax every WS handler test) — it travels as leptos context, the auth router's own state, and an axum Extension for the OG handler; the grant table is named rbac_grant to dodge SurrealQL keyword ambiguity. e2e rewrite (T-007) in flight.
- **2026-08-04** — T-007 done: auth.spec.ts (footer identity round-trip, cookie flag set on the wire, CSRF nonce cookie, session-required publish, anonymous functionality untouched) + mutable-shares.spec.ts rewritten around test-login sessions (owner lifecycle, unfurl, unpublish, author round-trip, second-device clone-to-local, non-owner and anonymous readers get no edit surface). Full Playwright run: 103 passed, 0 failed. UAT evidence: `cargo make ci` (782 unit tests) + the full Playwright suite; `test-integration` not re-run this cycle (compiler untouched). Marked verified on that basis.
