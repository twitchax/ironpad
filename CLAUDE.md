# ironpad Agents Guide

This document provides guidance for AI coding agents working in the ironpad repository.

## Quick Overview

**ironpad** is an interactive Rust notebook environment that compiles cells to WebAssembly and executes them in the browser. It supports real-time collaboration between humans and AI agents via WebSocket. The codebase spans 7 Rust crates, with a full-stack Leptos (SSR/WASM) frontend, Axum server with WebSocket relay, and a CLI daemon for agent interaction.

### Key Statistics
- **Crates**: 7 (app, cli, server, frontend, common, cell, proxy)
- **Compiler modules**: 7 (scaffold, cache, build, diagnostics, optimize, toolchain, mod)
- **Framework**: Leptos 0.8 + Axum + Monaco editor
- **Build tool**: cargo-make (all dev commands)
- **Collaboration**: WebSocket relay + CLI daemon + session/token management

---

## Workspace Overview

```
crates/
  ironpad-app/          # Core: compiler, UI, storage, pages, model, session
  ironpad-cli/          # CLI daemon + agent commands
  ironpad-server/       # HTTP server + WebSocket relay + session management
  ironpad-proxy/        # Domain-filtering forward proxy (cell-compile network sandbox)
  ironpad-frontend/     # WASM hydration (minimal)
  ironpad-common/       # Shared types + collaboration protocol
  ironpad-cell/         # Cell runtime (injected into every cell)

docker/                 # Multi-stage build + docker-compose
tests/e2e/              # Playwright e2e tests (including agent session tests)
public/                 # executor-bridge.js (+ worker/core), storage.js, Monaco editor, public notebooks
style/                  # SCSS styles
data/                   # Server-side shares + private notebook storage
```

---

## Quick Start

### Prerequisites
- **Rust**: 1.93+
- **Node.js**: 18+ (for npm/Playwright)
- **wasm32-unknown-unknown**: `rustup target add wasm32-unknown-unknown`

### Build & Run

```bash
# Install development tools
cargo make install-tools

# Development server (with hot reload)
cargo make dev
# Opens http://localhost:3111

# Run all tests
cargo make test

# Full CI (fmt-check + gen-completions-check + clippy + test)
cargo make ci

# UAT (the one true gate: CI + integration tests + Playwright)
cargo make uat
```

### All cargo-make Tasks

| Task                 | Purpose                                            |
| -------------------- | -------------------------------------------------- |
| `install-tools`      | Install all dev tools + wasm target                |
| `setup-monaco`       | Install monaco-editor from npm, copy dist to public/monaco/ |
| `dev`                | Start cargo-leptos watch (dev server, live reload) |
| `build`              | Release build via cargo-leptos                     |
| `build-cli`          | Build ironpad-cli binary (release)                 |
| `fmt`                | Auto-format all Rust code                          |
| `fmt-check`          | Check formatting (no changes)                      |
| `clippy`             | Run clippy lints                                   |
| `test`               | Unit/integration tests via cargo-nextest           |
| `test-integration`   | Slow tests (requires wasm32 target)                |
| `gen-completions`    | Regenerate Monaco completions index from ironpad-cell source |
| `gen-completions-check` | Fail if the committed completions index is stale |
| `capture-outputs`    | Capture saved outputs into public notebooks (server on :3111; names after `--` for a subset) |
| `capture-outputs-check` | Fail if a public notebook's code changed without an output recapture |
| `glyph-check`        | Fail if a bare Unicode symbol glyph is rendered instead of an `icons::` role |
| `css-vars-check`     | Fail if `style/main.scss` reads a CSS custom property nobody defines |
| `warm-prod`          | Converge a deployed instance's compile/check caches (post-deploy) |
| `ci`                 | fmt-check + gen-completions-check + capture-outputs-check + glyph-check + css-vars-check + clippy + test |
| `warmup-atomics`     | Pre-build std with atomics for rayon cells (one-time) |
| `playwright-install` | Install Playwright browsers                        |
| `playwright`         | Build CLI + run Playwright e2e tests               |
| `uat`                | ci + test-integration + playwright                 |
| `coverage`           | Coverage report via cargo-llvm-cov                 |
| `docker-build`       | Build Docker image                                 |
| `docker-up`          | Start container via docker-compose                 |
| `docker-down`        | Stop container                                     |
| `docker-uat`         | Build image, start container, run playwright, tear down |

---

## Constitutional Rules

These are inviolable project principles. Agents **must** follow these at all times.

1. **Clippy cleanliness**: All code must pass `cargo make clippy` (which runs with `-D warnings`). Clippy is enforced in both `ci` and `uat`. When making changes, fix any clippy warnings you introduce — and fix pre-existing warnings in files you touch when reasonable.

---

## Code Conventions

### Style & Patterns

- **Formatting**: Run `cargo make fmt` before committing
- **Imports**: Group by workspace, std, then external (alphanumeric)
- **Error handling**: `anyhow::Result<T>` for fallible functions; use `.context()` for error messages
- **Logging**: `tracing` (info, warn, debug) — no `println!`
- **Comments**: Use `//` and separate sections with `// ── Section ──────`
- **Modules**: Keep modules focused; use descriptive names

### Rust Idioms

- Prefer guard clauses over nested conditionals
- Use `?` operator for error propagation
- Leverage functional programming (map, filter, fold)
- Avoid `unwrap()` in library code; use `?` or `.context()`

### Testing

All compiler logic should be well-tested:
- **Unit tests**: In-crate `#[test]` functions
- **Integration tests**: Marked `#[ignore]` if slow, for `cargo make test-integration`
- **E2E tests**: Playwright in `tests/e2e/` (browser automation)

---

## Key Architecture

### Compilation Pipeline

The core of ironpad is a 5-stage WASM compiler:

1. **Scaffold** (`compiler/scaffold.rs`):
   - Generates a micro-crate from user source + Cargo.toml
   - Wraps user code in `cell_main` FFI function
   - Injects ironpad-cell dependency

2. **Cache Check** (`compiler/cache.rs`):
   - blake3 hash of (source, cargo_toml, previous cell types, shared cargo_toml/source, atomics flag) plus a toolchain fingerprint, so toolchain upgrades invalidate stale blobs
   - Lookup at `{cache_dir}/blobs/{hash}.wasm`

3. **Build** (`compiler/build.rs`):
   - `cargo build --target wasm32-unknown-unknown --release --message-format=json`
   - 300-second timeout (override with `IRONPAD_BUILD_TIMEOUT_SECS`)
   - Returns WASM blob path or stdout with diagnostics

4. **Diagnostics** (`compiler/diagnostics.rs`):
   - Parses rustc JSON output
   - **Critical**: Adjusts line numbers by subtracting the scaffold's computed `preamble_lines` (base 7 since the PRD-0060 trampoline; grows with shared source and referenced-slot bindings)
   - Maps back to user source for inline error display

5. **Optimize** (`compiler/optimize.rs`):
   - Best-effort `wasm-opt -O3` (binaryen; runtime speed over size)
   - Non-fatal failures

**Important**: All stages are tested with unit tests; full pipeline tested with integration tests in `compiler/mod.rs`.

### Notebook Storage & Sharing

**Private notebooks** are stored client-side in **IndexedDB** (browser-local), with a **version-history ring** (PRD-0058, DB_VERSION 6): every save mints at most one snapshot per 5-minute bucket per notebook (`history` store, capped at 30, deleted with the notebook, captured at the `storage.js` saveNotebook choke point). The editor's hamburger "🕘 History" (Local mode only) lists them; Restore force-snapshots the current version first, writes the chosen snapshot, and hard-reloads.

**Private notebooks** are stored client-side in **IndexedDB** (browser-local):
- `storage/client.rs` — wasm-bindgen bindings to `window.IronpadStorage` (from `public/storage.js`)
- No server-side notebook CRUD — the server is stateless for private notebooks
- Canonical format: `IronpadNotebook` (defined in `ironpad-common/src/types.rs`)

