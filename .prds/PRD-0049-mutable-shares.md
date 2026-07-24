---
id: PRD-0049
title: "Mutable shares: author-updatable notebooks at /mutable/{id}, no accounts"
status: draft
owner: "Aaron Roney"
created: 2026-07-24
updated: 2026-07-24

principles:
- "No accounts: identity is a key. The server verifies and never custodies; it stores blake3 hashes, and plaintext keys exist only in the author's IndexedDB and on the TLS wire."
- "Mutability is a pointer property: every push re-snapshots content-addressed immutable blobs (PRD-0047 machinery verbatim); only the id-to-content resolve is mutable and served no-cache."
- "Conversion, not mapping: a mutable share IS the notebook's storage class. The notebook moves into a mutable IndexedDB store keyed by the server-minted id; there is no side table relating local and remote ids."
- "Degradation is freezing: lost keys leave a share readable forever and updatable never. Same durability contract as local-first private notebooks."
- "Two keys, either works: a per-profile user key (all your shares) and a per-share notebook key (delegation and fallback). The server does not care which one authorizes a push."

references:
- name: "Server functions (share machinery to extend)"
  url: crates/ironpad-app/src/server_fns.rs
- name: "IndexedDB storage (new mutable store, DB_VERSION bump)"
  url: public/storage.js
- name: "Blob snapshot recipe (reused per push)"
  url: crates/ironpad-common/src/cache_key.rs
- name: "View-only renderer (mutable reader page)"
  url: crates/ironpad-app/src/components/view_only_notebook.rs
- name: "PRD-0047 static blob delivery (snapshot layer this composes with)"
  url: .prds/PRD-0047-static-blob-delivery.md
- name: "PRD-0048 canonical routes (prefix names the storage class)"
  url: .prds/PRD-0048-canonical-routes.md

acceptance_tests:
- id: uat-001
  name: "Convert a notebook to a mutable share; /mutable/{id} renders it in a fresh browser context; push an edit; the reader context sees the new content after reload"
  command: cargo make uat
  uat_status: unverified
- id: uat-002
  name: "Push with an invalid key is rejected; push with the user key succeeds; push with the notebook key succeeds"
  command: cargo make uat
  uat_status: unverified
- id: uat-003
  name: "Rebind on a second profile: enter either key on /mutable/{id}, the notebook is pulled into the local mutable store, and push works from that profile"
  command: cargo make uat
  uat_status: unverified
- id: uat-004
  name: "Unpublish removes the server copy (subsequent GET is a miss) and returns the notebook to the private store"
  command: cargo make uat
  uat_status: unverified

tasks:
- id: T-001
  title: "Server: mutable share store + server fns"
  priority: 1
  status: todo
  notes: "create_mutable_share(notebook_json, user_key, notebook_key) mints a 16-hex random id, stores {data_dir}/mutable/{id}.json plus both key hashes; push_mutable(id, key, notebook_json) verifies the presented key against either hash and overwrites; get_mutable_notebook(id) -> Option; verify_mutable_key(id, key) -> bool for the rebind UX; delete_mutable(id, key) unpublishes. Keys travel plaintext over TLS. Hashing: blake3 derive_key with fixed context 'ironpad mutable-share auth v1' (domain separation instead of salts; keys are full-entropy so salting/argon2 buy nothing, and an unsalted user-key hash is what makes enumeration an index lookup); constant-time comparison via subtle. Reuse the share-notebook size caps and atomic_write_async; each successful create/push re-runs the PRD-0047 snapshot (cache-hit-only) into a {id}.manifest.json that is REPLACED, not merged."
- id: T-002
  title: "Reader route: /mutable/{id} page with blob delivery"
  priority: 1
  status: todo
  notes: "MutableNotebookPage renders the view-only notebook plus manifest, mirroring shared_notebook.rs; the notebook resolve and manifest are no-cache (the id is a mutable pointer) while /share-blobs stays immutable. Fork to private works unchanged. Include an 'I have a key' affordance for T-005."
