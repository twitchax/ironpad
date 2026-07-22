---
id: PRD-0047
title: "Static blob delivery: share-time blob snapshots + client-side blob cache"
status: active
owner: "Aaron Roney"
created: 2026-07-22
updated: 2026-07-22

principles:
- "The compile cache is invalidatable; a share must be durable. Snapshot artifacts at share time, never resolve shares through the live cache."
- "One cache-key recipe. The blake3 content-hash logic and feature detection move to ironpad-common and compile for both ssr and hydrate; the server delegates, the client reuses. No parallel reimplementation."
- "Viewers never trigger compiles. A shared/embed notebook with a snapshot replays from immutable static GETs; only editors (and the force-fresh toggle) can cost compile CPU."
- "The cache-vs-fresh selector keeps its meaning everywhere: fresh bypasses every cache layer (local blob store, share snapshot, server cache) and overwrites the local entry with the fresh result."
- "Graceful degradation over hard failure: missing tags, cap overflow, or a failed blob fetch fall back to today's compile_cell path."

references:
- name: "Compile cache module"
  url: crates/ironpad-app/src/compiler/cache.rs
- name: "Share server functions"
  url: crates/ironpad-app/src/server_fns.rs
- name: "View-only renderer (shared/public/embed)"
  url: crates/ironpad-app/src/components/view_only_notebook.rs

acceptance_tests:
- id: uat-001
  name: "Shared notebook with a snapshot replays via /share-blobs/ GETs with zero compile_cell calls (Playwright network assertion)"
  command: cargo make uat
  uat_status: unverified
- id: uat-002
  name: "Editor second run of an unchanged cell serves from the local IndexedDB blob store with no compile_cell request"
  command: cargo make uat
  uat_status: unverified
- id: uat-003
  name: "Fresh mode forces a server compile (force: true) and overwrites the local blob entry; flipping back to cache reuses the fresh blob"
  command: cargo make uat
  uat_status: unverified

tasks:
- id: T-001
  title: "Move cache-key machinery to ironpad-common"
  priority: 1
  status: done
  notes: "content_hash_inner (as content_hash_with_fingerprint), CACHE_EPOCH, merge_dependencies, crate_name_from_dep_line, merged_deps_contain_rayon, uses_std_autodiff, uses_wasm_simd. compiler/cache.rs and compiler/scaffold.rs delegate; public in-crate APIs unchanged. blake3 dep added to common (wasm-safe)."
- id: T-002
  title: "Share-time blob snapshot"
  priority: 1
  status: done
  notes: "share_notebook gains cell_type_tags (positional, from the editor's live outputs). Server walks runnable cells in order, builds each tag chain, computes content_hash, and snapshots CACHE HITS ONLY (no compiling at share time: the sharer just ran the cells, so they are warm; a cold cell means an unrun cell whose tag chain is unreliable anyway, and cache-only keeps share latency at file-copy speed). Blob+glue copied to {data_dir}/shares/blobs/{content_hash}.wasm/.js, sidecar {share_hash}.manifest.json. Misses are skipped (no manifest entry, live fallback). Blobs dir has its own byte cap; overflow degrades to a partial/absent manifest."
- id: T-003
  title: "Immutable /share-blobs/{file} GET route"
  priority: 1
  status: done
  notes: "Axum route serving {data_dir}/shares/blobs/ with strict filename validation (hex hash + .wasm/.js only, no traversal) and Cache-Control: public, max-age=31536000, immutable. Extend cache_control_value tests."
- id: T-004
  title: "Manifest fetch on shared/embed pages"
  priority: 1
  status: done
  notes: "get_shared_manifest(hash) server fn returning Option<ShareManifest> (cell_id -> {blob hash, has_glue}). SharedNotebookPage and EmbedSharedPage fetch it alongside the notebook and pass it to the view-only renderer."
- id: T-005
  title: "Blob-first run path in view-only renderer"
  priority: 1
  status: done
  notes: "Per cell: manifest entry + cache mode -> GET blob (+glue), executor::load_blob, execute. Fetch failure or missing entry -> compile_cell as today. Fresh mode ignores the manifest entirely and sends force: true."
- id: T-006
  title: "Toolchain fingerprint exposure to the client"
  priority: 2
  status: done
  notes: "Lightweight server fn returning toolchain_fingerprint(); client fetches once per session and memoizes. Client-side hashing folds it exactly as the server does, so a deploy/toolchain bump invalidates every local key for free."
- id: T-007
  title: "IndexedDB blob store in storage.js"
  priority: 2
  status: done
  notes: "New object store (IDB schema version bump): key = content hash, value = {wasm ArrayBuffer, glue, diagnostics, lastUsed}. getBlob/putBlob/prune with LRU eviction (count + byte caps). Record shape mirrors the server cache hit: diagnostics stored span-adjusted, preamble_lines 0 convention."
- id: T-008
  title: "Local-cache compile path in editor + viewer"
  priority: 2
  status: done
  notes: "Cache mode: compute content hash client-side (common recipe + fetched fingerprint), try local store, hit -> load_blob + execute, miss -> compile_cell then putBlob. Fresh mode: skip local read, compile_cell(force: true), then overwrite the local entry under the same key."
- id: T-009
  title: "e2e coverage"
  priority: 2
  status: in-progress
  notes: "Playwright: shared replay with zero compile_cell network calls (uat-001); editor local-cache second run (uat-002); fresh-mode force + local overwrite (uat-003). Route interception/request counting per project convention."
- id: T-010
  title: "Docs"
  priority: 3
  status: done
  notes: "CLAUDE.md storage/sharing + common-tasks sections; DEVELOPMENT.md architecture notes; Last Updated stamps."