**Public notebooks** are static `.ironpad` JSON files served from `public/notebooks/` (bundled into `{site_root}/notebooks/` at build time):
- **Saved outputs (PRD-0056)**: every public notebook carries `saved_output` for the runnable cells that produce one, captured by `tools/capture-outputs.mjs` (`cargo make capture-outputs`; seeds each notebook as a scratch `/local` copy, runs cells one at a time, and drives the app's own Download flow — the production capture path). The tool records a per-cell source hash into `public/notebooks/.capture-manifest.json`, and `cargo make capture-outputs-check` (in `ci`) fails when a notebook's code changed without a recapture — snapshots of code that no longer exists cannot ship silently. Deliberate compile-fail teaching cells (`borrows`, `dynosaur`, `gen-blocks`) capture nothing themselves, and since PRD-0060's dependency-aware cascade their independent siblings capture fine.
- **Authorship disclosure (convention, not enforced)**: every notebook tagged `blog` opens its first markdown cell with `> *AI authored to showcase ironpad capabilities.*`, or `> *AI authored to showcase ironpad capabilities; human edited.*` where a human rewrote it. Prepend to the existing first cell rather than adding a new one, so cell indices (and therefore piping slots) do not shift. New notebooks in that series should carry it.
- **Prose for `blog` notebooks goes through `voice-lint`** (skill in `~/projects/dotagent`) before shipping. A Rust maintainer publicly quoted eleven lines of the cannon notebook as proof of AI authorship in July 2026; the skill exists so that check happens here first.
- **No index file**: `list_public_notebooks()` enumerates `{site_root}/notebooks/*.ironpad` at runtime and reads each notebook's own `title`/`description`
- Server functions: `list_public_notebooks()`, `get_public_notebook(filename)`

**Shared notebooks** use content-addressed storage:
- Upload notebook JSON → blake3 hash (16 hex chars) → stored at `{data_dir}/shares/{hash}.json`
- Server functions: `share_notebook(notebook_json, cell_type_tags)`, `get_shared_notebook(hash)`, `get_shared_manifest(hash)`
- Share URL: `/shared/{hash}`. Immutable: editing then re-sharing mints a NEW hash; old links are frozen forever.

**Accounts (PRD-0053)**: GitHub OAuth is the only login. An embedded SurrealDB (SurrealKV file at `{data_dir}/ironpad.db`, opened once at boot, ~1.5s) holds `user`, `session`, `mutable_share`, and `rbac_grant` tables — schema DEFINEs are idempotent on boot (`ironpad-app/src/db.rs`; ssr-gated so both the server fns and axum handlers reach it; deliberately NOT in `AppState` so WS tests don't pay the open, so it travels as leptos context + the auth router's own state + an axum `Extension` for the OG handler). Auth routes live in `ironpad-server/src/auth.rs`, mounted via `nest_service("/auth", …)`: `/auth/github` (redirect; CSRF nonce in state param + `/auth`-scoped cookie), `/auth/callback` (code exchange via reqwest, user upsert, session mint), `/auth/logout`, and `/auth/test-login` which exists ONLY when `IRONPAD_TEST_AUTH` is set (Playwright sets it; prod never does; unit tests assert absence). Sessions: 32-byte token in an `ironpad_session` HttpOnly/Secure/SameSite=Lax cookie, stored blake3-hashed as the record key, sliding 30-day expiry renewed at most every 12h. When `GITHUB_CLIENT_ID`/`GITHUB_CLIENT_SECRET` are absent the whole sign-in surface disappears and the instance runs anonymous-only (contributor/CI default). RBAC: `rbac_grant(user, resource_kind, resource_id, role)` with only OWNER minted and a `private` flag on shares defaulting false — private shares and EDIT/READ later are data changes. Everything else stays anonymous: local notebooks, immutable shares, public notebooks, agent sessions, compilation. `get_auth_info` server fn feeds the footer (sign-in link ↔ avatar + handle + sign-out).

**Mutable shares (PRD-0054: server-authoritative draft/published)**. ONE address per published notebook: `/mutable/{id}` is the view-only reader of PUBLISHED for everyone and the live editor over the server-side DRAFT for the owner (auto-swapped on hydrate; `?view=reader` pins the reader; SSR always renders the reader so crawlers/readers can never see draft content). The `mutable_share` record carries `notebook_json` (published), `draft_json` (option; `None` = clean), and the manifest. The editor gains a storage seam (`NotebookState.server_draft_share`): Local mode persists to IndexedDB exactly as before; ServerDraft mode debounce-autosaves the draft (1.5s, epoch-coalesced, retry-on-failure, `save_mutable_draft`) with a "Saving draft…" indicator. The toolbar **Push button** is the one editorial act: grayed "Published ✓" when clean, "⬆ Push" when the draft differs; Push flushes + durable-saves the draft, then `push_mutable(id, tags)` promotes it server-side (uploads NOTHING) and snapshots blobs from the draft at that moment. Push ABORTS loudly if the durable save fails (`persist_notebook_durable` returns success; a `draft_save_inflight` counter drains in-wire autosaves so the durable write lands last before the promote). Draft autosaves count toward `MAX_TOTAL_MUTABLE_BYTES`. The share/publish/push/discard/unpublish/download workflows live in `notebook_editor/sharing.rs` with ONE flush-before-serialize helper (Unpublish once skipped the flush and lost the last debounce-window of typing before deleting the only other copy; Download serializes from the live model — a published notebook has no IndexedDB record). Menu: "View as Reader", confirm-gated "Discard Draft" (clears `draft_json`), Unpublish (saves current content into the private IndexedDB store FIRST, then deletes the share, hard-navigates to `/local/{uuid}`). Share Mutable uploads, DELETES the local copy, and hard-navigates to `/mutable/{id}`. There is NO local mutable store anymore (storage.js DB_VERSION 5 deletes it): home lists three sources with zero reconciliation (private = IndexedDB, Published = `list_mutable_shares()` by session, public = server scan), and the divergence banner, Pull Latest, clone-to-local, and the share binding are all deleted. Concurrency is last-write-wins on the draft at autosave granularity (documented, not solved). Server fns: `create_mutable_share`, `save_mutable_draft`, `get_mutable_for_edit` (owner-gated; draft + dirty), `push_mutable` (promote; `Ok(false)` = already clean), `discard_mutable_draft`, `get_mutable_notebook` (readers; published + attribution + is_owner), `get_mutable_manifest`, `delete_mutable_share`, `list_mutable_shares`. Published notebooks are online-only to edit; private notebooks stay local-first.

**Persisted cell outputs (PRD-0056)**: view-only pages render each cell's `saved_output` (panels JSON from the author's last run) as the initial output state — dashed border + a badge saying it is serialized and that Run executes it live — replaced by the first live result. Capture is enrichment of the OUTGOING JSON at the editorial moments only (`embed_saved_outputs` in ironpad-common: Share Immutable/Mutable via `flush_serialize_tags`, the pre-Push durable save via `save_draft_now(_, true)`, Download): the model and debounced autosaves never embed, and a session with no fresh capture PRESERVES prior snapshots rather than stripping them (the /mutable editor after its hard navigation is the case that bit). 256 KiB per cell; over-budget degrades to a placeholder panel. `PanelMode::Snapshot` renders Simulation as its embedded first frame and LiveView as its captured content, statically through the same sanitizers; Animation replays as-is (frames are embedded). Display-only: piping bytes are never persisted.

