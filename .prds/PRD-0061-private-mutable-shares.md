---
id: PRD-0061
title: "Private mutable shares: a private flag plus READ grants by GitHub handle"
status: done
owner: "Aaron Roney"
created: 2026-08-06
updated: 2026-08-06

depends_on:
- PRD-0053
- PRD-0054

principles:
- "The data model was pre-shaped for this (PRD-0053): mutable_share.private defaults false and rbac_grant carries (user, resource_kind, resource_id, role) with OWNER minted. This PRD mints READ and starts reading the flag."
- "Grant targets must already be ironpad users: rbac_grant.user is a typed record<user> link and a bare GitHub login cannot be resolved to a github_id without calling GitHub. The UI says so plainly; resolving unknown handles via the GitHub API is a later enhancement, not silently absent."
- "Denial is explicit, never a soft 404: the reader page tells an anonymous visitor to sign in and a signed-in non-grantee that they lack access — a private link handed to the right person must WORK after one sign-in, not dead-end."
- "Unfurl surfaces must not leak: the OG card handler and the oEmbed provider 404 for private shares, and SSR renders the denial (never draft or published content). Content-addressed share blobs stay ungated: a blob hash is only knowable by having seen the manifest, which IS gated."

references:
- name: "Grant storage + access checks"
  url: crates/ironpad-app/src/db.rs
- name: "Server fns (gating + grant management)"
  url: crates/ironpad-app/src/server_fns.rs
- name: "Reader page (denial UI)"
  url: crates/ironpad-app/src/pages/mutable_notebook.rs
- name: "Owner sharing UI"
  url: crates/ironpad-app/src/pages/notebook_editor/metadata_panel.rs

acceptance_tests:
- id: uat-001
  name: "A private share denies anonymous readers and non-grantees (reader page, manifest, OG card, oEmbed) while the owner sees content everywhere"
  command: cargo make uat
  uat_status: verified
- id: uat-002
  name: "Granting READ by GitHub handle admits that user; revoking denies again; granting an unknown handle errors with the sign-in-first explanation"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "db.rs: READ grants + private flag plumbing"
  priority: 1
  status: done
  notes: "ROLE_READ; set_share_private; private on MutableShareRow + ShareEditRow; find_user_by_login; grant_read/revoke_read/list_read_grants (idempotent upsert via the grant_unique index); user_can_read_share = OWNER or READ."
- id: T-002
  title: "Access gating on every read surface"
  priority: 1
  status: done
  notes: "MutableNotebookAccess { Found, Private, NotFound } in ironpad-common; get_mutable_notebook returns it; get_mutable_manifest denies; OG mutable handler and /oembed 404 on private; /embed/mutable renders the denial (cross-site iframes carry no SameSite=Lax cookie, so embeds of private shares deny by construction — documented)."
- id: T-003
  title: "Server fns + owner UI"
  priority: 1
  status: done
  notes: "set_mutable_private, grant_mutable_read(login), revoke_mutable_read, list_mutable_read_grants (owner-gated). MutableEditResponse gains private (serde default). Editor metadata panel grows an Access section in ServerDraft mode: private toggle, grant list with revoke, add-by-handle."
- id: T-004
  title: "Reader denial UI + tests"
  priority: 1
  status: done
  notes: "Reader/embed pages render the Private arm (sign-in link when anonymous). Unit: private lifecycle + grant/revoke/unknown-handle via the in-memory db harness. e2e: owner flips private, fresh anonymous context sees the denial, granted test user sees content (IRONPAD_TEST_AUTH)."
---

# Summary

A mutable share's owner can flip it private and grant read access to named GitHub users. Every read surface (reader page, embed, manifest, OG card, oEmbed) honors the flag; denial is an explicit sign-in prompt rather than a 404.

# Non-Goals (MVP)

- EDIT roles (co-editing forces the draft-concurrency question PRD-0054 deliberately documented).
- Granting handles that have never signed in (needs GitHub API resolution).
- Private immutable `/shared` links (content-addressed and frozen; a different design).

# History

- **2026-08-06** — Created; scope (toggle + grants by handle) confirmed by Aaron.
- **2026-08-06** — Implemented and closed. db: ROLE_READ, set_share_private, find_user_by_login (case-insensitive), grant/revoke/list (idempotent via the unique index), user_can_read_share, `private` read into both row shapes (absent = public for pre-0061 rows). One access core (`mutable_access_core`) feeds the reader server fn; `get_mutable_notebook_core` returns None for private unconditionally, which gates OG + oEmbed for free (both already 404 on None). `MutableNotebookAccess { Found, Private { signed_in }, NotFound }` on the wire; reader + embed render explicit denials; manifest withheld. Access UI in the metadata panel (ServerDraft only). Tests: `private_shares_gate_every_read_surface` unit lifecycle; `private-shares.spec.ts` e2e (owner flips private, anonymous denial + no title in raw SSR + OG/oEmbed 404, grant-by-handle admits, revoke denies, unknown handle errors). Gate: 822 unit, full Playwright green.
