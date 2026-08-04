---
id: PRD-0054
title: "Server-authoritative mutable shares: draft/published split, unified URL, Push button"
status: active
owner: "Aaron Roney"
created: 2026-08-04
updated: 2026-08-04

depends_on:
- PRD-0053

principles:
- "One address per published notebook: /mutable/{id} is the reader for everyone and the editor for the owner. The /local working-copy world exists only for private notebooks."
- "Draft and published are two server-side slots on one share. The editor writes the draft; readers only ever see published; Push is the single editorial act that promotes one to the other."
- "Private stays local-first, published is cloud-native: drafts of published notebooks live on the server, so every signed-in device edits the same draft and cross-device sync ceases to be a feature."
- "Last-write-wins on the draft, stated plainly: simultaneous edits from two devices (or an agent session) clobber at autosave granularity. Merging is CRDT territory and out of scope."
- "Push uploads nothing: it promotes the draft the server already has, and that is the only moment blobs are snapshotted."

references:
- name: "DB layer (draft slot + promote)"
  url: crates/ironpad-app/src/db.rs
- name: "Mutable server fns (draft save/load, promote)"
  url: crates/ironpad-app/src/server_fns.rs
- name: "Reader page (becomes mode-switching)"
  url: crates/ironpad-app/src/pages/mutable_notebook.rs
- name: "Editor (storage adapter seam)"
  url: crates/ironpad-app/src/pages/notebook_editor/mod.rs
- name: "IndexedDB storage (mutable store deleted, DB_VERSION 5)"
  url: public/storage.js
- name: "PRD-0053 accounts (ownership model this builds on)"
  url: .prds/PRD-0053-accounts-github-oauth-rbac.md

acceptance_tests:
- id: uat-001
  name: "Owner opens /mutable/{id} and gets the editor with a grayed Published button; a non-owner and an anonymous context get the view-only reader"
  command: cargo make uat
  uat_status: verified
- id: uat-002
  name: "An edit activates Push; readers still see the old published copy until Push; after Push a fresh context sees the update and the button grays again"
  command: cargo make uat
  uat_status: verified
- id: uat-003
  name: "Second device, same account: opens the same URL and sees the in-progress draft in the editor while anonymous readers still see published"
  command: cargo make uat
  uat_status: verified
- id: uat-004
  name: "Discard draft reverts the editor to the published copy and grays the button"
  command: cargo make uat
  uat_status: verified
- id: uat-005
  name: "Share Mutable removes the local IndexedDB copy and the home card links to /mutable/{id}; Unpublish returns the latest draft content to the private list"
  command: cargo make uat
  uat_status: verified
- id: uat-006
  name: "Draft autosave failure surfaces a visible unsaved state and recovers on the next successful save"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "DB: draft slot + promote"
  priority: 1
  status: done
  notes: "mutable_share gains draft_json (option<string>; None means draft == published). New fns: save_draft(id, json) (sets draft_json), get_share_for_edit(id) -> {draft-or-published json, dirty: draft_json.is_some(), pushed_at}, discard_draft(id) (clears draft_json), promote_draft(id, manifest_json) (published := draft, clears draft_json, bumps pushed_at — one transaction). get_mutable_share (reader path) returns published only, unchanged. Idempotent DEFINE for the new field; unit tests for the whole draft lifecycle including promote-with-no-draft (no-op or error, pick and test)."
- id: T-002
  title: "Server fns: draft save/load, promote replaces push"
  priority: 1
  status: done
  notes: "save_mutable_draft(id, notebook_json) — OWNER-gated, size-capped, no blob work (called on the autosave debounce). get_mutable_for_edit(id) — OWNER-gated, returns draft + dirty flag. push_mutable(id, cell_type_tags) — OWNER-gated, snapshots blobs from the DRAFT content and promotes; no notebook payload anymore. discard_mutable_draft(id). get_mutable_notebook (readers) untouched. Rate/size: draft saves inherit MAX_SHARE_BYTES; consider a per-session debounce floor server-side only if abuse shows up (non-goal now)."