---

# Summary

Two-layer static delivery for compiled cell WASM: (1) shares and embeds snapshot their compiled blobs at share time and replay them as immutable static assets, taking the compile server out of the viewer loop entirely; (2) the browser keeps a local content-addressed blob cache in IndexedDB so unchanged cells re-run instantly and offline in the editor and viewers.

# Problem

Every cell run pays a `compile_cell` round trip carrying megabytes of WASM, even on a warm server cache. Worse, viewers of shared/embedded notebooks depend on the live compile pipeline: any anonymous viewer can trigger real cargo builds, a viral share costs compile CPU per cold cell, and a toolchain bump plus cache wipe can silently break or slow every previously shared notebook (old source may not even compile on the new nightly).

# Goals

1. A shared/embedded notebook created after this feature replays all its cells with zero `compile_cell` invocations, served from immutable content-addressed GETs that CDNs and browser caches absorb.
2. Shares are durable across toolchain bumps and cache wipes: artifacts are frozen at share time.
3. Re-running an unchanged cell in the editor or a viewer is instant and offline (local blob store), with correct invalidation on any input change, toolchain bump, or CACHE_EPOCH bump.
4. The cache-vs-fresh selector still forces a real server recompile and refreshes every cache layer.

# Technical Approach

**Key recipe extraction (T-001).** `content_hash_inner`, `CACHE_EPOCH`, and the three feature-detection functions (plus the dep-merge helpers they need) move to `ironpad-common`, which already compiles for both targets. `compiler/cache.rs` keeps its `content_hash` entry point delegating with the process-cached `toolchain_fingerprint()`. The client calls the same common function with a fingerprint fetched once per session (T-006).

**Layer 2 (T-002 to T-005).** A cell's cache key depends on the upstream type-tag chain, and tags only exist after execution — so the sharer's browser supplies them (`cell_type_tags`, positional, defaulted so stale clients still share manifest-less). Share time: walk runnable cells in order, compute each key, resolve the blob from the compile cache (compile on miss — the sharer just ran these cells, so misses are rare), copy blob + glue into `{data_dir}/shares/blobs/` (content-addressed, deduped across shares), and write a `{share_hash}.manifest.json` sidecar. The stored `{share_hash}.json` notebook format is untouched — old shares keep working, new fields ride the sidecar. Viewers fetch the manifest with the notebook and, in cache mode, GET blobs from `/share-blobs/` (immutable) instead of invoking `compile_cell`; any failure falls back to the live pipeline.

**Layer 1 (T-006 to T-008).** `storage.js` gains a content-addressed blob store (LRU-capped). The run path in both the editor and the view-only renderer becomes: fresh mode → `compile_cell(force: true)` then overwrite the local entry; cache mode → local hit → execute, miss → (viewer: share blob →) `compile_cell` → store. The local record mirrors the server cache hit shape (blob, glue, span-adjusted diagnostics, `preamble_lines: 0`).

# Assumptions

- Type tags are deterministic given cell source (they name the output type, not the data), so share-time chains match viewer-run chains.
- blake3 compiles and runs correctly on wasm32-unknown-unknown (portable implementation).
- Compiled blobs are toolchain-agnostic at runtime: a blob built on an old nightly executes fine forever.

# Constraints

- The share JSON on-disk format must not change (backward compatibility with existing shares and `get_shared_notebook` parsing).
- Share disk caps: blob snapshots participate in aggregate cap accounting; overflow degrades gracefully rather than failing the share.
- Server fn signature changes must tolerate stale clients within a deploy window (`#[serde(default)]` on new fields).
- `check_cell` (live diagnostics) intentionally stays on the wire — no local caching of checks.

# References to Code

- `crates/ironpad-app/src/compiler/cache.rs` — `content_hash`, `try_cache_hit`, `store_blob`, `CACHE_EPOCH`
- `crates/ironpad-app/src/compiler/scaffold.rs` — detection fns, dep merge helpers
- `crates/ironpad-app/src/server_fns.rs` — `compile_cell_core`, `share_notebook_core_capped`, caps
- `crates/ironpad-app/src/components/view_only_notebook.rs` — viewer run path, `force_recompile` toggle
- `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs` — editor run path
- `crates/ironpad-server/src/main.rs` — routes, cache-control middleware
- `public/storage.js` — IndexedDB layer

# Non-Goals (MVP)

- Public notebooks (`public/notebooks/*.ironpad`) stay on the live compile path; they are deploy-warmed already. Snapshotting them at build time is a natural follow-up.
- Fully serverless export (single-file bundle with embedded blobs) — enabled by this architecture, not built here.
- Local caching of `check_cell` results.
- Share deletion/GC of orphaned blobs.

# History

- 2026-07-22: Created; design agreed in session (layer split, sharer-supplied type tags, sidecar manifest, common-crate key recipe).
- 2026-07-22: Implemented T-001 through T-008 and T-010. Cache-key recipe + CACHE_EPOCH moved to ironpad-common/src/cache_key.rs (blake3 in common); share snapshots are cache-hit-only with a merged manifest sidecar and a 2 GiB blob-dir cap; /share-blobs/{hash}.{wasm,js} served immutable with strict name validation; viewer takes snapshot -> local IndexedDB -> compile_cell in that order; editor takes local -> compile_cell; Force Recompile bypasses all layers and overwrites the local entry. 8 new unit tests (cargo make ci green, 667 passing). T-009 spec written (tests/e2e/blob-cache.spec.ts), runs with the Playwright gate next.