- id: T-003
  title: "Client storage: mutable store + key management"
  priority: 1
  status: todo
  notes: "storage.js DB_VERSION 3: 'mutable' object store keyed by share id (notebook JSON + notebook_key + pushed_at); user key in a small meta store, auto-generated (32 random bytes hex) on first use. Replace-by-clobber: writing a new key value overwrites local storage only and never revokes anything server-side. Rust bindings in storage/client.rs."
- id: T-004
  title: "Editor: convert, push, staleness, unpublish"
  priority: 1
  status: todo
  notes: "Share button splits into Share Immutable (today's flow, unchanged) and Share Mutable (convert: create on server, move notebook from private store to mutable store under the minted id). Mutable-backed notebooks open in the editor from the mutable store and gain a Push button (overwrite server, re-snapshot); on open, compare content hash with the server copy and offer to pull when the server is newer (last-push-wins otherwise, but never silently from a stale base). Unpublish deletes server-side and moves the notebook back to the private store. Home page lists mutable-backed notebooks as their own group."
- id: T-005
  title: "Rebind flow on the reader page"
  priority: 2
  status: todo
  notes: "Enter either key on /mutable/{id}: verify_mutable_key gates it, then pull the notebook into this profile's mutable store and navigate to the editor. Dedupe: if the store already holds the id, navigate instead of re-importing. Visually distinct from Fork (fork = new independent notebook, no binding)."
- id: T-006
  title: "Key surfaces: footer + share panel"
  priority: 2
  status: todo
  notes: "Footer shows the user key masked with copy/reveal/replace (replace = local clobber, labeled as such). The notebook key for a mutable-backed notebook lives in its share panel next to the /mutable/{id} link, same masked treatment. Keys never appear in URLs. Replace validates format (64 hex chars) and rejects arbitrary strings: the no-salt hashing model depends on keys staying full-entropy, so no human-chosen passphrases may enter through this door."
- id: T-007
  title: "My published notebooks"
  priority: 3
  status: todo
  notes: "list_mutable_by_user(user_key) enumerates shares whose user_key_hash matches; home page section listing them with pushed_at, so a fresh machine with a pasted user key can find everything. Server-side this is a directory scan like list_public_notebooks."
- id: T-008
  title: "e2e coverage"
  priority: 2
  status: todo
  notes: "Playwright: the four UAT flows (convert/read/push/reader-sees-update across contexts; key rejection + both-keys-accepted; rebind on a second context; unpublish). Wrong-key paths assert the error toast, not silence."
- id: T-009
  title: "Docs"
  priority: 3
  status: todo
  notes: "CLAUDE.md routes list + storage sections + Last Updated; DEVELOPMENT.md route table."
---

# Summary

A third storage class: mutable shares. `Share Mutable` *converts* a private notebook into a server-backed one at `/mutable/{id}`: anyone with the link reads it, and the author overwrites it with an explicit **Push**. Authorization is two device-minted keys (a per-profile user key and a per-share notebook key; the server accepts either), hashed at rest, with no accounts anywhere. Immutable shares (`/shared/{hash}`) are unchanged.

# Problem

Shares are content-addressed and frozen: editing a shared notebook mints a new hash, so a posted link fossilizes. `/public/{name}` is a mutable alias, but only for notebooks bundled with the app. There is no way for a user to publish a notebook at a stable URL and keep it current.

# Goals

1. A stable reader URL per notebook that the author can update in place.
2. Zero accounts: possession of a key is the entire identity model.
3. Recovery paths that do not weaken the model: paste your user key on a new machine (everything), or a notebook key on the reader page (one share).
4. Reuse the immutable blob-delivery machinery so mutable readers never trigger compiles for snapshotted cells.

# Technical Approach