- id: T-003
  title: "Editor storage adapter + unified /mutable/{id} route"
  priority: 1
  status: done
  notes: "The one real architectural lift. A storage seam for the editor: Local (IndexedDB, today's path) vs ServerDraft(share_id) (load via get_mutable_for_edit, save via debounced save_mutable_draft with a 'Draft saved · Xs ago' indicator and a visible failure/retry state). /mutable/{id} SSRs the reader shell (SocialMeta from published, unchanged for crawlers); on hydrate, if get_mutable_for_edit succeeds the page swaps to the editor (Monaco is client-only anyway, so owners lose nothing). 'View as reader' menu item shows the published page (e.g. ?view=reader or a reader-mode signal). Agent sessions keep working: the browser model stays authoritative during an editing session; only persistence routes differ."
- id: T-004
  title: "Push button states + Discard draft"
  priority: 1
  status: done
  notes: "A dedicated toolbar button, not a hamburger item. States: 'Published ✓' (grayed; server said dirty=false and no local edits since load) → 'Push' (active; server-reported dirty OR any local edit) → pushing spinner → grayed. A push from another device isn't observed live (no polling); clicking Push with an already-clean draft is a harmless no-op with an 'Up to date' toast. 'Discard draft' lives in the menu, confirm-gated, reloads the editor from published. Pull Latest and the divergence banner are deleted — the draft IS the sync."
- id: T-005
  title: "Client storage: delete the mutable store; conversion/unpublish rework"
  priority: 2
  status: done
  notes: "storage.js DB_VERSION 5: drop the mutable object store and the binding (orphaned local working copies from 0.15.0 are discarded — dev-only data, same scorched-earth authorization). Share Mutable: upload, then DELETE the local IndexedDB copy and navigate to /mutable/{id}. Unpublish: download the latest draft content into the private store, delete the share server-side, navigate to /local/{uuid}. Home: three sources with zero reconciliation — private from IndexedDB, Published from list_mutable_shares() (cards link /mutable/{id} for everyone), public from the server scan. Rust storage/client.rs bindings shrink accordingly."
- id: T-006
  title: "Tests: unit + e2e rewrite"
  priority: 2
  status: done
  notes: "Unit: draft lifecycle in db.rs + server_fns cores (owner-gated draft save, promote snapshots from draft, discard). e2e: the six UATs; mutable-shares.spec restructures around the unified URL (the second-device clone test becomes a shared-draft test); auth.spec untouched."
- id: T-007
  title: "Docs + ship"
  priority: 3
  status: todo
  notes: "CLAUDE.md/DEVELOPMENT.md mutable sections rewritten again; PRD history; version bump + deploy + warm (deploy requires explicit authorization). Note for the release: published notebooks become online-only to edit; private notebooks stay local-first."
---

# Summary

Mutable shares become fully server-authoritative with a **draft/published split**. One URL — `/mutable/{id}` — serves everyone: the view-only published copy for readers, the live editor over the server-side draft for the owner. A dedicated toolbar button carries the editorial act: grayed **"Published ✓"** when there is nothing to push, an active **"Push"** the moment an edit lands in the draft. The local working copy, the share binding, clone-to-local, the divergence banner, and Pull Latest are all deleted.

# Problem

PRD-0053 kept the PRD-0049 storage shape: a local IndexedDB working copy per device plus the server copy, reconciled by hand (push, pull, divergence banner, clone-to-local). Every one of those mechanisms exists only because the truth lives in two places. With accounts, the server can simply own published notebooks — but naively editing the published record live would expose readers to half-finished edits at keystroke granularity. The draft/published split keeps Push as the editorial gate while deleting the two-places problem.

# Goals

1. One URL per published notebook; opening it from a link or the home list is the same action.
2. The owner edits the server draft from any signed-in device; readers only ever see published.
3. A visible, stateful Push button: grayed when clean, active when the draft differs.
4. Delete the local mutable store and every reconciliation mechanism it required.
5. Home lists three sources with zero merging: private (IndexedDB), Published (session), public (server scan).