**Blob delivery (PRD-0047)** — two cache layers in front of `compile_cell`:
- **Share snapshots**: at share time the server recomputes each runnable cell's cache key (from the sharer-supplied positional type tags) and copies CACHE HITS into `{data_dir}/shares/blobs/{content_hash}.wasm/.js` plus a `{share_hash}.manifest.json` sidecar. Viewers of `/shared/*` and `/embed/shared/*` fetch blobs from the immutable `/share-blobs/{hash}.{wasm,js}` route (axum handler in `ironpad-server/src/http_policy.rs`) instead of compiling — shares survive toolchain bumps and cache wipes, and viewers can never trigger builds. Misses are skipped (never compiled at share time); everything degrades to the live pipeline.
- **Local blob cache**: the browser keeps a content-addressed IndexedDB store (`blobs` object store in `public/storage.js`, LRU-capped; Rust side in `ironpad-app/src/blob_cache.rs`). Keys come from the SAME recipe as the server — `ironpad_common::cache_key` (moved there so both targets share it; `CACHE_EPOCH` now lives in `cache_key.rs`) plus the server fingerprint from `get_toolchain_fingerprint()` — so a deploy/toolchain bump invalidates local entries for free. The Force Recompile toggle bypasses every layer (share snapshot, local store, server cache) and its fresh result overwrites the local entry.