**Conversion.** Share Mutable calls `create_mutable_share`, which mints a random 16-hex id and stores the notebook JSON plus `blake3(user_key)` and `blake3(notebook_key)` under `{data_dir}/mutable/{id}.json`. The client moves the notebook from the private IndexedDB store into a new `mutable` store keyed by the share id; that store is now the working copy. The URL scheme follows PRD-0048: the prefix names the storage class.

**Keys.** The user key is auto-generated per browser profile (32 random bytes, hex) and shown masked in the footer; the notebook key is minted at conversion and shown in the share panel. Push presents one plaintext key over TLS; the server hashes and compares against either stored hash. A leaked volume leaks nothing writable. Replacing a key in the UI clobbers local storage only; server-side rekeying is explicitly out of scope for MVP.

**Push.** Explicit overwrite: replace the JSON, re-run the PRD-0047 share-time snapshot (cache-hit-only, misses degrade to the live pipeline), and replace the manifest sidecar. The underlying blobs stay content-addressed and immutable; only the `id → content` resolve is no-cache. On editor open, a content-hash comparison against the server offers a pull when someone (usually the same author on another machine) pushed since; conflict resolution is last-push-wins with that staleness nudge, nothing fancier.

**Lifecycle.** Fork from the reader page mints an independent private notebook (existing flow, no binding). Unpublish deletes the server copy and moves the notebook back to the private store. Losing both keys freezes the share: readable at the same URL forever, updatable never.

# Assumptions

- Plaintext-key-over-TLS with hash-at-rest is acceptable (password-model; hashes on the wire would make the hash itself the credential and buy nothing).
- Keys are always machine-generated full-entropy 256-bit values (UI-enforced on replace), which is why unsalted domain-separated blake3 suffices and slow password hashes are unnecessary. Known trade: identical user-key hashes across shares let a volume compromise group shares by anonymous author; that linkage is also exactly the enumeration feature, and author unlinkability is a non-goal.
- A 16-hex random id is unguessable enough to serve as the read capability, matching the existing share-hash posture.
- The mutable store holding the working copy (rather than the private store plus a mapping) is acceptable data migration for the convert/unpublish transitions.

# Constraints

- No accounts, no sessions, no server-side key recovery. Ever, within this PRD.
- Immutable shares and their guarantees are untouched.
- Mutable resolve must never be cached by browsers or intermediaries (no-cache headers on the notebook JSON and manifest).

# References to Code

- `crates/ironpad-app/src/server_fns.rs`: `share_notebook` / `snapshot_share_blobs` / `get_shared_manifest` are the templates for the mutable trio.
- `public/storage.js` + `crates/ironpad-app/src/storage/client.rs`: DB_VERSION bump, new store, key meta.
- `crates/ironpad-app/src/pages/shared_notebook.rs`: template for `MutableNotebookPage`.
- `crates/ironpad-server/src/main.rs`: `cache_control_value` gains the no-cache rule for mutable resolves.

# Non-Goals (MVP)

- Server-side key rotation/revocation (rekey endpoint is v2; local replace is clobber-only and does not revoke).
- Read keys / private mutable shares (the link is the read capability).
- Concurrent editing: no CAS, no merge, no WebSocket presence, no server-hosted live documents. The protocol groundwork (operation-based mutations, permissioned relay) keeps all of that reachable later.
- Renaming `/shared/` to `/immutable/` (separate conversation; would carry legacy redirects forever per PRD-0048).
- `/embed/mutable/{id}` (natural follow-on; embeds of mutable shares are the auto-updating-blog use case).
- Capability-based rendering of `/mutable/{id}` (editor-when-you-hold-the-id, viewer otherwise) — nicety, later.
- Accounts, OAuth, passkey wrapping. If ever wanted, they layer on as a recovery/sync wrapper around the key model, not a replacement.

# History

- 2026-07-24: Created from the design discussion (2026-07-23/24): capability-URL exploration collapsed to the conversion paradigm with two either-works keys after four rounds of descoping (live collaboration, read keys, CAS publishing, and per-share-only keys all considered and deliberately deferred).