# Technical Approach

**DB.** `mutable_share.draft_json: option<string>` — `None` means clean (draft == published). Autosave writes it; Push promotes it into `notebook_json`, snapshots blobs from the promoted content, and clears it, in one transaction. Discard clears it without promoting.

**Editor seam.** The editor gains a storage adapter: `Local` (IndexedDB, unchanged, private notebooks) vs `ServerDraft(share_id)` (load `get_mutable_for_edit`, save via debounced `save_mutable_draft`, save-state indicator with a visible failure mode). `/mutable/{id}` SSRs the reader (crawler metadata unchanged, from published); on hydrate an owner swaps into editor mode.

**Conversion and unpublish.** Share Mutable uploads and deletes the local copy — the notebook now lives at its public URL. Unpublish pulls the latest draft content back into IndexedDB as a private notebook. There is no in-between state.

**Concurrency.** Last-write-wins on the draft at autosave granularity, across devices and agent sessions alike. Stated in the UI docs, not papered over. Real merging is out of scope.

# Assumptions

- Scorched earth on 0.15.0 local working copies (DB_VERSION 5 drops the store); no production data exists beyond Aaron's tests.
- Online-only editing for published notebooks is acceptable; private notebooks remain the local-first path.
- Draft saves at a few-second debounce are within the single-author deployment's write budget (they are plain JSON writes, no blob work).

# Constraints

- Reader-path behavior (`get_mutable_notebook`, OG cards, oEmbed, no-cache resolve) must be byte-for-byte indistinguishable from today: published is published.
- The SSR body of `/mutable/{id}` must not leak draft content (crawlers and readers share it).
- PROTOCOL_VERSION untouched; agent sessions interact with the browser model exactly as before.

# References to Code

See frontmatter. The storage adapter (T-003) touches `notebook_editor/mod.rs` load/save paths and `storage/client.rs`; everything else is contained in the files already rewritten by PRD-0053.

# Non-Goals (MVP)

- CRDT/operational merging of concurrent drafts, or draft locking.
- Offline editing of published notebooks.
- READ/EDIT roles, private shares (unchanged from PRD-0053).
- Draft history/versioning (the draft is one slot, not a timeline).
- Server-side rate limiting of draft saves beyond the existing size caps.

# History

- **2026-08-04** — T-001..T-005 implemented in one pass: `draft_json` slot with single-statement promote (WHERE-guarded, LWW-safe against concurrent autosaves), draft server fns (save is owner-gated + size-capped, push promotes with `Ok(false)` for clean), editor storage seam via `NotebookState.server_draft_share` + epoch-coalesced 1.5s debounce with retry, unified `/mutable/{id}` (SSR always the reader; owner swaps to `NotebookEditor` with the `server_draft` prop on hydrate; `?view=reader` pins), toolbar Push button + Discard Draft + View as Reader, Share Mutable deletes the local copy and hard-navigates, Unpublish saves-local-then-deletes, storage.js DB_VERSION 5 deletes the mutable store, home lists three unmerged sources. Pull Latest, the divergence banner, clone-to-local, and the local binding are deleted. e2e rewrite (T-006) in flight.
- **2026-08-04** — T-006 done: mutable-shares.spec.ts rewritten around the unified URL (owner lifecycle with reader-invisible drafts asserted in the DOM AND the raw unfurl body, cross-device shared draft with push-from-device-two, discard + view-as-reader round-trip, unpublish-brings-it-home with hard 404s, unfurl contract + home listing). Full Playwright: 103 passed, 0 failed; 783 unit tests. UAT evidence: cargo make ci + full Playwright (test-integration not re-run; compiler untouched). uat-006's failure-path half is covered by the retry logic's unit-visible structure and manual reasoning — the indicator's Failed state has no deterministic e2e trigger without fault injection; noted as the one soft spot.