**Routes** (canonical scheme, PRD-0048 — the prefix names the storage class):
- `/` — HomePage (lists private IndexedDB notebooks + public notebooks)
- `/local/{id}` — NotebookEditorPage (private, IndexedDB-backed; dashed UUID)
- `/public/{name}` — PublicNotebookPage (read-only, static `.ironpad` file; extension-less URL, `get_public_notebook` appends the extension)
- `/shared/{hash}` — SharedNotebookPage (read-only, immutable, shared via 16-hex content hash)
- `/mutable/{id}` — MutableNotebookPage (PRD-0054: reader of published for everyone; the owner's draft editor on hydrate; `?view=reader` pins the reader)
- `/auth/github`, `/auth/callback`, `/auth/logout` — GitHub OAuth sign-in (PRD-0053); `/auth/test-login` exists only under `IRONPAD_TEST_AUTH`
- Legacy `/notebook/{id}` and `/notebook/public/{filename}` redirect to canonical forever (bookmarks + third-party embed specs never break)
- `/embed/shared/{hash}` — EmbedSharedPage (chrome-less iframe variant; PRD-0039)
- `/embed/public/{filename}` — EmbedPublicPage (chrome-less iframe variant; PRD-0039)
- `/embed/mutable/{id}` — EmbedMutablePage (PRD-0057: published copy, live resolve, never autoruns)
- `/og/{class}/{id}.png`, `/og/ironpad.png` — generated social-preview cards (PRD-0050)
- `/robots.txt`, `/sitemap.xml` — crawler files (PRD-0050)
- `/oembed?url=…` — oEmbed provider (PRD-0051); maps a `/public`, `/shared`, or `/mutable` URL (PRD-0057) to its `/embed/*` iframe. Origin-locked
- `/ws/host?notebook_id=<id>` — WebSocket: browser connects as session host
- `/ws/connect?token=<token>` — WebSocket: CLI connects as session guest

**Social previews (PRD-0050)** — what a pasted link looks like on Reddit, X, Slack, and Discord. `SocialMeta` (`ironpad-app/src/components/social_meta.rs`) emits `<title>`, `og:*`, and `twitter:card=summary_large_image` on `/`, `/public`, `/shared`, and `/mutable`; the card image is generated server-side from notebook metadata (`ironpad-server/src/og/`: `text.rs` fonts + metrics, `svg.rs` pure layout, `mod.rs` extraction/cache/handlers), rasterized with `resvg`, and cached at `{data_dir}/og/{blake3-of-svg}.png`. Sharp edges, all of which have already bitten:
- **`SsrMode::Async` on the three notebook routes is load-bearing.** Their titles come from a `Resource`; under the default out-of-order streaming the `<head>` is flushed first and `leptos_meta` patches the tags in with a script — correct in a browser, invisible to every unfurler, since none of them run JavaScript. **Test against raw response bodies, never the hydrated DOM.**
- **Fonts are `include_bytes!`-embedded** (Inter + JetBrains Mono under `crates/ironpad-server/assets/fonts/`) and `resvg` is built with `default-features = false, features = ["text"]` — no `system-fonts`. The runtime image is `rust:slim` and ships no fonts, so discovery works on a dev box and finds nothing in prod.
- **`og:image`/`og:url` must be absolute**, hence `IRONPAD_PUBLIC_URL` (`AppConfig::public_url` + `absolute_url`), set in `.hidden/fly.toml`. Client-side the origin falls back to `window.location.origin`.
- **Unlisted ≠ blocked.** `/shared` and `/mutable` carry `<meta name="robots" content="noindex">` rather than a `robots.txt` `Disallow`, because several unfurlers honour robots.txt and would then refuse to build a preview at all. `robots.txt` disallows only `/embed/` (duplicate content).
- **Notebook text is attacker-controlled** on `/shared` and `/mutable`: it is XML-escaped before reaching the SVG, and a notebook's optional `og_image` override is forced root-relative by `IronpadNotebook::og_image_path()` so a share cannot point a crawler at another origin.

Cell I/O uses **bincode 2.0** serialization for piping output between cells.

**Dependency-aware cascade (PRD-0060)**: a cell consumes upstream outputs ONLY through the injected `cellN` bindings and the `last` alias, so its dependency set is knowable from source text (`ironpad_common::cell_deps`: word-boundary `cellN` matching, `last` not preceded by `.`; false positives only over-cascade). Running a cell cascades exactly its unexecuted transitive dependencies; the scaffold binds only referenced slots (an unreferenced typed slot would eagerly deserialize empty bytes and panic), and `normalize_previous_types` runs INSIDE the cache hash so unreferenced upstream types cannot fork the key — an independent cell is cache-portable across notebooks. Run All / autorun / reactive queues CONTINUE past a failure: transitive dependents are dropped and marked blocked (editor `cell_blocked_by`, viewer blocked map + inline notice), independents keep running. `last` is conservative (depends on all upstream runnable; the alias target is dynamic). Cells compile on **edition 2024** (`gen` is a reserved keyword there — user code naming a variable `gen` gets rustc's rename-or-`r#gen` error), and `cell_main` is a one-line trampoline into a plain inner fn because `#[wasm_bindgen]`'s syn parser lags rustc on new syntax (it rejects `gen { … }` outright). `CACHE_EPOCH` 9.

**Private mutable shares (PRD-0061)**: the owner flips `mutable_share.private` and grants READ to GitHub handles (`rbac_grant` role `READ`; targets must have signed in once — grants link to `user` records and a bare login cannot be resolved server-side). `mutable_access_core(db, id, viewer)` is the ONE gate: reader page and embed render an explicit denial (sign-in prompt when anonymous — never a soft 404 for a share that exists), the manifest is withheld (it is the hash list; content-addressed `/share-blobs/` stays ungated because hashes are only knowable from a manifest), and the anonymous surfaces (OG card, oEmbed) 404 via `get_mutable_notebook_core` returning `None` for private. Access UI lives in the metadata panel (`ShareAccessSection`, ServerDraft mode only). Cross-site iframes carry no SameSite=Lax cookie, so embeds of private shares deny by construction.

**Shared cells (PRD-0044)**: a cell with `shared: true` renders amber inline, never executes, and its source is appended to the notebook's `shared.rs` after the notebook-level shared source. The assembly is `ironpad_common::effective_shared_source` — used by the editor, the view-only runner, and the notebook gate; never assemble it by hand. Shared cells hold empty piping slots (`cellN` indices stay positional), and editing one stales ALL code cells. Caveat: shared-cell text feeds feature detection (simd/autodiff/rayon), so a shared cell mentioning `std::simd` opts every cell in the notebook into simd128.

### Agent Collaboration Architecture

The browser is the authoritative model server. The API server is a dumb relay. The CLI daemon keeps a warm WebSocket connection for fast agent interactions.

```
Browser (model) ←→ WS ←→ API Server (relay) ←→ WS ←→ CLI Daemon ←→ Agent
```

**Agent-triggered execution (PRD-0052)**: `ironpad cells run <cell_id>` executes a cell in the hosting browser and blocks until its terminal event. `Mutation::CellRun` rides the mutation envelope (write-permission gate) but is intercepted in `session/connection.rs` BEFORE `model.apply` — execution is an editor action, not a state mutation — and dispatched to the same `run_all_queue` Run All uses, so DEPENDENCIES cascade (PRD-0060: only cells the target transitively consumes via `cellN`/`last`, not everything upstream). The browser emits `CellCompiling`/`CellCompiled{success}`/`CellExecuted{success}` from the compile flow (session-gated via `NotebookModel::emit_event`; nothing is emitted without a live session); the CLI daemon fans events out on a broadcast channel and `cells.run` correlates by `cell_id`, terminal on `CellExecuted` (ours) and `CellCompiled{success:false}` (ours = `compile_error`). Queue drops are AUTHORITATIVE and ALWAYS reported: the browser emits `Event::CellBlocked{cell_id, blocked_by}` per dependent dropped on a failure (`CellRunCtx::fail_and_report` — drop and report are one act), `Event::RunCancelled{cell_ids}` when user-terminate clears the queue, and `CellDeleted` covers a target deleted mid-run (`cell_deleted`). The daemon terminal-izes on these; its dependency-INFERENCE fallback (same `ironpad_common::cell_deps` recipe against the cached notebook, kept for pre-v6 browsers) is grace-held (`INFERENCE_GRACE_MS`, `RunSignal::Inferred`) rather than returned — the cache can trail live Monaco buffers by the save debounce, so the verdict waits ~1.5s for the browser's authoritative report or for the target's own activity (`RunSignal::OursAlive` refutes it) before it stands. Independent failures keep the queue running and the wait open. `PROTOCOL_VERSION` 6.

**Build admission control (PRD-0052)**: `BuildAdmission` (compiler/admission.rs, context alongside `CompileLocks`) bounds cargo processes. Consulted only AFTER a confirmed cache miss — hits are free. Compiles queue for a slot (`--max-concurrent-builds`, default 3; queue bounded by `IRONPAD_BUILD_QUEUE_TIMEOUT_SECS`); live checks `try_acquire` a separate pool and degrade to `CheckStatus::Skipped`. Per-client token bucket on build starts keyed by `Fly-Client-IP`→`X-Forwarded-For`→`"local"` (`IRONPAD_BUILD_RATE_BURST`/`IRONPAD_BUILD_RATE_PER_MIN`). Queue wait shows as the `build_permit_wait` span.

Key modules:
- **`ironpad-common/src/protocol.rs`** — unified message protocol (mutations, queries, events, control messages)
- **`ironpad-server/src/sessions.rs`** — session store, token generation/validation, permission checking
- **`ironpad-server/src/ws.rs`** — WebSocket relay handlers
- **`ironpad-server/src/state.rs`** — shared server state (`AppState`, `WsState`)
- **`ironpad-app/src/model.rs`** — `NotebookModel` — all mutations go through here (same codepath for UI and agent)
- **`ironpad-app/src/session/`** — browser-side WebSocket session management
- **`ironpad-app/src/components/session_panel.rs`** — session UI (start/stop, token display, agent list)
- **`ironpad-cli/src/daemon.rs`** — CLI daemon (WS connection, Unix socket IPC, state cache)
- **`ironpad-cli/src/main.rs`** — CLI subcommands (cells list/get/add/update/delete/reorder/run, notebook get/update, status). `notebook update` sends a `NotebookMetaPatch` (tri-state per field: omitted flag = untouched, `--clear-*` = explicit-null clear, value = set), so agents can edit the notebook-level shared source/Cargo.toml, title, description, and tags; a shared-source/cargo change stales every code cell, same as the editor's panel

### Frontend Architecture

**Leptos** with SSR + hydration:
- Server renders HTML; client hydrates into WASM SPA
- `ironpad-app` split by feature flags: `ssr` (server), `hydrate` (client)
- Components: Monaco editor, executor bindings, error panel, layout, view-only notebook
- Pages: home, notebook editor, public notebook viewer, shared notebook viewer

**Key client-side APIs**:
- `window.IronpadMonaco.*` — Monaco editor JS bridge
- `window.IronpadExecutor.*` — WASM executor (cell loading/execution)
- `window.IronpadStorage.*` — IndexedDB storage (notebook CRUD, from `public/storage.js`)

---

## Common Tasks

### Adding a New Server Function

Current server functions: `compile_cell`, `check_cell` (live check-on-type, PRD-0045), `list_public_notebooks`, `get_public_notebook`, `share_notebook`, `get_shared_notebook`, `get_shared_manifest` (blob-snapshot sidecar, PRD-0047), `get_toolchain_fingerprint` (client cache keys, PRD-0047), the mutable-share set (PRD-0054, session-gated writes): `create_mutable_share`, `save_mutable_draft`, `get_mutable_for_edit`, `push_mutable` (draft promote), `discard_mutable_draft`, `get_mutable_notebook`, `get_mutable_manifest`, `delete_mutable_share`, `list_mutable_shares`, and `get_auth_info` (header auth surface, PRD-0053).

1. Add to `server_fns.rs` with `#[server]` attribute:
   ```rust
   #[server]
   pub async fn my_operation(param: Type) -> Result<Response, ServerFnError> {
       let config = expect_context::<AppConfig>();
       // ... implementation
   }
   ```

2. Call from client component:
   ```rust
   let result = my_operation(param).await?;
   ```

3. Test with Playwright if it needs e2e validation.

### Modifying Compiler Logic

1. Identify which stage: scaffold, cache, build, diagnostics, or optimize
2. Update the module and add/update unit tests
3. Run `cargo make test` to verify
4. If modifying diagnostics line mapping, test in `compiler/mod.rs::pipeline_tests`

### Adding UI Components

1. Create new file in `components/`
2. Define Leptos component with `#[component]` macro
3. Export from `components/mod.rs`
4. Use in pages (home_page or notebook_editor)
5. Add styling to `style/main.scss` (CSS custom properties + dark theme)

### Updating CLI Configuration

1. Modify `server/src/config.rs` (clap parser)
2. Update default values, env var names, help text
3. Add tests in config.rs for arg parsing
4. Update Docker environment in `docker/Dockerfile` if needed

---

## Debugging Tips

### Compilation Errors

Most compilation issues are caught by `cargo make ci`. For diagnostic details:

```bash
# Check a specific cell's output
RUST_LOG=debug cargo make dev
# Look for "cache hit", "cache miss", "compilation succeeded/failed"

# See full cargo output
cd /tmp/ironpad-e2e-test-{uuid}
cargo build --target wasm32-unknown-unknown --release 2>&1 | less
```

### Test Failures

```bash
# Run a specific test with output
cargo test --lib compiler::pipeline_tests::pipeline_hash_scaffold_diagnostics_round_trip -- --nocapture

# Run Playwright tests with visible browser
HEADED=1 cargo make playwright
```

### Runtime Issues

Enable detailed logging:
```bash
RUST_LOG=ironpad=debug cargo make dev
```

Check browser console (F12) for JS errors, especially:
- `IronpadMonaco` not found → Monaco setup failed
- `IronpadExecutor` not found → executor-bridge.js not loaded
- WASM trap errors → Cell runtime panic

---

## File Organization

### Hot-Edit Files

Files you'll frequently modify:

- **Compiler logic**: `crates/ironpad-app/src/compiler/*.rs`
- **Notebook model**: `crates/ironpad-app/src/model.rs`
- **UI components**: `crates/ironpad-app/src/components/*.rs`
- **Cell run pipeline**: `crates/ironpad-app/src/pages/notebook_editor/pipeline.rs` (compile/execute + live check); `sharing.rs` (share/publish workflows); `cell_item.rs` renders
- **Session management**: `crates/ironpad-app/src/session/`
- **Client storage**: `crates/ironpad-app/src/storage/*.rs` (IndexedDB bindings)
- **Server functions**: `crates/ironpad-app/src/server_fns.rs`
- **Pages**: `crates/ironpad-app/src/pages/*.rs`
- **WebSocket relay**: `crates/ironpad-server/src/ws.rs`, `state.rs`, `sessions.rs`
- **Protocol types**: `crates/ironpad-common/src/protocol.rs`
- **CLI daemon/commands**: `crates/ironpad-cli/src/`
- **Styles**: `style/main.scss`
- **Executor JS**: `public/executor-bridge.js` (worker lifecycle + fallback), `executor-core.js` (executor class/ABI), `executor-gpu.js` (WebGPU), `executor-glue.js` (env import table + glue rewriting)
- **IndexedDB JS**: `public/storage.js`
- **Public notebooks**: `public/notebooks/*.ironpad`

### Configuration Files

- **Workspace**: `Cargo.toml` (dependencies, profiles, workspace metadata)
- **Build tasks**: `Makefile.toml`
- **Frontend config**: `playwright.config.ts`, `package.json`
- **Docker**: `docker/Dockerfile`, `docker/docker-compose.yml`

### Generated Files (Do Not Edit)

- `target/site/` — Built frontend bundle
- `public/monaco/vs/` — Monaco dist (copied from npm)
- `Cargo.lock` — Lock file (commit if workspace changes)

---

## Testing Strategy

### Unit Tests
Most modules have in-crate `#[test]` functions:
- Compiler scaffolding, caching, diagnostics
- FFI memory management
- Input/output serialization

Run with: `cargo make test`

### Integration Tests
Full pipeline tests in `compiler/mod.rs::e2e_tests`:
- Compile trivial cell → valid WASM
- Compilation failure → correct diagnostics
- Cache round-trip

Marked `#[ignore]` (slow). Run with: `cargo make test-integration`

### E2E Tests
Playwright tests in `tests/e2e/`:
- Page loads (sanity)
- Notebook editing
- Cell compilation + execution

Run with: `cargo make playwright`

---

## Common Pitfalls

### 1. **Forgetting the preamble offset**
When working with diagnostics, always subtract the preamble offset:
```rust
let user_line = diagnostic.spans[0].line_start - preamble_lines; // returned by the scaffold
```

### 2. **Not Running cargo make before committing**
Always run `cargo make ci` (or at least `cargo make fmt && cargo make clippy`).

### 3. **Modifying ironpad-cell without version bump**
If you change the cell runtime API, the injected dependency version must match. Update in `scaffold.rs`.

### 4. **Blocking Operations on Async Runtime**
Server functions are async. Avoid sync blocking calls; use `tokio::task::spawn_blocking` if needed.

### 5. **Cache Invalidation**
If you change the compilation pipeline, the cache key (blake3 hash input) may need updating. Test with `cargo make test-integration`.

### 6. **Browser-Cache Hygiene for Static Assets**
The pkg bundle is content-hashed (cargo-leptos `hash-files`; `hash.txt` must sit next to the server binary or leptos panics — the Dockerfile keeps it in lockstep with `LEPTOS_HASH_FILES=true`). Every URL-stable `<script>`/`<link>` in the shell (`lib.rs`) must be wrapped in `versioned()` so it carries a `?v={release}` cache-buster, and a middleware in `ironpad-server/src/http_policy.rs` serves `/pkg/` as immutable and everything else `no-cache`. Without all three, browsers heuristically cache stale JS/WASM across releases, and an old client silently drops newer notebook fields (this shipped a live bug where shared cells compiled as normal cells).

---

## Development Workflow

### Typical Feature Branch

```bash
# Create branch
git checkout -b feature/my-feature

# Make changes
# ... edit files ...

# Run CI locally
cargo make ci
cargo make test-integration  # If touching compiler
cargo make playwright         # If touching UI

# Commit
git add .
git commit -m "feat: description"

# Push and open PR
git push origin feature/my-feature
```

### Code Review Checklist

- [ ] `cargo make ci` passes
- [ ] New tests added for new logic
- [ ] No `unwrap()` or `println!()` without justification
- [ ] Error messages are clear (use `.context()`)
- [ ] Code style follows conventions (spacing, naming, comments)
- [ ] If compiler changes, `cargo make test-integration` passes
- [ ] If UI changes, Playwright tests updated
- [ ] **If a public notebook's cell count or order changes, run `cargo make playwright`.** `cargo make ci` cannot see it. Several specs assert on notebook structure (`seed.spec.ts` hard-codes welcome's cell count; `public-notebooks.spec.ts` asserts collapsed-cell counts and autorun behavior), so adding or reordering a cell breaks e2e while unit tests stay green.

---

## Troubleshooting

### Build Issues

**"cannot find `ironpad_cell` in path dependency"**
- Check that `crates/ironpad-cell` exists
- Verify path in `scaffold.rs` matches actual location
- Run `cargo clean` and retry

**"WASM module instantiation failed"**
- Executor.js couldn't load Monaco or cell WASM
- Check browser console (F12) for 404s
- Verify `public/executor-bridge.js` (plus its worker/core siblings) and `public/monaco/` exist

### Runtime Issues

**"cache miss — compiling" takes 30+ seconds**
- First compile is slow (full cargo build + link)
- Subsequent identical cells hit cache instantly
- Check cargo registry cache setup (Docker `CARGO_HOME`)

**Diagnostics point to wrong line**
- Verify the preamble math in `scaffold.rs::generate_lib_rs` (base 7: use, shared line, attribute, trampoline, inner-fn header, panic hook, output-block open)
- Check `diagnostics.rs::adjust_span()` is subtracting correctly

---

## Key Dependencies

### Workspace

- **Leptos 0.8**: Full-stack web framework (SSR + WASM)
- **Axum 0.8**: Web framework (routes, middleware)
- **Tokio 1**: Async runtime
- **Blake3 1**: Content hashing (cache keys)
- **Bincode 2**: Binary serialization (cell I/O)
- **Serde 1**: Serialization framework
- **Clap 4**: CLI argument parsing

### Frontend-Only

- **wasm-bindgen 0.2**: Rust ↔ JS FFI
- **web-sys 0.3**: Browser APIs
- **js-sys 0.3**: JavaScript utilities

### Build Tools

- **cargo-leptos**: Leptos SSR builder
- **cargo-nextest**: Parallel test runner
- **cargo-make**: Task runner
- **Playwright**: E2E testing

---

## Further Reading

- **Dev guide**: `DEVELOPMENT.md` (setup, architecture deep-dive, contribution workflow)
- **Full PRD**: `MegaPrd.md` (historical product vision; current behavior is tracked in `.prds/`)
- **Per-module docs**: Each compiler module has inline `//!` documentation
- **Test examples**: Look at test cases for API usage patterns
- **Leptos docs**: https://leptos.dev

---

## System & Toolchain Notes

### Active Rust Toolchain

The project uses **nightly** (`nightly-x86_64-unknown-linux-gnu`) as the default toolchain. The `wasm32-unknown-unknown` target is installed on nightly.

### Global Cargo Config (`~/.cargo/config.toml`)

The user's global cargo config sets:

```toml
[build]
jobs = 128
rustc-wrapper = "/home/twitchax/.cargo/bin/sccache"

[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

**Impact on cell compilation**: The `build_micro_crate()` function in `compiler/build.rs` sets `CARGO_HOME` and `CARGO_TARGET_DIR` but does **not** override `RUSTFLAGS` or the global cargo config. This means:
- `sccache` wraps `rustc` for cell builds (if available)
- The `mold` linker config applies only to the native `x86_64-unknown-linux-gnu` target and does **not** affect `wasm32-unknown-unknown`
- The `wasm32-unknown-unknown` target spec uses `rust-lld` as its default linker (`"linker": "rust-lld"`, `"linker-flavor": "wasm-lld"`)

### Known Issue: `rust-lld` Linking Failures During Cell Evaluation

Cell compilation targets `wasm32-unknown-unknown`, which uses `rust-lld` as the default linker. On current nightly toolchains, `rust-lld` for this target no longer defaults to `--allow-undefined`, so any bare `extern "C"` block in `ironpad-cell` declaring a host-provided import (e.g. `ironpad_sim_read`, `ironpad_host_message`, the `ironpad_gpu_*` functions) fails to link with an error like `rust-lld: error: undefined symbol: ironpad_sim_read` — even though `cargo build` of the ironpad project itself succeeds (the host target uses `clang`+`mold` and isn't affected).

**Root cause**: the browser executor supplies these host functions under the WASM import module `env` (the generated table in `public/executor-glue.js`), but the `extern "C"` blocks in `ironpad-cell` (`sim.rs`, `lib.rs`, `gpu.rs`) didn't tell rustc which import module to target, so the linker couldn't resolve them once `--allow-undefined` stopped being the default.

**Fix**: annotate each host-import `extern "C"` block with `#[link(wasm_import_module = "env")]` (PRD-0031 T-001). This is a compile-time hint, not a linker flag — no `.cargo/config.toml` or `RUSTFLAGS` changes are needed. Regression coverage lives in `crates/ironpad-app/src/compiler/mod.rs::e2e_tests::compile_cell_with_host_imports_links_successfully`.

### Cell Special Cases (opt-in by usage)

Cells opt into heavier build modes just by using a feature; detection is substring-based over (source, shared_source):

| Feature | Trigger substrings | Build effect | Runtime effect |
| --- | --- | --- | --- |
| rayon (atomics) | `rayon` in merged deps | `ATOMICS_TOOLCHAIN` (`nightly-2025-12-22`), `-Zbuild-std`, atomics target features + shared-memory link flags | wasm-bindgen-rayon worker pool |
| autodiff (Enzyme) | `autodiff_forward` / `autodiff_reverse` / `std::autodiff` | `AUTODIFF_TOOLCHAIN` (`nightly-2026-06-01`) + `enzyme` component, `-Zautodiff=Enable`, forced fat-LTO profile, crate-root `#![feature(autodiff)]` | none |
| SIMD (PRD-0042) | `std::simd` / `core::simd` / `std::arch::wasm32` (comments count) | `-C target-feature=+simd128`, crate-root `#![feature(portable_simd)]`; no std rebuild | none (all modern browsers) |
| blocking/JSPI (PRD-0043) | imports `ironpad_blocking_*` (via `ironpad_cell::blocking`) | none (plain imports) | executor wraps imports in `WebAssembly.Suspending`, enters raw `cell_main` via `WebAssembly.promising`; Chrome/Edge 137+ only, friendly gate elsewhere |
| coroutines | `#[coroutine]` / `CoroutineState` / `ops::Coroutine` | crate-root `#![feature(coroutines, coroutine_trait, stmt_expr_attributes)]`; no toolchain change | none |

Coroutine detection (`uses_coroutines` in `ironpad-common/src/cache_key.rs`) deliberately omits a bare `yield`, which is ordinary English and would opt prose-heavy cells into a feature gate. Nothing is lost: rustc rejects `yield` outright without the attribute (*"`yield` can only be used in `#[coroutine]` closures, or `gen` blocks"*), so every real coroutine contains one of the three matched strings. `gen` blocks (which use `yield` without `#[coroutine]`) have their own gate: `uses_gen_blocks` matches the block forms (`gen {`, `gen move {`, `async gen`) — never bare `gen` — and injects `#![feature(gen_blocks)]` with its own preamble bump.

**Toolchains**: cell builds pin their toolchain explicitly via `cell_toolchain` (in `compiler/build.rs`), never the host default (dev boxes, CI, and the deploy image all differ, and that divergence once shipped cells that compiled locally and failed on prod). Three pins:
- **`CELL_TOOLCHAIN`** (`nightly-2026-07-14`) — normal + SIMD cells, the common case, tracked fresh. Defined ungated in `crates/ironpad-app/src/lib.rs` and displayed in the footer.
- **`AUTODIFF_TOOLCHAIN`** (`nightly-2026-06-01` + `enzyme`) — `std::autodiff` cells. Held back because July 2026 nightlies ICE on autodiff typetrees for slices; also carries `rust-src` for the autodiff+rayon `-Zbuild-std` combo.
- **`ATOMICS_TOOLCHAIN`** (`nightly-2025-12-22`) — rayon cells (wasm-bindgen-rayon's atomics guard breaks on newer nightlies).

`AUTODIFF_TOOLCHAIN` wins over `ATOMICS_TOOLCHAIN` when both apply. The cache fingerprint describes only `CELL_TOOLCHAIN`'s rustc (plus the wasm-bindgen CLI), so bumping `CELL_TOOLCHAIN` invalidates all blobs automatically — but bumping `AUTODIFF_TOOLCHAIN`/`ATOMICS_TOOLCHAIN` needs a `CACHE_EPOCH` bump. **In the deploy image those two version strings are BAKED at build time** (`docker/Dockerfile` writes raw `--version` output to `/app/toolchain-versions`; `IRONPAD_TOOLCHAIN_VERSIONS_FILE` points the server at it) and read back through the same parsers used on live output, so the fingerprint is byte-identical to the probed one and cached blobs survive. Probing it at runtime cost a cold `rustc --version` that demand-pages ~350 MB of libLLVM + librustc_driver: **5.9s of a 7.2s Fly cold start**, for a string fixed when the image was built. Anything unset/missing/malformed falls back to probing, so dev boxes and CI are unchanged; the boot log's `toolchain fingerprint source=` field (`baked` vs `probed`) says which path a given deploy took. Server startup no longer awaits that warm-up either — `prewarm` is detached so the listener binds first. `docker/Dockerfile` and `.github/workflows/build.yml` must install all three pins (the image drops stable entirely). Autodiff cells carry distinct RUSTFLAGS + a fat-LTO profile, so they never shared the Docker default-target warmup; they're warmed post-deploy by the runtime cache warmer.

Sharp edges: target features from independent concerns must merge into ONE `-C target-feature=` flag (rustc keeps only the last; see `compose_rustflags` in `build.rs`); each injected crate-root gate bumps `preamble_lines` by one for diagnostics mapping; all detection booleans are part of the blake3 cache key; a cell whose text contains the literal `.await` substring anywhere (even in a string) compiles to an async wrapper, which the JSPI promising path cannot enter — keep `blocking::*` cells free of it.

### PRDs

- **PRD files live in `.prds/`** with YAML frontmatter (currently `PRD-0001` through `PRD-0051`)
- **`MegaPrd.md` (repo root)** holds the original product vision; treat it as historical
- **Review notes live in `.prds/reviews/`**; agentic-collaboration specs live in `.prds/agentic-collaboration/`

---

**Last Updated**: 2026-08-07 — Agent-session control is an icon-only square in the toolbar's right-hand group (menu/gear/close) with a corner guest-count badge; its wording lives in the tooltip + aria-label, which an icon-only button needs anyway (the svg is aria-hidden). That removes the reflow at the source and the toolbar is ONE row again — the second row stopped the button shoving Push sideways but spent a whole row on a control most sessions never touch. `session.spec.ts` asserts the invariant (button width before connecting an agent == width after the badge appears), not the old layout. Header theme toggle drops to 24px squares via `--compact`: `.ironpad-theme-toggle` is SHARED with the Cached/Fresh cache toggle in the view-only toolbar, which sits beside Run All and must stay 34px (that two unrelated controls share a "theme toggle" class is a real naming problem, deliberately left). The first version of that shrink was INERT — `height: 24px` against the shared pill rule's `min-height: 34px`, and min-height wins; 118 green specs missed it because none measured it, so the reset lives in `ip-icon-button-shape` (any square from that mixin is beatable the same way) and embed.spec.ts now measures BOTH halves of the split. PREVIOUSLY — Icon-sweep completion (found by driving the DEPLOYED site, not by reading the diff): `PRIVATE` and `PUBLIC` both point at one Lucide diamond and the filled-vs-outline reading (`◆` vs `◇`) lives only in `.ironpad-notebook-badge.private svg { fill: currentColor }` — that rule was never written, so the two badges rendered byte-identical and the home page could not tell its storage classes apart (icons.rs claimed a `--filled` class preserved it; the class did not exist). An audit of all 55 roles found the only two with no call site, both from mapping by GLYPH SHAPE rather than by affordance: `ADD` claimed `⊞`, which was really "⊞ Export HTML" (already `EXPORT`), while the actual add buttons kept an ASCII `+`; `REFRESH` claimed `⟳`, whose three uses were already `PUBLISHED`/`RERUN`/`PENDING` (deleted). `glyph-check` gained an ASCII arm — a character does an icon's job as well as an emoji does, and the U+2000 floor was blind to it; a LABEL is whitespace-then-a-word with no code punctuation, which distinguishes it from generated Rust/HTML string literals that also open on punctuation. Renaming those labels broke 52 specs through `createNotebook`'s literal "+ New Notebook" filter, so add-cell selectors are now `ADD_CODE`/`ADD_MARKDOWN` in `tests/e2e/helpers/session.ts` (class, not prose — and `hasText: "Code"` would now also match the cell's Code/Cargo.toml tab). PREVIOUSLY — UI consistency pass: ONE button geometry (`ip-button-shape`/`ip-icon-button-shape` mixins in `style/main.scss`, two size tiers as mixin defaults) replacing nine hand-authored paddings/fonts across Run All, session, Push, Fork, Embed, Edit, Restore, the toolbar icon squares, and both segmented controls; the notebook toolbar split into two rows (Run All + Push on top, session below — its label swings from "Start Agent Session" to "2 agents" and shoved Push sideways inline) with the draft indicator moved AFTER Push (its show/hide reflow slid the button out from under the cursor); auth header signed-in state is now the same portrait-over-label stack as signed-out (avatar over "Sign out", handle in the tooltip, shared `AuthSilhouette` fallback when a user has no GitHub picture); "Fork to Private" → "Fork" everywhere (the `fork_label` prop deleted — all four callers passed one string); "View as Reader" → "View Published" and the bottom-left toggle retitled "Preview" (they show PUBLISHED vs YOUR DRAFT and read as duplicates until the labels say so; both mode segments gained aria-labels, which is also what e2e now selects); Unpublish gained an "Unpublishing…" progress toast; `--ip-bg`/`--ip-bg-secondary`/`--ip-text` were NEVER DEFINED, so the version-history panel rendered transparent over the notebook — fixed, plus `tools/css-vars-check.py` in `ci` (an undefined var() with no fallback drops the whole declaration silently, and neither clippy nor Playwright could see it); `glyph-check`'s box-drawing exemption removed (it was hiding three live `╳` affordances and only ever fired on false negatives) with `icons::DELETE`/`icons::SESSION` added. PREVIOUSLY — Second-pass fanout fixes (post-remediation): disposal-safe blame helpers (`try_with_untracked` — the W1 unification had turned a disposal-safe write into a panicking read; navigating away from an autorunning viewer mid-compile aborted the whole wasm app), `NotebookState::scrub_deleted_cell` shared by UI delete AND agent CellDelete (ghost queue fronts stalled Run All) + `merged_run_queue` never pins a front absent from cells, `Event::RunCancelled` (inside the unshipped v6 bump) reports user-terminate drops and daemon inference is grace-held (`RunSignal` Terminal/Inferred/OursAlive, `apply_signal`, `INFERENCE_GRACE_MS` 1.5s — a stale-cache guess defers to the browser's report or the target's own activity), capture zero-capture guard (0 captured against a snapshot-bearing file = keep + exit 1; `--allow-empty` overrides; FAILED arm exits nonzero), manifest re-serialized in `recordManifest` key order, `CellRunCtx::fail_and_report` folds the three drop-report loops, `tests/e2e/helpers/menu.ts` (MENU/menuClick, re-exported from mutable.ts) adopted by ALL specs + `shareMutable` replaces the two remaining weak inline copies; Fanout-review remediation (post-v0.18.0, three waves): W1 cascade cluster — queue/blame bookkeeping shared via `run_flow` (`fail_in_queue`/`clear_own_blame`/`clear_blame_held_by`; editor and viewer delegate, sticky-⛔-after-successful-rerun fixed, `delete_cell_fn` cleans both blame sides), cascade MERGES into an in-flight queue (`merged_run_queue`: pinned front + notebook-ordered union; replacing dropped queued independents), `Event::CellBlocked` (PROTOCOL_VERSION 6) makes browser queue-drops authoritative for `cells.run` (inference kept as pre-v6 fallback) and `CellDeleted` for the target is terminal (`cell_deleted`); W2 capture tooling — seeded scratch copies stripped of prior `saved_output` (preserve-on-no-capture can't re-ship stale snapshots under fresh hashes; set/delete write-back on every path), freshness manifest widened to `{cells: sha256(source NUL cargo_toml), shared: sha256(shared context)}` in BOTH tools (shared-cell/shared-source/cargo_toml edits now flag stale), zero-runnable notebooks get manifest entries (markdown-only notebook no longer bricks ci); W3 sweep — `private_share_readable` is the one private-gate predicate (notebook + manifest cores; manifest signed-in arms unit-tested), whitespace-tolerant `uses_gen_blocks` (`gen{` no longer dead-ends on an unreachable E0658 gate), `run_flow::sleep_ms` the one sleeper, `tests/e2e/helpers/mutable.ts` (MENU/menuClick/shareMutable shared by four specs), `ShareAccessSection` → `share_access.rs`; OG/oEmbed cache TTL after flip-to-private deliberately left (review decision); Run-flow unification (PRD-0059): one blob-acquisition/execute engine (`components/run_flow.rs`, snapshot -> local probe -> compile with transport retries) consumed by editor + viewer, editor gains retries; Dependency-aware cascade + continue-past-failures (PRD-0060): `ironpad_common::cell_deps`, referenced-slot-only scaffold bindings, key normalization inside the hash, edition-2024 cells (the v0.17.0 gen_blocks gate was inert on 2021 — never deployed), `cell_main` trampoline shielding user tokens from wasm-bindgen's syn, CACHE_EPOCH 9, daemon prerequisite_failed now dependency-aware, borrows/dynosaur finally capture outputs; gen-blocks public notebook (46th; movable-vs-static coroutine story, E0626 teaching cell); capture-outputs cargo-make task + sha256 freshness manifest checked in ci; Private mutable shares (PRD-0061): private flag + READ grants by GitHub handle, one access core gating reader/embed/manifest/OG/oEmbed, Access UI in the metadata panel; /local version history (PRD-0058, unreleased): IndexedDB `history` ring (DB_VERSION 6, 5-min buckets, cap 30, dies with the notebook), History panel + confirm-gated undoable Restore; /embed/mutable + oEmbed for published notebooks (PRD-0057); gen_blocks feature gate; Persisted cell outputs (PRD-0056, unreleased): `IronpadCell.saved_output` + `embed_saved_outputs` (preserve-on-no-capture), capture at Share/Push-durable/Download, `PanelMode::Snapshot` with static Simulation/LiveView arms, `.view-only-saved-badge` serialized-output affordance, `PROTOCOL_VERSION` 5; SOC refactors (PRD-0055, unreleased): `notebook_editor/pipeline.rs` (CellItem's compile/execute pipeline + live-check dispatch behind a `CellRunCtx`; cell_item.rs 2,073 → 1,480), executor JS split (`executor-gpu.js` WebGPU runtime, `executor-glue.js` env table + glue rewriting, `executor-core.js` 1,480 → 1,070 keeps class/ABI/sim-bus; loaders updated in both contexts, env sync test retargeted to the glue file), `main.rs` split into `cache_valve.rs`/`http_policy.rs`/`otel.rs` bin modules (main.rs 845 → ~330); Fanout-review remediation, three waves (v0.16.2): W1 — reactive route params on all five notebook pages (same-route navigation used to keep rendering/EDITING the old notebook; markdown cross-links between public notebooks hit it), `notebook_editor/sharing.rs` extraction with one flush-before-serialize helper (fixes Unpublish losing the last ~1s of typing, Push promoting a stale draft behind a success toast when the durable save failed, Download .ironpad silently no-op in the mutable editor; orphaned exportNotebook seam deleted at all three layers), draft autosaves counted toward `MAX_TOTAL_MUTABLE_BYTES`; W2 — `run_group_with_timeout` (build/check/wasm-opt/wasm-bindgen share one process-group kill discipline; orphaned rustc children died with cargo), sliding session cookie actually slides (`session_user` reports renewals, `current_user` re-issues Max-Age; cookie mint/clear + TTL single-sourced in `ironpad_app::auth`), expired sessions disconnect their guests (`WsState::sweep_expired_sessions`), `WidgetSink` carries runnability (button widgets stalled the view-only queue on shared cells), `cells.run` terminal on runtime prerequisite failures, session panel teaches the real CLI command, executor-worker onerror terminates-then-respawns with a rapid-failure cap (main-thread fallback, e2e-tested via synthetic ErrorEvents), one generated env host-import table in executor-core.js (+ sync test against ironpad-cell's extern blocks), toolchain-pin sync test, `assemble_cell_inputs`/`unexecuted_upstream`/blob-cache probe policy shared editor↔viewer, `ironpad_common::notebook_ops` shared model↔daemon; W3 — viewer per-cell Run cascades unexecuted upstream cells, dotted `[dependencies.X]` subtables parsed (merge/rayon-detection/ironpad-cell filter), `PROTOCOL_VERSION` 4 (`ErrorCode::Unknown`, explicit-null cargo_toml clear survives the wire), daemon init snapshot cached on the recv path (event-loss race), GPU readbacks claim distinct panels, `safe_redirect_path` rejects control chars, promote records byte (not char) sizes, wasm-opt temp cleanup on all paths, duplicated defaults single-sourced; Draft/published mutable shares (PRD-0054): server-authoritative drafts (`draft_json` slot, debounced epoch-coalesced autosave, promote-on-Push with `Ok(false)` for clean, Discard Draft, View as Reader), unified `/mutable/{id}` (owner editor swap on hydrate via `NotebookEditor` + `server_draft` prop; SSR always the reader), local mutable store deleted (storage.js DB_VERSION 5, three-source home listing), Push toolbar button (`Published ✓`/`⬆ Push`), unpublish saves-local-then-deletes; header sign-in placement fix (auth surface moved from the home-hidden status bar to the header far right); Accounts (PRD-0053): GitHub OAuth sign-in (`ironpad-server/src/auth.rs`, nest_service /auth), embedded SurrealDB (`ironpad-app/src/db.rs`, SurrealKV at {data_dir}/ironpad.db, user/session/mutable_share/rbac_grant tables, OWNER-only grants shaped for EDIT/READ later), sessions in an HttpOnly/Secure/SameSite=Lax cookie stored blake3-hashed with sliding 30-day expiry, env-gated /auth/test-login for e2e (IRONPAD_TEST_AUTH; absent otherwise, asserted by test), mutable shares rewritten onto the session's OWNER grant with content+manifest in the DB (two-key mechanism deleted: user key, notebook key, rebind form, derive_key, subtle), reader-page owner attribution + clone-to-local Edit, footer sign-in/avatar, storage.js DB_VERSION 4 in-place migration, get_auth_info + MutableNotebookResponse wire types, OG mutable cards via axum Extension(Db); CLI `notebook update` (PRD-0051 gap-closer): agents edit notebook-level shared source/Cargo.toml, title, description, tags via a tri-state `NotebookMetaPatch` (`--clear-*` = explicit-null clear; daemon deserializes the patch verbatim so the CLI tracks the protocol; PROTOCOL_VERSION unchanged); Progress toasts + action feedback (v0.14.2): Info intent, `Toaster::toast_after_reload` (sessionStorage across Pull's reload), Pull's Up-to-Date short-circuit, rebind/fork toasts; Mutable-share author round-trip (PRD-0049 follow-up): binding-aware `/mutable` reader ("✎ Edit" toolbar shortcut + menu swap on the authoring device, rebind form always verifies), author-only divergence banner backed by `IronpadNotebook::content_matches` (ignores `updated_at`), editor Pull Latest (confirm-gated local overwrite + reload) and View Published menu items, published URL + copy in the metadata panel, `ViewOnlyNotebook` `controls` slot prop; Agent-triggered execution + build admission control (PRD-0052): `ironpad cells run` (CellRun mutation intercepted pre-model, run-queue dispatch, session-gated execution events, daemon broadcast wait keyed by cell_id, PROTOCOL_VERSION 3), `BuildAdmission` (global cargo cap w/ bounded queue for compiles + Skipped-degrading checks, per-IP token bucket charged only on cache MISSES); Pipeline trace instrumentation: `#[tracing::instrument]` spans (always `skip_all` — payloads carry user source) across compile/check/share/mutable/OG request paths, root request spans named by route template via `otel.name` with query strings excluded (the `/ws/connect` token lived in the exported URI field), lock-wait and `spawn_blocking` rasterize spans, outcome fields (`cache`, `status`, `hit`) recorded on spans; span taxonomy table in DEVELOPMENT.md Observability; Notebook metadata + oEmbed (PRD-0051): `description`/`tags`/`og_image` are editable from a panel below the cell list (`pages/notebook_editor/metadata_panel.rs`), new `og_image_width`/`og_image_height` gated by `og_image_dimensions()` so an override stops advertising 1200x630, `NotebookMetaPatch` flattened into BOTH `Mutation::NotebookUpdateMeta` and `Event::NotebookMetaUpdated` (wire format unchanged; `apply_to` replaces the CLI daemon's hand-written mirror; `PROTOCOL_VERSION` 2), `explicit_null_is_a_clear` fixes doubled-option clears that serde silently decoded as "unchanged", oEmbed provider at `/oembed` + discovery links, CSP (`object-src`/`base-uri`/`form-action`; deliberately no `script-src` or `frame-ancestors`), public-notebook scan cached per process, single-flight card rendering, `absolute_url` wired; social-preview security patch: stored XSS via the raw `<title>` splice, quadratic `ellipsize`/`expand_tabs`, a card cache that never evicted, and soft-404s on the async routes (v0.13.1); Social previews (PRD-0050): per-page Open Graph/Twitter metadata behind `SsrMode::Async`, server-generated 1200x630 preview cards at `/og/*` via resvg with embedded fonts, `og_image` override, robots.txt + sitemap.xml, `IRONPAD_PUBLIC_URL`; Coroutine cells (`#[coroutine]` feature gate, fourth cell special case) and the `pin-coroutines` notebook; AI-authorship disclosure banners on all 14 `blog` notebooks; voice pass removing meta-discourse, self-certification, and contrast-definition across them (`voice-lint` skill in dotagent is the gate for public prose); AI-authorship disclosure banners on the 14 `blog` notebooks (`> *AI generated.*`; cannon reads `human edited`), meta-discourse/self-certification voice pass across all of them, ammonia 4.1.4 for RUSTSEC-2026-0213, Share Immutable button label (v0.12.17); Mutable shares (PRD-0049): author-updatable /mutable/{id} conversion, two-key (user + notebook) push auth hashed with domain-separated blake3, reader-page rebind, footer user key, home Published group (v0.12.16); Blog-notebook voice pass + cannon rewrite with real Enzyme IR/WAT excerpts, Prism wasm/llvm grammars (v0.12.15); Canonical routes /local, /public, /shared with legacy redirects (PRD-0048); notebook menu + close in view mode; Static blob delivery (PRD-0047): share-time blob snapshots + immutable /share-blobs route + client IndexedDB blob cache, cache-key recipe moved to ironpad-common (v0.12.14); Thaw removed: native UI primitives + owned toaster (v0.12.13); session teardown on page disposal + disposal-read guards (v0.12.12); view mode renders the public notebook renderer + code-wide papercut sweep (v0.12.11); per-cell collapse defaults (v0.12.10); live check-on-type + completions (PRD-0045); shared cells; unified toolchains
**Target Audience**: AI agents, developers contributing to ironpad
