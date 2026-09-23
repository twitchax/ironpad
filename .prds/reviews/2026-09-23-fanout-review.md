# Fan-out Code Review, 2026-09-23

A DRY / SOC / readability / performance review of the workspace at v0.23.3 (`59e33bc`), split into seven partitions. Each partition had one reviewer and one adversarial verifier, whose job was to refute each finding. That was 14 agents and about 3.3M tokens.

| Partition | Scope |
|---|---|
| compiler | `compiler/`, `cache_key.rs`, `cell_deps.rs` |
| server-fns-and-db | `server_fns.rs`, `db.rs`, app `auth.rs`, `blob_cache.rs`, `sanitize.rs`, `lib.rs` |
| server-crate | `ironpad-server/src` + its tests |
| editor | `pages/notebook_editor/`, `model.rs`, `session/`, `storage/` |
| components-and-pages | `components/`, the non-editor pages |
| cell-runtime-and-common | `ironpad-cell`, `ironpad-common` (minus the compiler files) |
| cli-js-tools-config | CLI, proxy, `public/*.js`, `tools/`, Makefile/Dockerfile/CI, SCSS DRY |

**Outcome:** 105 findings. The verifiers confirmed **62**, corrected **41** (partial: the core claim held, but scope, severity or fix changed) and refuted **2**. Six findings were reported twice from neighbouring partitions and are merged below, which leaves **97 distinct items**. Value is the verifier's 1-5 score. Effort is S/M/L.

| Wave | Items | Value 4-5 |
|---|---|---|
| Bugs found along the way | 15 | 5 |
| Single source of truth | 30 | 5 |
| Performance | 20 | 2 |
| Readability, dead code, and module boundaries | 28 | 1 |
| Test-code duplication | 4 | 0 |

## Wave 1: bugs found along the way

Not what the review was looking for, but each is a real defect, and most are one-line to one-function fixes. The three cell-runtime items (cell-gpu-1, cell-enzyme-1, cell-sim-1) change what the injected `ironpad-cell` does, and that source is not in the cache hash, so they share ONE `CACHE_EPOCH` bump.

### editor-1: Every save/share/preview flush re-applies every cell unconditionally, marking all code cells stale

*bug · confirmed · value 5 · effort S*

**Where:** `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:974` (also `crates/ironpad-app/src/model.rs:409`, `crates/ironpad-app/src/model.rs:446-450`, `crates/ironpad-app/src/pages/notebook_editor/mod.rs:610-617`, `crates/ironpad-app/src/pages/notebook_editor/mod.rs:359`, `crates/ironpad-app/src/pages/notebook_editor/mod.rs:855`, `crates/ironpad-app/src/pages/notebook_editor/mod.rs:1175-1178`, `crates/ironpad-app/src/pages/notebook_editor/sharing.rs:23`)

**Problem:** The save_generation flush effect runs in every CellItem and applies `CellUpdate { source: Some(src), cargo_toml: Some(Some(toml)) }` whether or not anything changed. `cell_update` sets `content_changed = source.is_some() || cargo_toml.is_some()` (model.rs:409) and calls `mark_downstream_stale`. A shared cell escalates that to `mark_all_code_cells_stale`. The flush fires on Ctrl+S, Share Immutable/Mutable, Push, Save to Account, Download, Export and the Preview toggle (twice). Each one leaves every executed code cell showing the stale indicator, bumps every cell's OCC version, and buffers a full-source CellUpdated event per cell for agents. Markdown cells also gain `cargo_toml: Some("")`. In reactive mode the stale-watcher at mod.rs:610 then re-runs the whole notebook on a plain Ctrl+S. On Preview the queue it sets is never drained, because the CellItems are unmounted, so the run fires as a surprise on return to Edit. request_cell_run (state.rs:287-297) deliberately guards against exactly that surprise.

**Fix:** (1) cell_item.rs flush effect (~line 962): after the generation check, `let src_dirty = source_dirty.get_untracked(); let toml_dirty = cargo_toml_dirty.get_untracked(); if !src_dirty && !toml_dirty { return; }`, then send `source: src_dirty.then(|| source.get_untracked())` and `cargo_toml: toml_dirty.then(|| Some(cargo_toml.get_untracked()))`. On Ok, clear only the flags that were sent. (2) Fix the root cause in model.rs `cell_update`: fold the existence check, the `was_shared` lookup and a new content comparison into ONE `self.notebook.with_untracked` over the found cell: `content_changed = source.as_ref().is_some_and(|s| *s != cell.source) || cargo_toml.as_ref().is_some_and(|t| *t != cell.cargo_toml)`. Keep the version bump and the CellUpdated event unchanged, so the wire behaviour does not change. A no-op CellUpdate from any client then stales nothing. (3) Follow-up to note, not required here: the external-edit effect should also refresh `cargo_toml` from the model when `!cargo_toml_dirty`. Otherwise a Run after an agent Cargo.toml edit compiles with the stale local value (pipeline.rs:192).

**Tests:** Existing: model.rs `cell_update_persists_collapse_defaults` (uses notebook_with_one_cell) and stale_tests. Add in model.rs tests: (a) `identical_source_update_stales_nothing`: load notebook_with_one_cell, apply CellUpdate with the cell's current source and cargo_toml, and assert `cell_stale` is empty and the version still bumped. (b) `changed_source_update_stales_downstream`: the positive control. Add a Playwright case (execution.spec.ts or keyboard.spec.ts): run a cell, press Ctrl+S, assert `.ironpad-stale-indicator` count is 0. Optionally, in reactive mode, assert no second compile fires. Add a session.spec.ts case: an agent `cells update --cargo-toml`, then a host Ctrl+S, and the model keeps the agent's Cargo.toml.

### cp-1: Rail run state is never written, so its status dots, timings and Runtime totals never change

*bug · confirmed · value 5 · effort M*

**Where:** `crates/ironpad-app/src/components/notebook_rail.rs:100` (also `crates/ironpad-app/src/components/notebook_rail.rs:115`, `crates/ironpad-app/src/components/notebook_rail.rs:126`, `crates/ironpad-app/src/components/notebook_rail.rs:133`, `crates/ironpad-app/src/components/notebook_rail.rs:529`, `crates/ironpad-app/src/components/view_only_notebook.rs:138`, `crates/ironpad-app/src/components/view_only_notebook.rs:330`, `crates/ironpad-app/src/components/view_only_notebook.rs:785`, `crates/ironpad-app/src/components/view_only_notebook.rs:868`, +4 more)

**Problem:** `RailRunState` has a private field, and its only writers are `set`, `set_status` and `set_ran`. None of the three has a caller anywhere in the crate. `ViewOnlyNotebook` creates it at view_only_notebook.rs:141 and hands it to `NotebookRail`, but never passes it to `ViewOnlyCell`, `ViewOnlyCodeCell` or `ViewOnlyLinuxCell`. Every code row therefore stays on the `not-run` dot with no timing forever. `run.totals()` is always `(None, None)`, so the whole Runtime compile/total-run stats block (notebook_rail.rs:529-547) is dead markup. PRD-0065 T-006 ('status dots, timings') is marked done anyway, and no e2e test asserts any rail state except `--prose`. The comment at view_only_notebook.rs:138-140 ('falls back to a not-run default when nothing writes here') describes what happens today, not a design.

**Fix:** Mirror each cell's existing local signals into the rail with one Effect per cell. That is more robust than writes scattered across the four transitions in the async flow, where it is easy to miss one (for example the early `compiling` guard or the failure arm).
1. Add a prop `rail_run: RailRunState` (Copy) to `ViewOnlyCell`. Pass it from the loop at view_only_notebook.rs:510 (`rail_run=rail_run`) and forward it to `ViewOnlyCodeCell` and `ViewOnlyLinuxCell`. Shared, markdown and inert cells ignore it.
2. In `ViewOnlyCodeCell`, after the signals are declared (~:706), add:
`let rail_id = cell.with_value(|c| c.id.clone());
Effect::new(move || { let status = if compiling.get() { RailCellStatus::Running } else if error_message.with(Option::is_some) { RailCellStatus::Failed } else if blocked_by.with(|m| m.contains_key(&rail_id)) { RailCellStatus::Blocked } else if execution_result.with(Option::is_some) { RailCellStatus::Ran } else { RailCellStatus::NotRun }; rail_run.set(&rail_id, RailCellRun { status, compile_ms: compile_time_ms.get(), run_ms: execution_result.with(|r| r.as_ref().map(|r| r.execution_time_ms)) }); });`
This is safe because `execution_result`/`compile_time_ms` are not cleared at run start, so timings survive the Running state, which is the rule `set_status` exists for.
3. Do the same in `ViewOnlyLinuxCell`: `stage.get().busy()` gives Running, `error_message` set or `outcome.failed()` gives Failed, `outcome.started()` gives Ran, `compile_ms: compile_time_ms.get()`, `run_ms: None`.
4. `set_status`/`set_ran` then have no callers. Delete them, or leave them documented as the imperative API, whichever the implementer prefers. `RailCellRun`/`RailCellStatus` must be `pub` imports in view_only_notebook.rs.
Fix the misleading comment at view_only_notebook.rs:138-140 while there.

**Tests:** Nothing guards this today. Add to tests/e2e/studio-chrome.spec.ts: open a /public notebook (autorun=true), wait for the first code cell's `.view-only-output`, then assert its rail row has `.ip-rail-dot--ran` and a timing, and that `.ip-rail-stats` is visible. Add a negative-direction check that a markdown row still carries `--prose`. For an optional failure case, the `borrows` notebook's deliberate compile-fail cell should show `--failed`. Add a unit test in notebook_rail.rs tests that `totals()` sums across entries after `set`, since nothing covers it now.

### server-4: 4 MiB WS frame cap assumes one cell per message, but NotebookGet returns the whole notebook; an oversized reply tears down the host

*bug · confirmed · value 4 · effort S*

**Where:** `crates/ironpad-server/src/ws.rs:34` (also `crates/ironpad-server/src/ws.rs:31`, `crates/ironpad-server/src/main.rs:41`, `crates/ironpad-common/src/protocol.rs:453`, `crates/ironpad-cli/src/daemon.rs:161`, `crates/ironpad-app/src/server_fns.rs:725`)

**Problem:** MAX_WS_MESSAGE_BYTES is hard-coded to 4 MiB. Its justification (ws.rs:31-33) is that 'a single mutation/event carries at most one cell's source + metadata'. That is not true for host replies: `Response::Notebook { notebook }` (protocol.rs:453) carries the entire notebook. The CLI daemon requests one on every connect (daemon.rs:161, the init NotebookGet). The host socket is upgraded with this cap (ws.rs:90). If the reply is over 4 MiB, the host read loop gets `Ok(Some(Err(_)))` and breaks, which unregisters the host and ends every one of its sessions. Local notebooks have no size cap, and a notebook right at MAX_SHARE_BYTES (also 4 MiB) already goes over once the envelope is added. main.rs:41 derives the HTTP body cap from MAX_SHARE_BYTES with 2x headroom for exactly this reason; the WS cap is a separate literal that happens to have the same value.

**Fix:** ws.rs: `const MAX_WS_MESSAGE_BYTES: usize = 2 * ironpad_app::server_fns::MAX_SHARE_BYTES;`. Rewrite the comment: the largest legitimate frame is the host's `Response::Notebook` reply to NotebookGet (whole notebook plus envelope), so the cap is derived from the per-share cap with the same 2x headroom main.rs uses for request bodies, and any shareable notebook fits in one frame; exceeding it tears down the host connection. The lib already depends on ironpad_app (oembed/og use it), so this adds no new edge.

**Tests:** No existing test exercises frame size. Add to tests/relay.rs `host_can_reply_with_a_notebook_larger_than_four_mib`: connect a host plus a read-permission guest; the guest sends Query::NotebookGet; the host replies with a Response::Notebook whose single cell source is about 5 MiB; assert the guest receives it (recv_text with a larger timeout) and that the host is still connected afterwards (e.g. a follow-up CreateSession gets SessionCreated). Before the fix this closes the host.

### server-fns-3: Session renewal UPDATE is not conflict-retried; concurrent requests at the 12h boundary resolve as anonymous

*bug · confirmed · value 4 · effort S*

**Where:** `crates/ironpad-app/src/db.rs:533` (also `crates/ironpad-app/src/db.rs:524-528`, `crates/ironpad-app/src/db.rs:147-182`, `crates/ironpad-app/src/auth.rs:62-73`)

**Problem:** session_user's sliding-renewal UPDATE is the one session write not wrapped in with_conflict_retry, and its error propagates via `?`. A single page load runs several server fns concurrently with the same cookie: SSR resources such as get_auth_info plus the page's notebook fetch, and the owner fetches after hydrate. When the session crosses SESSION_RENEW_AFTER_SECS, every one of them sees `renewed = true` and UPDATEs the same session record. SurrealKV commits one and fails the rest with the retryable conflict that with_conflict_retry's doc describes (measured at 7 of 8 failing for save_draft, which has the same statement shape). current_user maps Err to None, so the losing requests treat a valid signed-in user as anonymous. The header renders signed out, the owner editor swap fails with 'sign in with GitHub…', or a private share renders the denial page, once per session every 12h.

**Fix:** In db.rs session_user, replace the bare renewal block with `if renewed { let renewal = with_conflict_retry(|| async { self.inner.query("UPDATE type::record('session', $key) SET expires_at = $exp").bind(("key", key.clone())).bind(("exp", now + SESSION_TTL_SECS)).await.context("session renewal failed")?.check().context("session renewal returned an error")?; Ok(()) }).await; if let Err(e) = renewal { tracing::warn!(error = %e, "session renewal failed; session still valid"); renewed = false; } }`. Make `renewed` a `let mut`, and when renewal fails report renewed=false so the cookie is not re-issued for a row that did not slide. Return Some((user, renewed)) either way.

**Tests:** The existing session_renewal_slides_and_reports_exactly_when_it_happens (db.rs:1480) guards the single-caller semantics. Add `#[tokio::test(flavor = "multi_thread", worker_threads = 8)] async fn concurrent_lookups_at_the_renewal_boundary_all_resolve_the_user()`. It should age a session past SESSION_RENEW_AFTER_SECS the same way the existing renewal test does, spawn 8 concurrent session_user calls, and assert all 8 return Ok(Some(_)). Also assert that at least one reported renewed=true and that expires_at slid. As with concurrent_writers_to_one_record_all_succeed, keep the multi_thread flavor: a current-thread runtime will not reproduce the race.

### js-1: Worker host-message override re-resolves memory and decodes each message twice, the first time without the core's shared-memory-safe copy

*dry · confirmed · value 4 · effort S*

**Where:** `public/executor-worker.js:155` (also `public/executor-worker.js:158-163`, `public/executor-core.js:155-185`)

**Problem:** The worker wraps `_dispatchHostMessage` with its own copy of the bindgen/raw memory ternary (158-160). It decodes `new Uint8Array(memory.buffer, ptr, len)` directly, posts the text, then calls the original, which resolves memory again, `.slice()`s, decodes again and JSON.parses. Host messages carry sim_emit (per simulation tick) and progress updates, so this is per-message work done twice. The two copies also disagree: the core copies out with `.slice()` before decoding, but the worker decodes a live view. For rayon cells the memory is a SharedArrayBuffer, and on an engine whose TextDecoder rejects shared views (browserpod-runtime.js:43-48 documents a close relative of this restriction) that throw happens inside a WASM import. The import traps and the cell silently falls back to the main thread.

**Fix:** In executor-core.js, split _dispatchHostMessage into `CellExecutor.prototype._readHostText = function (cellId, ptr, len)` and `CellExecutor.prototype._handleHostMessage = function (cellId, text)`. The first resolves memory (via the js-2 helper), slices and decodes once, and returns the string or null. The second does the existing JSON.parse, the gpu_read_pixels deferral and handler dispatch, inside its try/catch. `_dispatchHostMessage` becomes `var text = this._readHostText(cellId, ptr, len); if (text !== null) this._handleHostMessage(cellId, text);`. In executor-worker.js, replace the override with `var origHandle = executor._handleHostMessage.bind(executor); executor._handleHostMessage = function (cellId, text) { self.postMessage({ type: "hostMessage", cellId: cellId, messageJson: text }); origHandle(cellId, text); };`. That keeps post-before-local-dispatch ordering, gpu_read_pixels included.

**Tests:** There is no JS unit coverage of the core yet. Add tests/js/executor-host-message.test.mjs, run by `cargo make test-js` (node --test over tests/js/*.test.mjs), following browserpod-runtime.test.mjs: load executor-core.js in a vm with a `self` stub, register a fake raw entry whose memory is `new WebAssembly.Memory({initial:1, maximum:1, shared:true})`, write JSON into it, and assert that _dispatchHostMessage calls a registered handler once with the parsed message. Also add an e2e in execution.spec.ts where a rayon cell emits a host message (e.g. progress) and the result carries no `fallback` flag. The e2e is the check that can catch the SAB decode throw.

### server-fns-4: Linux-cell share snapshots hash the sharer's type tags, which no Linux compile ever uses, so they can miss

*bug · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/server_fns.rs:1103` (also `crates/ironpad-app/src/components/linux_cell.rs:464`, `crates/ironpad-common/src/cache_key.rs:180`, `crates/ironpad-common/src/cell_deps.rs:131-135`, `crates/ironpad-app/src/compiler/scaffold.rs:73-82`)

**Problem:** The Linux scaffold ignores previous_cell_types (scaffold.rs:73 returns before they are read). The only thing that warms a Linux blob is the viewer, which compiles with `previous_cell_types: Vec::new()` (linux_cell.rs:464), because the editor never compiles Linux cells. Since PRD-0067, write_cell_blobs_capped hashes Linux cells too, but it uses the sharer's tag chain `&cell_type_tags[..idx]`. The recipe keeps the tags for every slot the source references, and a bare `last` identifier (ordinary Rust in a Linux cell) counts as depends-on-all. So a Linux cell that mentions `last` or `cellN` and sits below a Code cell that has been run gets a snapshot key that no compile ever produced. The cache lookup misses, the cell is left out of the manifest, and every reader's Run on /shared or /mutable spawns a real cargo build. PRD-0067 wired the snapshot to prevent exactly that. The shipped linux-cells notebook does not trigger it: it has no Code cells and no matching tokens in its Linux cells.

**Fix:** In ironpad-common/src/cache_key.rs content_hash_with_fingerprint, replace the normalize line with `let previous_types = if target.is_linux() { Vec::new() } else { crate::cell_deps::normalize_previous_types(source, previous_types) };` and add a comment: 'A Linux cell has no typed piping (the scaffold never reads the slots), so no upstream tag can change what is built.' Keys the viewer and server already compute for Linux cells hash [] and do not change, so no CACHE_EPOCH bump is needed. If `is_linux` is not reachable on CellTarget in common, compare with `CellTarget::Linux`.

**Tests:** Guarded by share_snapshots_cover_linux_cells_under_the_linux_key (server_fns.rs:2260) and the cache_key.rs normalization tests. Add a cache_key.rs unit test asserting that `content_hash_with_fingerprint("let last = 1;", "", &["i32".into()], None, None, false, false, false, CellTarget::Linux, fp)` equals the same call with `&[]`, and that the Executor target still differs between the two (a control). Extend the server_fns test with a notebook of [Code cell, Linux cell whose source is `fn main() { let last = 1; println!("{last}"); }`] and tags ["i32", ""]. Seed the blob under the viewer's []-tag Linux key and assert the snapshot picks it up.

### server-7: OAuth redirect_uri is built by string concatenation, skipping the trailing-slash-tolerant absolute_url

*bug · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-server/src/auth.rs:92` (also `crates/ironpad-common/src/config.rs:70`, `crates/ironpad-server/src/config.rs:116`, `crates/ironpad-server/src/main.rs:147`)

**Problem:** `format!("{}/auth/callback", state.public_url)` uses the raw `--public-url` value, which is never normalized (config.rs:116-118). `ironpad_common::absolute_url` exists because an operator typo like `https://host/` would otherwise produce `//path`. Its doc says both spellings must round to the same bytes. With a trailing slash, redirect_uri becomes `https://host//auth/callback`. That is not a subdirectory of the registered callback URL, so GitHub rejects every sign-in. OG, sitemap and oEmbed use absolute_url and are unaffected.

**Fix:** auth.rs:92: replace with `&ironpad_common::absolute_url(&state.public_url, "/auth/callback")`. Optionally update the config.rs:44-47 doc to list the OAuth redirect_uri and oEmbed as consumers.

**Tests:** Existing auth tests (`github_redirect_requires_configuration`, 459) use a slash-free public_url. Add `github_redirect_uri_tolerates_a_trailing_slash_on_public_url`: build AuthState with github configured and public_url "http://localhost:3111/", GET /github, parse the Location header with reqwest::Url, and assert the `redirect_uri` query pair == "http://localhost:3111/auth/callback".

### cp-8: Embed rayon banner uses a raw substring check instead of the compiler's one rayon-detection recipe

*bug · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/components/view_only_notebook.rs:113` (also `crates/ironpad-common/src/cache_key.rs:386`)

**Problem:** The threaded-cells banner decides whether a notebook needs atomics with `cargo_toml.contains("rayon")` over the shared manifest and every cell. The build decides with `ironpad_common::cache_key::merged_deps_contain_rayon`, which matches dependency entry names. The two disagree. A notebook with `ndarray = { features = ["rayon"] }`, a comment mentioning rayon, or a Linux cell (native threads, never atomics) gets told in an embed that its cells "can't run inside an embed", even though the compiler builds them without atomics and they run fine. This is a single-source-of-truth violation that surfaces as wrong user-facing copy.

**Fix:** Extract an ungated pure fn in view_only_notebook.rs: `fn notebook_needs_atomics(nb: &IronpadNotebook) -> bool { nb.cells.iter().filter(|c| !c.shared && c.cell_type == CellType::Code).any(|c| ironpad_common::cache_key::merged_deps_contain_rayon(nb.shared_cargo_toml.as_deref(), c.cargo_toml.as_deref().unwrap_or(""))) }`. Call it at :113 in place of `mentions_rayon`. The hydrate block keeps only the `crossOriginIsolated` probe.

**Tests:** Add a `#[cfg(test)] mod tests` in view_only_notebook.rs covering `notebook_needs_atomics`: `rayon = "1"` in a code cell gives true; a shared rayon dep with one code cell gives true; `ndarray = { features = ["rayon"] }` gives false; a rayon comment gives false; a Linux cell with rayon gives false; a notebook with no code cells gives false. The existing embed.spec.ts threaded-banner check, if one exists, still passes on a real rayon notebook.

### editor-3: History Restore and Export HTML bypass the one flush-before-serialize helper; Restore never flushes

*bug · partial · value 3 · effort S*

**Where:** `crates/ironpad-app/src/pages/notebook_editor/history.rs:191` (also `crates/ironpad-app/src/pages/notebook_editor/mod.rs:855-877`, `crates/ironpad-app/src/pages/notebook_editor/mod.rs:1175-1180`, `crates/ironpad-app/src/pages/notebook_editor/mod.rs:359-374`, `crates/ironpad-app/src/pages/notebook_editor/sharing.rs:22-27`)

**Problem:** The history.rs:188-190 comment says the code will 'flush in-progress editor content and force a pre-restore snapshot'. `persist_notebook_durable` never bumps `save_generation`, though: it persists the model, which lags live Monaco text by the 1s per-cell debounce. The hard reload then kills the pending debounce timer. Typing inside that window is missing from the pre-restore snapshot and is lost, which breaks the 'undoable' promise. This is the same failure class CLAUDE.md records for the old Unpublish flow. Export HTML (mod.rs:855-877) hand-rolls the bump+yield+`try_get_untracked` that `sharing::flush_and_read_notebook` already provides. The Ctrl+S watcher (mod.rs:359-374) bumps and persists with no yield, which relies on microtask ordering that the mod.rs:50-57 header says callers must not assume.

**Verifier's correction:** The core claim holds. history.rs:188-191 comments "flush in-progress editor content" but only calls `persist_notebook_durable`, which (state.rs:461-489) never bumps save_generation, and the reload kills the pending 1s debounce. The loss window is small in practice: the user has to open the menu, the History panel and a confirm within 1s of the last keystroke. Severity is lower than stated, but the comment is false and the fix is trivial. Export HTML (mod.rs:851-876) does duplicate `flush_and_read_notebook` (sharing.rs:22-27) by hand. The Ctrl+S watcher (mod.rs:359-374) bumps and then calls `persist_notebook` with no yield. In Local mode the cell's debounce still persists later, so this is ordering fragility rather than data loss.

**Fix:** Move `flush_and_read_notebook` out of sharing.rs into state.rs as `impl NotebookState { pub(super) async fn flush_cells(&self) -> Option<()> { self.save_generation.try_update(|g| *g += 1)?; yield_for_cell_flush().await; Some(()) } pub(super) async fn flush_and_read(&self) -> Option<IronpadNotebook> { self.flush_cells().await?; self.notebook.try_get_untracked().flatten() } }`. Move `CELL_FLUSH_YIELD_MS` and `yield_for_cell_flush` with them. Call sites: history.rs:191, add `state.flush_cells().await;` before `persist_notebook_durable`. sharing.rs: replace the private helper with `state.flush_and_read()`. Export HTML: move into sharing.rs as `export_html_current_notebook(state)` using `flush_and_read` (shared with editor-10). Preview (mod.rs:1175-1180): `spawn_local(async move { state.flush_cells().await; state.flush_cells().await; let _ = state.is_view_mode.try_set(true); })`. Ctrl+S watcher: keep the title meta-apply synchronous, then `spawn_local(async move { state.flush_cells().await; persist_notebook(&state); })`.

**Tests:** Existing: local-history.spec.ts (Restore), import-export.spec.ts (Export/Download), keyboard.spec.ts (Ctrl+S). Add to local-history.spec.ts: type into a cell, then immediately (<1s, no wait) trigger Restore through the panel. After the reload, restore the pre-restore snapshot and assert the typed text is present.

### cli-1: CLI hand-builds IPC JSON that the daemon hand-parses back into protocol types; the cell-type mapping has already drifted

*dry · partial · value 3 · effort S*

**Where:** `crates/ironpad-cli/src/daemon.rs:828` (also `crates/ironpad-cli/src/main.rs:68-81`, `crates/ironpad-cli/src/main.rs:400-461`, `crates/ironpad-cli/src/main.rs:487-511`, `crates/ironpad-cli/src/main.rs:540-584`, `crates/ironpad-cli/src/main.rs:586-601`, `crates/ironpad-cli/src/daemon.rs:837-840`, `crates/ironpad-cli/src/daemon.rs:941-953`, `crates/ironpad-common/src/types.rs:198-206`)

**Problem:** Each mutation is written three times: the CLI turns clap args into ad-hoc JSON keys ("type", "after_cell_id", ...), and translate_command (~150 lines) plucks them back out field by field into NewCell / Mutation::CellUpdate / CellDelete. The daemon's own notebook.update arm already says the right approach is to deserialize the protocol type verbatim "rather than plucking fields" so the CLI stays in lockstep, but only that arm does it. Even there the CLI builds the patch with string keys, so a renamed NotebookMetaPatch field would be silently ignored because it has no deny_unknown_fields. The drift is already real. CellTypeArg is Code|Markdown only, so the CLI cannot create a CellType::Linux cell. The daemon maps any other `type` string to Code (`_ => CellType::Code`), so a raw `"type":"linux"` quietly becomes a Code cell, which is the silent Linux-as-Code mistake that types.rs:182-188 calls out as dangerous.

**Verifier's correction:** The drift is real. main.rs:68-81 CellTypeArg is Code|Markdown only, so the CLI cannot create a Linux cell. daemon.rs:837-840 maps any `type` other than "markdown" to Code, so the hidden `raw` command or a typo silently makes a Code cell, which is the Linux-as-Code confusion types.rs:182-188 warns about. The CLI also builds NotebookMetaPatch with string keys (main.rs:408-461). The proposed rewrite is scoped too wide: it changes the CLI-daemon IPC contract (the hidden raw command, and daemons left running across an upgrade), and the plucking in translate_command is ordinary mapping, not a live bug. The cell-type hole and the stringly patch are the parts worth fixing now.

**Fix:** (1) main.rs: add `Linux` to CellTypeArg and have as_str return "linux" for it. (2) daemon.rs translate_command "cells.add": replace the `_ => Code` arm with `None | Some("code") => CellType::Code, Some("markdown") => CellType::Markdown, Some("linux") => CellType::Linux, Some(other) => return Err(format!("unknown cell type: {other} (expected code, markdown or linux)"))`. (3) main.rs handle_notebook_update: build a typed `ironpad_common::protocol::NotebookMetaPatch { title, shared_source: Some(None)/Some(Some(s)), ... , ..Default::default() }` and send `json!({"meta": serde_json::to_value(&patch)?})` in place of the serde_json::Map, so a renamed field is a compile error. Skip_serializing_if already keeps the tri-state wire shape. Sending whole Mutation values over IPC can be a later follow-up, not part of this fix.

**Tests:** The translate_cells_add_defaults, translate_cells_add_full and translate_notebook_update_tri_states tests in daemon.rs guard the existing mapping. Add translate_cells_add_linux ("type":"linux" yields CellType::Linux) and translate_cells_add_unknown_type_is_an_error. Add a clap parse test that `cells add --type linux` parses. Add a unit test that the typed patch serializes --clear-description as an explicit null and leaves omitted fields absent, i.e. the same JSON translate_notebook_update_tri_states consumes.

### config-1: `cargo make warmup-atomics` cannot run, and when fixed it would warm the wrong toolchain with a third hand copy of the atomics flags

*bug · confirmed · value 3 · effort S*

**Where:** `Makefile.toml:224` (also `Makefile.toml:231-239`, `docker/Dockerfile:189-192`, `docker/Dockerfile:198-207`, `crates/ironpad-app/src/compiler/build.rs:78`, `crates/ironpad-app/src/compiler/build.rs:85-92`, `crates/ironpad-app/src/compiler/build.rs:1050-1118`)

**Problem:** (1) `cp -r crates/ironpad-cell "$TMPDIR/crates/ironpad-cell"` copies into a fresh `mktemp -d` that has no `crates/` directory. cp does not create parent directories, so under `set -euo pipefail` the task aborts on that line. (2) Even with that fixed, it builds with `cargo +nightly`, a floating channel, instead of CELL_TOOLCHAIN (nightly-2026-05-19). The Dockerfile's own comment (189-192) says a warmup on any other nightly produces artifacts the runtime fingerprints as stale and rebuilds from scratch. (3) Its RUSTFLAGS are a third literal copy of ATOMICS_TARGET_FEATURES + ATOMICS_LINK_RUSTFLAGS (the other copy is Dockerfile:198-205). The toolchain sync test only scans dated `nightly-20…` literals, so neither the floating `+nightly` nor drift in the flags is caught.

**Fix:** In Makefile.toml [tasks.warmup-atomics]: add `mkdir -p "$TMPDIR/crates"` before the cp, add `cp rust-toolchain.toml "$TMPDIR/"` so rustup resolves CELL_TOOLCHAIN inside the temp dir (its channel is already asserted equal to CELL_TOOLCHAIN), and change `cargo +nightly build` to `cargo build`. Add a sibling test in build.rs tests, `atomics_warmup_flags_match_the_runtime`: read docker/Dockerfile and Makefile.toml, and assert each contains `target-feature={ATOMICS_TARGET_FEATURES}` and every whitespace-split token of ATOMICS_LINK_RUSTFLAGS. Also assert that Makefile.toml has no `cargo +nightly ` floating channel. If Aaron would rather not keep a local warmup, deleting the task and its CLAUDE.md table row is an acceptable alternative.

**Tests:** There is no current coverage of the task, and the existing toolchain_pins_are_in_sync_across_dockerfile_ci_and_toolchain_toml does not read Makefile.toml. Add the atomics_warmup_flags_match_the_runtime test described above, then run `cargo make warmup-atomics` once by hand to confirm the script now completes.

### cp-9: LayoutContext page reset is ad hoc, so the footer shows a stale 'Saved: Xm ago' on read-only pages

*bug · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-app/src/pages/home_page.rs:215` (also `crates/ironpad-app/src/pages/public_notebook.rs:27`, `crates/ironpad-app/src/pages/public_notebook.rs:33`, `crates/ironpad-app/src/pages/shared_notebook.rs:26`, `crates/ironpad-app/src/pages/shared_notebook.rs:40`, `crates/ironpad-app/src/pages/mutable_notebook.rs:106`, `crates/ironpad-app/src/pages/mutable_notebook.rs:134`, `crates/ironpad-app/src/pages/admin.rs:67`, `crates/ironpad-app/src/pages/notebook_editor/mod.rs:376`, +1 more)

**Problem:** Each route resets its own subset of `LayoutContext`. Home clears title, cell_count and last_save_time. Public, shared and mutable clear only the title and set cell_count once their resource resolves. Admin clears only the title, and the embeds clear nothing. The editor sets `last_save_time` (notebook_editor/mod.rs:376) and nothing clears it on unmount. So after Fork on /public, an edit, and browser Back (client-side navigation), the read-only public page's status bar still reads "Saved: 1m ago". /admin likewise keeps the previous page's "Cells: N".

**Fix:** Add a method to `LayoutContext` in app_layout.rs: `pub fn enter_page(&self) { self.notebook_title.set(None); self.cell_count.set(0); self.last_save_time.set(None); }`. Call it as the first statement after `expect_context::<LayoutContext>()` in HomePage, PublicNotebookPage, SharedNotebookPage, MutableNotebookPage, the admin page and NotebookEditorPage, replacing the scattered `.set(None)` lines. The editor's later Effects that set title and count (mod.rs:237-238) still run afterwards. Do NOT reset from an AppLayout pathname Effect: it would race with the pages' own synchronous sets.

**Tests:** Add an e2e in routes.spec.ts: create a /local notebook, edit a cell and wait for `Saved:` in `.ironpad-status-bar`, client-navigate to a /public notebook through an in-app link, then assert the status bar contains no `Saved:`. The existing home.spec.ts/admin.spec.ts cover the pages still rendering.

### cell-sim-1: Per-frame host messages build a serde_json::Value tree, and json! panics where host_message_json deliberately swallows

*performance · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-cell/src/sim.rs:32` (also `crates/ironpad-cell/src/ui.rs:688`, `crates/ironpad-cell/src/gpu.rs:215`, `crates/ironpad-cell/src/gpu.rs:241`, `crates/ironpad-cell/src/lib.rs:49`)

**Problem:** `sim::emit` is called several times per tick in about ten public notebooks (double-pendulum, lorenz-attractor, game-of-life, ...). Each call goes through `json!`, which builds a Map, allocates three key Strings plus a clone of `key`, converts `value` with `to_value` into a full intermediate tree, and only then serializes to a String. That happens every frame at 30-60 fps. `ProgressHandle::update` (ui.rs:688) does the same inside user loops, and so do the gpu readback and warning messages. There is also a correctness gap. `json!` interpolation expands to `to_value(&v).unwrap()`, so a value whose JSON serialization fails panics and traps the cell (for example a `HashMap<(i32, i32), f64>`, which fails with "key must be a string"). `host_message_json` (lib.rs:49-53) is explicitly written to drop such values silently.

**Fix:** sim.rs: add `#[derive(serde::Serialize)] struct SimEmit<'a, T: ?Sized + serde::Serialize> { #[serde(rename = "type")] kind: &'static str, key: &'a str, value: &'a T }`, and have emit call `crate::host_message_json(&SimEmit { kind: "sim_emit", key, value })`. Relax emit's bound to `T: serde::Serialize + ?Sized` only if that is free; otherwise keep it. ui.rs: `struct ProgressUpdate<'a> { #[serde(rename = "type")] kind: &'static str, id: &'a str, value: f64 }` in ProgressHandle::update. The gpu.rs readback and warning messages can take the same treatment for consistency, but they are cold and cannot fail, so they are optional. Update emit's doc to say a value that fails to serialize is dropped rather than panicking. Put the CACHE_EPOCH bump in the same one bump as cell-gpu-1 and cell-enzyme-1, since this changes runtime behavior (panic becomes drop) and the cell runtime is outside the hash. Do not bump separately.

**Tests:** Existing sim.rs tests cover read_from_ffi only. Add (1) `emit_drops_unserializable_values_instead_of_panicking`, which calls `sim::emit("k", &HashMap::from([((1, 2), 3.0)]))` natively; it panics today and passes after the fix. (2) `sim_emit_message_shape_is_unchanged`, which asserts `serde_json::to_value(SimEmit{..}) == json!({"type":"sim_emit","key":"k","value":[1,2]})`. Add the equivalent shape test for ProgressUpdate.

### cell-gpu-1: GpuSimulation::tick takes the GPU path on a tick ABI that never reads back or frees GPU buffers

*bug · partial · value 2 · effort S*

**Where:** `crates/ironpad-cell/src/gpu.rs:321` (also `crates/ironpad-cell/src/gpu.rs:262`, `crates/ironpad-cell/src/gpu.rs:215`, `public/executor-core.js:544`, `public/executor-core.js:172`, `public/executor-core.js:885`, `public/executor-core.js:288`, `public/executor-core.js:335`)

**Problem:** `execute()` calls `Gpu.init()` on every run (executor-core.js:544), so `gpu_available()` returns 1 during later ticks on any WebGPU browser. The default `GpuSimulation::tick` then calls `GpuCanvas::render()` every frame. That allocates three GPU buffers, compiles a shader, dispatches, posts `gpu_read_pixels`, and returns a gray placeholder canvas. The tick path (`_tickBindgen`/`_tickRaw` -> `_readTickResult`, 885-905) never drains `_pendingGpuReadbacks` and never calls `Gpu.cleanupAllHandles()`; only `execute` does both (634-641). Result: every frame renders gray, buffers of size w*h*16 leak per frame until the next execute, and readbacks pile up. The next execute on that executor then runs `_processGpuReadbacks` over the stale tick readbacks, and those that match no placeholder are appended as extra BlobImage panels (335) on whichever cell ran next. The module docs (262-273) recommend this adapter pattern as working "today". No public notebook uses GpuSimulation, which is why this has gone unnoticed.

**Verifier's correction:** The bug is real. execute() calls Gpu.init() (executor-core.js:544), so availableSync is true during later ticks. The default GpuSimulation::tick (gpu.rs:321-328) then calls GpuCanvas::render, which pushes a gpu_read_pixels message (_dispatchHostMessage, executor-core.js:172-175) and returns a gray canvas. tick/_tickBindgen/_tickRaw/_readTickResult (797-905) never drain _pendingGpuReadbacks or call cleanupAllHandles; only execute's finally blocks do (638-641, 739-742). It is actually worse than the finding says. The scaffold's cell_main also calls sim.tick() for the first frame (scaffold.rs:763), and that first frame lands in a Simulation panel, which _processGpuReadbacks never matches because it only looks for BlobImage (313-321). So even the first frame is gray, and a stray BlobImage panel gets appended. The proposed JS fix is unsafe as written. cleanupAllHandles is global, so a per-tick cleanup can free the buffers of an execute that is interleaved at its `await Gpu.readPixels`. The file already notes that interleaved executes share this state (709). A tick readback pushed during an in-flight execute would also be consumed by that execute. Nothing uses the path: no public notebook and no scaffold route reference GpuSimulation (grep), and it is reachable only through the doc's adapter pattern. That makes it low value and argues for the honest stop-gap rather than new tick-ABI plumbing.

**Fix:** Take the stop-gap and treat the JS plumbing as its own design task. In gpu.rs, change the default `GpuSimulation::tick` to always call `self.tick_cpu()`, and add a comment saying the tick ABI has no readback channel: a per-tick dispatch would return the gray placeholder and leak the three buffers until the next execute. Rewrite the trait doc (gpu.rs:251-273) and the module doc (7-9) to say that GPU dispatch is currently available only through GpuCanvas in a regular cell, and that GpuSimulation ticks on the CPU. Bump CACHE_EPOCH to 12 with a doc paragraph in cache_key.rs. This is a runtime behavior change and the cell runtime is not part of the hash (see the 10->11 note). Ship it in the same bump as the other runtime behavior changes (cell-sim-1, cell-enzyme-1). If real per-tick GPU is wanted later, it needs: per-dispatch handle ownership in executor-gpu.js (so cleanup frees only the tick's own handles instead of calling cleanupAllHandles), a tick-local readback queue kept separate from execute's, and _processGpuReadbacks filling `Simulation.first_frame_data` with raw-RGB base64.

**Tests:** Nothing covers this today. There is no JS test for executor-core GPU paths (tests/js holds only browserpod), and headless Playwright lacks WebGPU. Add a native unit test in gpu.rs: a GpuSimulation impl whose tick_cpu increments a counter and returns a distinctive canvas, asserting the default tick() returns it. This pins the delegation, though natively gpu_available() is already false, so the test documents the change more than it guards it. Say so in the test comment.

### cell-enzyme-1: Enzyme malloc/realloc shims can wrap the size and hand back an undersized allocation

*bug · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-cell/src/enzyme_shims.rs:27` (also `crates/ironpad-cell/src/enzyme_shims.rs:76`, `crates/ironpad-cell/src/enzyme_shims.rs:54`)

**Problem:** `malloc` computes `size + SHIM_HEADER` unchecked and passes it to `Layout::from_size_align_unchecked`. In release builds (which cells are), a size above `usize::MAX - 16` wraps to a tiny layout, the header write lands inside it, and the returned `base + 16` points past the allocation. `calloc` uses `saturating_mul` precisely so an overflowing count forces a failure, but it then calls this `malloc`, which returns a small block that `write_bytes(ptr, 0, usize::MAX)` overruns. `realloc` also truncates its u64 `new_size` with `as usize`, so a size over 4 GiB becomes a small allocation. Enzyme will almost never request such sizes, but this is UB inside unsafe code.

**Fix:** enzyme_shims.rs: add `fn shim_layout(size: usize) -> Option<Layout> { Layout::from_size_align(size.checked_add(SHIM_HEADER)?, SHIM_HEADER).ok() }`. In malloc, `let Some(layout) = shim_layout(size) else { return core::ptr::null_mut() };`. free keeps from_size_align_unchecked, with a comment that the size was validated by shim_layout when it was allocated. In realloc, `let Ok(new_size) = usize::try_from(new_size) else { return core::ptr::null_mut() };`. calloc then fails naturally through malloc returning null. To make shim_layout testable natively, move it (and SHIM_HEADER) into a tiny ungated `pub(crate) mod shim_layout` or into lib.rs with `#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]`. On CACHE_EPOCH: this is practically unobservable. Fold it into the one bump planned for cell-sim-1/cell-gpu-1 rather than bumping separately.

**Tests:** Add native tests on shim_layout: `shim_layout(usize::MAX)` and `shim_layout(usize::MAX - 8)` are None, `shim_layout(isize::MAX as usize)` is None, and `shim_layout(0)`/`shim_layout(100)` are Some with size+16. The end-to-end guard is the existing autodiff integration test (compiler e2e, `cargo make test-integration`), which must still pass.

## Wave 2: single source of truth

Rules restated at several sites instead of derived from one. Several have already drifted (the CLI cannot create a Linux cell, the wasm-bindgen pin comment says 0.2.114, the path-segment predicate disagrees on the empty string). compiler-1 wants a golden-key test committed BEFORE the refactor, so the no-`CACHE_EPOCH`-bump claim is proven rather than asserted.

### compiler-1: Cache-key feature flags are re-derived by hand at every call site and passed as three loose bools

*dry · partial · value 4 · effort M*

**Where:** `crates/ironpad-common/src/cache_key.rs:157` (also `crates/ironpad-app/src/compiler/cache.rs:32`, `crates/ironpad-app/src/server_fns.rs:182`, `crates/ironpad-app/src/server_fns.rs:503`, `crates/ironpad-app/src/server_fns.rs:1097`, `crates/ironpad-app/src/blob_cache.rs:104`, `crates/ironpad-app/src/compiler/scaffold.rs:92`, `crates/ironpad-app/src/compiler/scaffold.rs:114`, `crates/ironpad-app/src/compiler/mod.rs:1944`, +7 more)

**Problem:** `content_hash_with_fingerprint` takes `needs_atomics/needs_autodiff/needs_simd` as parameters, yet all three are pure functions of inputs it already receives (source, cargo_toml, shared_cargo_toml, shared_source). As a result, the same three-line detection block (`merged_deps_contain_rayon` + `uses_std_autodiff` + `uses_wasm_simd`) is copied into compile_cell_core, check_cell_core, the share snapshot, the browser's `request_hash` and the notebook gate test. The scaffold re-derives it a sixth time. The module doc calls this 'one recipe instead of two drifting reimplementations', but the recipe's inputs are still assembled by hand at every site. A caller can hash with flags that disagree with the source, and the test helpers do exactly that: `seed_cache` and `key_for` hard-code `false,false,false`, which would silently mis-key any fixture that used rayon. The same bool triple then threads through `effective_features`, `build_micro_crate`, `check_micro_crate`, `configure_cargo_cmd` and `compose_rustflags`, each carrying `#[allow(too_many_arguments/fn_params_excessive_bools)]`, and roughly 50 `content_hash(...)` calls spell out `false, false, false`.

**Verifier's correction:** The duplication is real. The same three-line detection block appears at server_fns.rs:182-185 (compile), 503-506 (check), 1097-1099 (share snapshot), blob_cache.rs:104-110 (browser key), and compiler/mod.rs:1944-1950 (gate test). scaffold.rs:92-93 and 113 derive it again. The bool triple goes loose through content_hash_with_fingerprint (cache_key.rs:157-168), cache::content_hash (cache.rs:32-42), effective_features (build.rs:167), build_micro_crate (242), configure_cargo_cmd (441), compose_rustflags (496) and check_micro_crate (544). seed_cache (server_fns.rs:3085-3095) and key_for (2288-2299) hard-code false,false,false. Three parts of the proposed fix need correcting. (a) The hash must detect the RAW flags. Applying `for_target` first would change every Linux key, and today's Linux keys hash unmasked detection (see the note at build.rs:163-165). (b) Moving detection inside the public fn removes the only way to test that each flag byte is hashed. The hash_changes_with_needs_{atomics,autodiff,simd} tests (cache.rs:734/763/790) need a private seam, not only 'sources that trigger detection'. A source change already changes the key, so that swap would prove nothing about the flag bytes. (c) 'Byte-identical keys, no CACHE_EPOCH bump' is asserted, but nothing verifies it today. No golden-key test exists.

**Fix:** 1. In crates/ironpad-common/src/cache_key.rs, add `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub struct CellFeatures { pub atomics: bool, pub autodiff: bool, pub simd: bool }` with `#[must_use] pub fn detect(source: &str, cargo_toml: &str, shared_cargo_toml: Option<&str>, shared_source: Option<&str>) -> Self`, which calls merged_deps_contain_rayon, uses_std_autodiff and uses_wasm_simd, and `#[must_use] pub const fn for_target(self, target: CellTarget) -> Self`, which returns Self::default() for Linux. The latter replaces build.rs `effective_features`. 2. Split the hash. Add a private `fn content_hash_with_features(source, cargo_toml, previous_types, shared_cargo_toml, shared_source, features: CellFeatures, target, toolchain) -> String` holding today's body, with the three `hasher.update(&[u8::from(..)])` lines in the same order. The pub `content_hash_with_fingerprint` drops its three bool params and calls it with `CellFeatures::detect(..)` (RAW, never for_target). Drop the `fn_params_excessive_bools` allow. 3. Drop the bools from `cache::content_hash` (cache.rs:32) too. 4. build.rs: `build_micro_crate`, `check_micro_crate`, `configure_cargo_cmd` and `compose_rustflags` take `features: CellFeatures` in place of the triple. The two entry points do `let features = features.for_target(target);` where effective_features was called. Remove the fn_params_excessive_bools allows. 5. Call sites: in server_fns.rs:182-185 and 503-506, use `let features = CellFeatures::detect(&request.source, &request.cargo_toml, request.shared_cargo_toml.as_deref(), request.shared_source.as_deref());`, keeping the tracing fields as `features.atomics` etc. and passing `features.atomics` to optimize_wasm. At server_fns.rs:1097-1099, blob_cache.rs:104-110 and mod.rs:1944-1950, delete the detection lines and the bool args. In scaffold.rs:92-93/113, use `let features = CellFeatures::detect(source, cargo_toml, shared_cargo_toml, shared_source);`. 6. Tests: delete the false,false,false args everywhere. Move the three hash_changes_with_needs_* tests from cache.rs into cache_key.rs tests, calling `content_hash_with_features` with toggled features. Update seed_cache/key_for, which now detect automatically.

**Tests:** Before touching the recipe, ADD a golden test `hash_recipe_is_byte_stable` in cache_key.rs. It pins the literal hex of `content_hash_with_fingerprint` (current signature) for three inputs with fixed fingerprint "tc": a plain cell, a rayon-dep + std::simd cell, and a Linux-target cell mentioning std::simd. Commit it first, then refactor, and it must stay green. That proves no CACHE_EPOCH bump is needed and that Linux keys are unchanged. Existing guards: cache_key.rs hash_* tests, cache.rs hash_* tests, build.rs compose_rustflags tests, server_fns snapshot tests (seed_cache), and `cargo make test-integration` (compile_fearless_simd_cell_reaches_the_simd128_backend, compile_rayon_cell_links_a_shared_memory_module, all_public_notebook_cells_compile).

### server-fns-5: Cache-key feature detection is re-spelled at every caller; only the hash itself is shared

Merged into **compiler-1** (the same finding, seen from the server-fns-and-db partition).

### common-1: Cell-update field set is spelled out four times; apply the NotebookMetaPatch fix to it

*dry · confirmed · value 4 · effort M*

**Where:** `crates/ironpad-common/src/protocol.rs:242` (also `crates/ironpad-common/src/protocol.rs:362`, `crates/ironpad-common/src/notebook_ops.rs:64`, `crates/ironpad-app/src/model.rs:58`, `crates/ironpad-app/src/model.rs:109`, `crates/ironpad-app/src/model.rs:466`, `crates/ironpad-cli/src/daemon.rs:340`, `crates/ironpad-cli/src/daemon.rs:912`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:179`, +5 more)

**Problem:** `Mutation::CellUpdate` (242-269) and `Event::CellUpdated` (362-386) each list the same seven fields (source, cargo_toml with `explicit_null_is_a_clear`, label, shared, collapsed, output_collapsed, version) with the same serde attributes. `notebook_ops::CellPatch<'a>` (64) borrows the same six fields again, and model.rs's `CellUpdateFields` (58) is a fourth copy. Every caller destructures all seven fields and rebuilds them (model.rs:109-126 and 466-474, daemon.rs:340-360), and each of the six cell_item.rs sites writes out six `None`s. The doc on `NotebookMetaPatch` (protocol.rs:107-112) names the risk here: a guest rebuilds its cache from the event, so a field the mutation carries and the event drops is silent data loss. That struct was created to remove exactly this kind of duplication, and the cell-level pair never got the same treatment. The next per-cell attribute has to be added in four structs and about ten construction sites.

**Fix:** 1) protocol.rs: add `#[allow(clippy::option_option)] #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)] pub struct CellPatch { source: Option<String>, cargo_toml: Option<Option<String>> (default, skip_serializing_if, deserialize_with = "explicit_null_is_a_clear"), label: Option<String>, shared: Option<bool>, collapsed: Option<bool>, output_collapsed: Option<bool> }`, each field with `#[serde(default, skip_serializing_if = "Option::is_none")]`, and move the existing field doc comments onto it. 2) Change the variants to `CellUpdate { cell_id: String, #[serde(flatten)] patch: CellPatch, version: u64 }` and `CellUpdated { cell_id, #[serde(flatten)] patch: CellPatch, version }`. 3) notebook_ops.rs: delete the borrowed `CellPatch<'a>` and put `impl CellPatch { pub fn apply_to(&self, cell: &mut IronpadCell, version: u64) }` next to the other ops, with the body unchanged apart from the Option<&T> to &Option<T> adjustment. 4) model.rs: delete CellUpdateFields and change the signature to `cell_update(&self, cell_id: String, patch: CellPatch, version: u64)`. The staling logic reads `patch.source.is_some()` etc. The update_untracked closure calls `patch.apply_to(cell, new_version)`, and the Event is `Event::CellUpdated { cell_id, patch, version: new_version }`. `is_remote_content_edit` (model.rs:596) matches `Mutation::CellUpdate { patch: CellPatch { source: Some(_), .. }, .. } | Mutation::CellUpdate { patch: CellPatch { cargo_toml: Some(_), .. }, .. }`. 5) daemon.rs:340 becomes `Event::CellUpdated { cell_id, patch, version } => if let Some(cell) = ... { patch.apply_to(cell, *version) }`. daemon.rs:912 builds `CellPatch { source, cargo_toml, label, shared, collapsed, output_collapsed }`. 6) The cell_item.rs sites become `patch: CellPatch { label: Some(current), ..Default::default() }` and similar. 7) Update the test constructors at model.rs:616/638/724, daemon.rs:1478 and protocol.rs:831.

**Tests:** Existing guards: protocol.rs `cell_update_explicit_null_cargo_toml_survives_the_wire` (null clear on both the mutation and the event), the round_trip tests, the model.rs is_remote_content_edit tests, the daemon cache tests and notebook_ops tests. Add `cell_update_flatten_left_the_wire_format_alone`, which serializes a CellUpdate and a CellUpdated with only `label` set and asserts payload["label"] is at top level, no `patch` key exists, and untouched fields are absent. This mirrors flattening_the_patch_left_the_wire_format_alone. Also add a round-trip test asserting the Mutation's patch equals the Event's patch after model.apply, which pins that the event echoes every field.

### protocol-1: The seven CellUpdate fields are hand-listed in ~12 places; NotebookMetaPatch already solved this with a flattened struct

Merged into **common-1** (the same finding, seen from the cli-js-tools-config partition).

### editor-9: Browser mutation boilerplate: six hand-built CellUpdate literals and three CellAdd+focus+persist copies

Merged into **common-1** (the same finding, seen from the editor partition).

### cell-contract-1: Contracts between cell runtime and app are hand-mirrored with nothing enforcing them, and two comments are stale

*dry · confirmed · value 4 · effort M*

**Where:** `crates/ironpad-cell/src/lib.rs:246` (also `crates/ironpad-app/src/components/output_render.rs:33`, `crates/ironpad-cell/src/lib.rs:936`, `crates/ironpad-app/src/components/animation_canvas.rs:16`, `crates/ironpad-cell/src/lib.rs:198`, `crates/ironpad-cell/src/lib.rs:200`, `crates/ironpad-app/src/components/executor.rs:299`, `crates/ironpad-app/src/components/executor.rs:301`, `crates/ironpad-cell/src/ui.rs:95`, +1 more)

**Problem:** `DisplayPanel` (lib.rs:246 vs output_render.rs:33) and `SimSliderMeta` (lib.rs:936 vs animation_canvas.rs:16) are defined twice. The encoder for the `CellInputs` wire format also exists twice: `CellInputs::serialize` (lib.rs:200) and `encode_cell_inputs` (executor.rs:301). The widget kind strings and config JSON keys built in ui.rs are parsed by string in output_render.rs:472-481 and the `render_*` fns. The separation is deliberate ("so the UI doesn't depend on the cell runtime"), but the only thing keeping the copies in sync is comments, and two of those are wrong. lib.rs:198 says `serialize` "is used by the frontend" when only tests call it. executor.rs:299 points at an `input.rs` in ironpad-cell that does not exist. A variant or field added on one side decodes as a parse failure on the other at runtime, and no cargo test can catch it.

**Fix:** 1) crates/ironpad-app/Cargo.toml: add `ironpad-cell.workspace = true` under [dev-dependencies]. 2) In output_render.rs, hoist the kind strings the Interactive dispatch matches into `pub(crate) const WIDGET_KINDS: &[&str] = &["slider", "dropdown", "checkbox", "text_input", "number", "switch", "button", "progress"];` next to the match, with a comment that the match and the list must agree. The match arms stay literal. 3) Add `crates/ironpad-app/src/components/cell_contract_tests.rs` (declared `#[cfg(test)] mod cell_contract_tests;` in components/mod.rs) with three tests. (a) `every_cell_panel_decodes_as_the_app_mirror`: build one of each ironpad_cell::DisplayPanel variant (Simulation with a non-empty sliders vec, LiveView, Animation, BlobImage, Table, Interactive, and the four string variants), serde_json round-trip it into crate::components::output_render::DisplayPanel, and re-serialize to assert JSON equality. (b) `encode_cell_inputs_matches_cell_decoder`: `ironpad_cell::CellInputs::from_raw(&encode_cell_inputs(&[b"a".as_slice(), b"", b"xyz"]))`, asserting len and each `get(i).raw()`, plus byte-equality with `CellInputs::serialize`. (c) `every_cell_widget_kind_is_dispatched`: for each ironpad_cell::ui widget constructor, take `IntoPanels::into_panels(&CellOutput::from(w))`, extract the Interactive kind, and assert it is in WIDGET_KINDS. 4) Fix lib.rs:198 to say "The runtime counterpart of ironpad-app's `encode_cell_inputs`; the app's contract test holds them byte-identical", and fix executor.rs:299 to point at `ironpad-cell/src/lib.rs` and the contract test.

**Tests:** These tests are the deliverable. Before trusting them, run each once with a deliberately renamed field or kind to confirm it fails; a contract test that has never failed proves nothing (per CLAUDE.md's method notes). The existing display_panel_deserializes_from_cell_json stays.

### config-2: The wasm-bindgen CLI pin is copied into three build environments with no sync test, and one copy's comment has already drifted

*dry · confirmed · value 4 · effort S*

**Where:** `Makefile.toml:43` (also `.github/workflows/build.yml:52`, `.github/workflows/build.yml:57`, `docker/Dockerfile:38`, `Cargo.lock:6661`, `crates/ironpad-app/src/compiler/build.rs:1050`)

**Problem:** 0.2.127 has to match the locked `wasm-bindgen` crate (Cargo.lock:6661) and is written in Makefile.toml (twice on one line), build.yml and the Dockerfile builder stage. CLAUDE.md records that "all three must move together", but nothing enforces it. The toolchain sync test covers nightly dates only. build.yml:52 already says "(0.2.114)" beside a 0.2.127 install, which is exactly the drift in question. A mismatch fails cargo-leptos's frontend post-processing, and in the image that only surfaces deep into a docker build.

**Fix:** Add `wasm_bindgen_cli_pins_match_the_lockfile` next to the sync test in crates/ironpad-app/src/compiler/build.rs tests. Read ../../Cargo.lock, find the line after `name = "wasm-bindgen"`, and strip `version = "..."`. Assert that docker/Dockerfile and .github/workflows/build.yml contain `wasm-bindgen-cli@{v}` and that Makefile.toml contains `wasm-bindgen-cli --version {v}`. Also assert that every `wasm-bindgen-cli@` occurrence in those files carries {v}, so a second, stale pin fails too. Delete "(0.2.114)" from build.yml:52 so the comment has no number to go stale.

**Tests:** This finding is the new test, wasm_bindgen_cli_pins_match_the_lockfile. Before landing it, bump one copy locally and confirm the test fails, then revert.

### config-3: CI hard-codes the BrowserPod version that browserpod.env declares to be its only home

*dry · partial · value 3 · effort S*

**Where:** `.github/workflows/build.yml:46` (also `docker/browserpod.env:4-8`, `docker/browserpod.env:16`, `docker/Dockerfile:143-154`, `docker/vendor-browserpod.sh:17-20`)

**Problem:** browserpod.env says it is "The ONE place these three strings live… A second copy of the version string is a second thing to forget". build.yml:46 nonetheless writes `browserpod-rust-3.0.0` twice (tarball and install dir), and it repeats the Dockerfile's extract-then-run-install.sh sequence without the sha256 recheck. The sync test does not cover this literal, so a BROWSERPOD_VERSION bump leaves CI installing the old pack. That only shows up later, as test-integration's "toolchain not installed" failure.

**Verifier's correction:** build.yml:46 hard-codes `browserpod-rust-3.0.0` twice, although docker/browserpod.env:1-8 says it is the only home for that string. The sync test (build.rs:1088-1098) checks BROWSERPOD_TOOLCHAIN against the env file but never reads build.yml for that literal, so a version bump would leave CI extracting a tarball that does not exist. The 'without the sha256 recheck' part is wrong: the same step first runs docker/vendor-browserpod.sh, which verifies the digest (verify() at :22-24) before anything lands at the tarball path. browserpod.env:37 also re-literals the version inside BROWSERPOD_DIST_BASE (`.../3.0.0/rust`).

**Fix:** Replace the build.yml step body with: `bash docker/vendor-browserpod.sh` then `. docker/browserpod.env` then `tar -xzf "docker/vendor/browserpod-rust-${BROWSERPOD_VERSION}.tar.gz" -C /tmp` then `sh "/tmp/browserpod-rust-${BROWSERPOD_VERSION}/install.sh"`. The Dockerfile runs install.sh by absolute path from another cwd, so it does not depend on cwd. In docker/browserpod.env, write `BROWSERPOD_DIST_BASE=https://rt.browserpod.io/${BROWSERPOD_VERSION}/rust`, which works because sh sources the file after BROWSERPOD_VERSION is set. Put an `--install` mode on vendor-browserpod.sh only if a third caller appears.

**Tests:** Extend toolchain_pins_are_in_sync_across_dockerfile_ci_and_toolchain_toml to assert that build.yml contains no `browserpod-rust-` followed by a digit (it must go through ${BROWSERPOD_VERSION}) and that browserpod.env's DIST_BASE contains no copy of the literal version. The next CI run's test-integration Linux-cell tests confirm the install still works.

### compiler-2: compile_cell_core and check_cell_core duplicate their whole prologue

*dry · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/server_fns.rs:134` (also `crates/ironpad-app/src/server_fns.rs:453`, `crates/ironpad-app/src/server_fns.rs:154`, `crates/ironpad-app/src/server_fns.rs:472`, `crates/ironpad-app/src/server_fns.rs:159`, `crates/ironpad-app/src/server_fns.rs:474`, `crates/ironpad-app/src/server_fns.rs:241`, `crates/ironpad-app/src/server_fns.rs:508`)

**Problem:** Both cores repeat the same steps. `reject_uncompilable_cell_type` + `CellTarget::from` appear with an identical 9-line comment, copy-pasted, and each copy claims to be 'The one place the request's cell type becomes a compile target'. Both hard-code `session_id = "default"`. Both run `is_valid_cell_id` with an identical error string, the same feature detection, and an identical 10-argument `scaffold_micro_crate(...)` call built from request fields. A new validation (a size cap on source, say) has to be remembered twice, and the 'one place' comment is false because there are two.

**Fix:** In server_fns.rs (ssr), add `fn prepare_cell(request: &CompileRequest) -> Result<(CellTarget, CellFeatures), ServerFnError>`. It calls reject_uncompilable_cell_type, derives `CellTarget::from`, carries the single copy of the 'one place' comment, validates the id with is_valid_cell_id (same message), and returns `CellFeatures::detect(..)` from compiler-1. If compiler-1 is not done, return the three bools. Add `fn scaffold_request(config: &AppConfig, request: &CompileRequest, target: CellTarget) -> Result<(PathBuf, u32), ServerFnError>` wrapping scaffold_micro_crate plus its `scaffold failed` map_err. This is also the one place to add spawn_blocking for compiler-3. Add `pub const WORKSPACE_SESSION: &str = "default";` in compiler/mod.rs, referenced from the CompileLocks doc, and use it in both cores and in the test at server_fns.rs:2683. Both cores call prepare_cell first, keeping compile's validate-before-lock order, and call scaffold_request where they scaffold today.

**Tests:** Existing: the server_fns tests driving compile_cell_core/check_cell_core (2478-2590 reject/target tests, 2668, 2778, 2834 rate limit, 2877 check skip). Add `both_cores_reject_an_unsafe_cell_id`, which sends cell_id "../x" to compile_cell_core and check_cell_core and asserts both errors contain "invalid cell_id". Neither core has such a test today.

### server-fns-10: compile_cell_core and check_cell_core duplicate their request prologue verbatim

Merged into **compiler-2** (the same finding, seen from the server-fns-and-db partition).

### compiler-5: build_micro_crate and check_micro_crate duplicate the cargo environment setup

*dry · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/compiler/build.rs:256` (also `crates/ironpad-app/src/compiler/build.rs:557`, `crates/ironpad-app/src/compiler/build.rs:288`, `crates/ironpad-app/src/compiler/build.rs:584`)

**Problem:** `configure_cargo_cmd` was extracted to stop the build and check entry points from drifting apart, as its doc says. The ~20 lines around it are still copied in both functions: `effective_features`, atomics-vs-session target-dir selection, create_dir_all ×2, canonicalize ×2, `Command::new("cargo")`, and stdout/stderr piping. A change to target-dir policy (for example a separate autodiff dir) has to be made twice, which is the drift the helper exists to prevent.

**Fix:** In build.rs add `async fn prepare_cargo(subcommand: &str, crate_dir: &Path, cache_dir: &Path, session_id: &str, compilation_proxy: Option<&str>, target: CellTarget, features: CellFeatures) -> anyhow::Result<PreparedCargo>` with `struct PreparedCargo { cmd: Command, cargo_home: PathBuf, target_dir: PathBuf, features: CellFeatures /* post-for_target */ }`. It applies for_target (or effective_features if compiler-1 is not done), picks atomics_target_dir vs target_dir, runs the tokio create_dir_all/canonicalize calls and configure_cargo_cmd, and sets stdout/stderr piped. build_micro_crate keeps its info! log using the returned fields and uses `prepared.target_dir` for expected_wasm_path. check_micro_crate keeps its debug! log. Update configure_cargo_cmd's doc to say it is reached only through prepare_cargo.

**Tests:** Existing: build.rs compose_rustflags/cell_toolchain unit tests; `cargo make test-integration` (the e2e build tests plus all_public_notebook_cells_compile, which drive both entry points, including rayon's atomics target dir). Add a unit test that `prepare_cargo("check", ..)` with atomics features on the Executor target returns target_dir == canonical atomics_target_dir, and on the Linux target returns the session target dir with features == default. That pins the masking and dir policy in one place.

### compiler-8: Public-notebook compile gate reimplements the target mapping and slot detection instead of using the production recipe

*dry · partial · value 3 · effort S*

**Where:** `crates/ironpad-app/src/compiler/mod.rs:1878` (also `crates/ironpad-app/src/compiler/mod.rs:1893`, `crates/ironpad-app/src/compiler/mod.rs:1899`, `crates/ironpad-app/src/compiler/mod.rs:1944`)

**Problem:** `all_public_notebook_cells_compile` is the only whole-notebook compile coverage. It hand-codes `if is_linux() { Linux } else { Executor }` instead of `CellTarget::from`, which cache_key.rs:52 calls 'The one mapping'. It detects piped inputs with its own scan: `(0..10)` misses `cell10`+ and matches `mycell1`/`cell1x`, and the whitespace-token `last` check misses `*last`, `&last` and `{last}`. `ironpad_common::cell_deps::referenced_slots` exists to be the single source for this and handles all of those cases. Any mismatch means the gate either skips a checkable cell or scaffolds a piped cell with empty types and reports a false failure.

**Verifier's correction:** The gate (mod.rs:1878-1882) hand-codes `if is_linux() { Linux } else { Executor }` instead of `CellTarget::from` (cache_key.rs:52). Its slot scan (1893-1903) is a divergent copy of cell_deps::referenced_slots. One claim is wrong: `(0..10).any(|i| contains("cell{i}"))` does NOT miss cell10+, since `cell10` contains `cell1`. It does over-match `mycell1`/`cell1x`, skipping checkable cells. The whitespace-token `last` test misses `*last`, `&last`, `{last}`, `last.len()` and `last;`, and never checks the `__ironpad_inputs__` raw-buffer arm that referenced_slots reports. Switching moves coverage in both directions. Cells matched only by the over-match get checked, and cells using a local `last` in forms like `{last}` get skipped (they are compile-clean today, so that coverage is lost). That matches production and is acceptable.

**Fix:** In all_public_notebook_cells_compile, write `let target = CellTarget::from(cell.cell_type);` (dropping the hand-written if/else) and replace `uses_cell_ref`/`uses_last_binding` with `let piped = !target.is_linux() && !ironpad_common::cell_deps::referenced_slots(&cell.source).is_empty();`, keeping the Linux-exemption comment and shortening the rest to 'same recipe the scaffold binds from'. Fold the detection lines at 1944-1950 into compiler-1's `CellFeatures::detect`.

**Tests:** This is itself the gate test (`cargo make test-integration`). Before and after, log `total_cells`. Confirm the count changes only for cells the diff explains and that the gate stays green. referenced_slots is unit-tested in cell_deps.rs.

### compiler-15: CompileLocks::acquire and try_acquire duplicate the prune-and-fetch block

*dry · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/compiler/mod.rs:47` (also `crates/ironpad-app/src/compiler/mod.rs:68`)

**Problem:** Both methods repeat `lock_key`, table lock, `retain(strong_count > 1)` and `entry().or_default().clone()`. The long safety comment explaining why the prune preserves mutual exclusion is attached to only one copy. A change to the pruning rule has to be made in both, and the copy without the comment is the one more likely to be edited carelessly.

**Fix:** In compiler/mod.rs add `fn cell_lock(&self, cell_id: &str) -> Arc<tokio::sync::Mutex<()>>` containing lock_key, the table lock, the retain (with the existing safety comment moved onto it) and the entry clone. Then `acquire` becomes `self.cell_lock(cell_id).lock_owned().await` and `try_acquire` becomes `self.cell_lock(cell_id).try_lock_owned().ok()`. The table MutexGuard drops at the end of cell_lock, before the await, as today.

**Tests:** Existing: compile_locks_tests covers it: try_acquire_skips_when_busy_and_succeeds_when_free, the -/_ normalization test (table_len == 1), idle_locks_are_pruned (159) and in_use_locks_are_not_pruned (177). Add `try_acquire_also_prunes_idle_entries`: acquire+drop cell-a, then try_acquire cell-b, and assert table_len() == 1. That locks in that both paths share the prune.

### compiler-12: generate_cargo_toml and generate_linux_cargo_toml copy the deps/extra-sections assembly

*dry · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-app/src/compiler/scaffold.rs:257` (also `crates/ironpad-app/src/compiler/scaffold.rs:310`, `crates/ironpad-app/src/compiler/scaffold.rs:215`, `crates/ironpad-app/src/compiler/scaffold.rs:291`)

**Problem:** Both generators call `merge_dependencies` + `extract_extra_sections` and then run the same 14-line 'push block, ensure trailing newline, blank line before extras' tail. The Linux doc says the merge is 'exactly as they are for an ordinary cell', but that equivalence is kept by copy, not by construction.

**Fix:** In scaffold.rs add `fn append_user_sections(toml: &mut String, merged_deps: &str, extra_sections: &str)` containing the two `if !x.is_empty() { push; ensure '\n' }` blocks, with the blank line before extras. Replace scaffold.rs:257-273 and 310-326 with `append_user_sections(&mut toml, &merged_deps, &extra_sections);`. The ordinary path keeps its enforce_autodiff_profile rewrite before the call.

**Tests:** Existing: scaffold.rs generates_valid_cargo_toml, cargo_toml_with_no_user_deps, cargo_toml_with_rayon_enables_feature, extra_sections_* and the Linux scaffold tests pin the output. Add one assertion that for identical (user, shared) inputs, the text after `[dependencies]` + scaffold-owned deps is equal between generate_cargo_toml and generate_linux_cargo_toml. That turns the doc's 'exactly as' claim into a test.

### compiler-9: atomic temp-then-rename write is implemented twice (sync and async)

*dry · partial · value 2 · effort S*

**Where:** `crates/ironpad-app/src/compiler/cache.rs:133` (also `crates/ironpad-app/src/server_fns.rs:970`)

**Problem:** `cache::atomic_write` (std::fs) and `server_fns::atomic_write_async` (tokio::fs) are the same algorithm: uuid `.tmp.` sibling, write, rename, best-effort remove on failure. The async doc even says 'Async flavor of compiler::cache's atomic write'. A fix to one, such as fsync before rename or a temp-name collision guard, will not reach the other.

**Verifier's correction:** True. cache::atomic_write (cache.rs:133-145, std::fs) and server_fns::atomic_write_async (server_fns.rs:969-985, tokio::fs, with richer error context) are the same algorithm, and the async doc says it is the 'Async flavor of compiler::cache's atomic write'. The merge only makes sense once cache.rs is async (compiler-3). Without that, unifying a sync and an async variant means spawn_blocking glue that costs more than the 12-line copy.

**Fix:** Do this as part of compiler-3. Once try_cache_hit/store_blob are async, keep one `pub(crate) async fn atomic_write(path: &Path, contents: &[u8]) -> anyhow::Result<()>` in compiler/cache.rs, taking the server_fns body with its path-bearing error messages. Delete atomic_write_async and point server_fns.rs:1122/1124/1187 at `crate::compiler::cache::atomic_write`. If compiler-3's snapshot copy lands, add `pub(crate) async fn atomic_copy(src, dst)` next to it using the same temp-sibling-and-rename discipline. Skip this if compiler-3 is not done.

**Tests:** Existing: cache.rs `store_leaves_no_temp_files` and `store_overwrites_existing_blob` (as #[tokio::test]); server_fns snapshot/manifest tests exercise the share-side callers. Add a test that a failed rename (destination is a directory) leaves no `.tmp.` sibling, which covers the cleanup arm neither copy tests today.

### server-2: 'End session: send SessionEnded, then drop guests' is written out three times

*dry · confirmed · value 4 · effort S*

**Where:** `crates/ironpad-server/src/ws.rs:203` (also `crates/ironpad-server/src/ws.rs:347`, `crates/ironpad-server/src/state.rs:407`, `crates/ironpad-server/src/state.rs:390`)

**Problem:** Three places repeat the same steps: build a SessionEnded frame, call broadcast_to_guests, then call disconnect_guests. They are host-disconnect cleanup (ws.rs:203-212), the EndSession control message (ws.rs:347-354), and the expiry sweep (state.rs:407-418). The ordering matters: the notice must be sent before the senders are dropped, or guests never learn why they were closed. That rule currently exists as three hand-kept copies. The sweep copy also makes state.rs reach back into `crate::ws::wire_msg`, so the two modules depend on each other. `disconnect_guests`'s doc (state.rs:390) says it disconnects guests 'sending them a close reason', but it sends nothing; each caller has to send the reason first.

**Fix:** In state.rs, add `pub async fn end_session_guests(&self, session_id: &str, msg_id: &str)`. It builds the SessionEnded frame, calls `self.broadcast_to_guests`, then `self.disconnect_guests`, with a comment saying why the notice must come before the drop. Move `wire_msg` from ws.rs into state.rs as `pub(crate) fn wire_msg` (ws.rs then imports it from `crate::state`), which removes the state->ws edge. Call sites: in ws.rs:203 the loop body becomes `state.ws.end_session_guests(session_id, "").await;`; ws.rs:347 becomes `state.ws.end_session_guests(session_id, msg_id).await;` after invalidate_session; in state.rs:407 the loop body becomes `self.end_session_guests(session_id, "").await;`. Fix the disconnect_guests doc to 'Drops all guest senders on a session (closing their sockets); callers send the reason first, see end_session_guests.'

**Tests:** All three paths are already covered: state.rs `sweeping_an_expired_session_notifies_and_disconnects_its_guests` (877), ws.rs `host_end_session_broadcasts_and_disconnects` (1004), and tests/relay.rs `host_disconnect_ends_guest_session` (423). Add a direct unit test in state.rs, `end_session_guests_notifies_before_disconnecting`: register a guest, call it, assert the receiver yields a SessionEnded frame carrying the given msg_id, then yields None (channel closed).

### server-fns-2: get_mutable_notebook_core re-implements the mutable access gate instead of delegating to it

*dry · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/server_fns.rs:1563` (also `crates/ironpad-app/src/server_fns.rs:1805-1851`, `crates/ironpad-app/src/server_fns.rs:1547-1549`, `crates/ironpad-server/src/og/mod.rs:461`, `crates/ironpad-server/src/oembed.rs:194`)

**Problem:** CLAUDE.md names mutable_access_core as THE one gate. But get_mutable_notebook_core, which the OG card and oEmbed use, restates the whole sequence itself: fetch the row, check published_copy, check private, then parse with the same error text. Called with viewer=None, mutable_access_core already gives exactly this answer. Unpublished yields NotFound. Private yields Private, because private_share_readable returns false for no viewer without touching the DB. Everything else yields Found. So the two agree today only because both copies have been kept in step by hand. A future visibility rule added to mutable_access_core would reach the reader page and embed but not the anonymous unfurl surfaces. That is the 'a fourth surface inherits two of the three' failure the published_copy doc warns about.

**Fix:** Replace the body of get_mutable_notebook_core with `use ironpad_common::MutableNotebookAccess; Ok(match mutable_access_core(db, id, None).await? { MutableNotebookAccess::Found(r) => Some(r.notebook), MutableNotebookAccess::Private { .. } | MutableNotebookAccess::NotFound => None })`. Keep its doc comment's 'private returns None unconditionally for crawler surfaces' sentence and add 'because it is mutable_access_core with no viewer'. Update published_copy's doc so it says the rule is consumed by two access cores (mutable_access_core and mutable_manifest_access_core) rather than three.

**Tests:** Well covered already: roughly 15 get_mutable_notebook_core assertions in server_fns.rs tests (lines 3520-4280) over public, private, unpublished, published-then-unpublished and missing ids, plus crates/ironpad-server/tests/unpublished_notebooks.rs over the raw OG and oEmbed bodies. This is a pure refactor, so no new test is required. Run those suites plus the ironpad-server OG/oEmbed tests.

### server-fns-7: Notebook upload validation is copied into three cores, and share_notebook parses the upload twice

*dry · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/server_fns.rs:780` (also `crates/ironpad-app/src/server_fns.rs:1400-1409`, `crates/ironpad-app/src/server_fns.rs:1478-1485`, `crates/ironpad-app/src/server_fns.rs:857`)

**Problem:** The upload gate is pasted into share_notebook_core_capped, save_notebook_to_account_core and save_mutable_draft_core: a `len > MAX_SHARE_BYTES` bail with the same message, then a serde parse into IronpadNotebook with 'invalid notebook JSON'. save_notebook_to_account_core's own doc (1387-1391) warns that two upload paths meant 'two places to validate JSON … and only one of them would ever get the next fix', yet the validation itself still exists in three copies. Separately, share_notebook re-parses the same body of up to 4 MiB at 857, after share_notebook_core already parsed and discarded it. The comment there concedes the second parse cannot fail.

**Fix:** Next to MAX_SHARE_BYTES, add `#[cfg(feature = "ssr")] fn validate_notebook_upload(notebook_json: &str) -> anyhow::Result<IronpadNotebook>` with the size bail (keeping the existing 'reject before parsing' comment) and the parse. Call it from all three cores; the two that discard the result bind `let _ =`. Move share_notebook_core_capped's body after validation into `async fn store_share(data_dir, notebook_json, max_total_bytes) -> anyhow::Result<String>`. Add `pub(crate) async fn share_notebook_parsed(data_dir, notebook_json) -> anyhow::Result<(String, IronpadNotebook)>` that validates once and stores. Have share_notebook call that and snapshot from the returned notebook, deleting the second `serde_json::from_str` and its 'unreachable' comment. share_notebook_core and share_notebook_core_capped keep their signatures by delegating.

**Tests:** Existing: the share_notebook_core oversized/invalid/idempotent tests (server_fns.rs:2919-2990), the save_mutable_draft and save_notebook_to_account tests, which assert oversize and invalid rejection, and the share snapshot tests. Add a unit test for validate_notebook_upload asserting the exact error prefixes for oversize and garbage, plus Ok for VALID_NOTEBOOK_JSON.

### server-fns-8: The OWNER gate and its non-oracle message are pasted five times, under two conventions

*dry · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/server_fns.rs:1486` (also `crates/ironpad-app/src/server_fns.rs:1511-1513`, `crates/ironpad-app/src/server_fns.rs:1595-1597`, `crates/ironpad-app/src/server_fns.rs:1609-1611`, `crates/ironpad-app/src/server_fns.rs:1938-1946`, `crates/ironpad-app/src/server_fns.rs:1743`, `crates/ironpad-app/src/server_fns.rs:1771`, `crates/ironpad-app/src/server_fns.rs:1954`, `crates/ironpad-app/src/server_fns.rs:2001`, +1 more)

**Problem:** `if !db.user_owns_share(github_id, id).await? { bail!("unauthorized: you do not own this share") }` is pasted into four cores, and require_share_owner restates it a fifth time with its own copy of the message. The deliberate non-oracle property ('a share you do not own reads exactly like one that does not exist', 1593-1594) therefore lives in five places. There are also two conventions for the same gate. save, push, unpublish and delete check ownership inside a `*_core`. get_mutable_for_edit, discard, set_private, grant/revoke and get_mutable_access check it in the #[server] wrapper.

**Fix:** Add `#[cfg(feature = "ssr")] async fn ensure_share_owner(db: &crate::db::Db, github_id: &str, id: &str) -> anyhow::Result<()>` just above require_share_owner. It holds the non-oracle doc sentence and the single message. Replace the four core sites with `ensure_share_owner(db, github_id, id).await?;`. Rewrite require_share_owner as `let user = require_login(db).await?; ensure_share_owner(db, &user.github_id, id).await.map_err(|e| ServerFnError::new(e.to_string()))?; Ok(user)`. Skip moving the wrapper-gated bodies into cores.

**Tests:** Existing: the server_fns tests asserting that a non-owner's save_draft/push/unpublish/delete fails with 'unauthorized', and the private-share grant tests that go through require_share_owner. Add one test asserting that a nonexistent id and a share owned by someone else produce byte-identical error strings from ensure_share_owner. That test pins the non-oracle property in one place.

### server-fns-9: The path-traversal predicate is written three times across two crates and already differs

*dry · partial · value 2 · effort S*

**Where:** `crates/ironpad-app/src/server_fns.rs:668` (also `crates/ironpad-server/src/og/mod.rs:413`, `crates/ironpad-server/src/oembed.rs:109`)

**Problem:** The segment check (no '/', no '\\', no "..") exists as validate_safe_path_segment (private, so the server crate cannot reuse it), inside og's notebook_id_from, and inside oembed's target parser. The copies already differ: both server copies reject an empty segment and the app copy does not. This check is what stands between a URL segment and `data_dir.join(...)`. Tightening it in one copy (for example, also rejecting NUL or a leading '.') would leave the others open.

**Verifier's correction:** The copies exist as described: validate_safe_path_segment (server_fns.rs:668-673), og/mod.rs:413 and oembed.rs:109. They also differ on the empty string. The security framing is overstated, though. The og and oembed copies are documented pre-filters ('The _core loaders validate too', og/mod.rs:408-410), and mutable ids never reach a filesystem join (they are DB keys). The gate that actually protects the join is the app copy at 682, 1199 and 1237. So the claim that tightening one copy 'would leave the others open' does not hold for the filesystem. The real risk is drift of the pre-filter semantics. That still makes it a cheap DRY fix, but a low-value one.

**Fix:** Make the app predicate the source: in ironpad-common (for example beside PublicNotebookSummary in types.rs, or in a small `paths.rs`), add `#[must_use] pub fn is_safe_path_segment(s: &str) -> bool { !s.is_empty() && !s.contains(['/', '\\']) && !s.contains("..") }`. Have validate_safe_path_segment bail when `!is_safe_path_segment(s)`. It now also rejects empty, which only turns a 'notebook not found' into 'invalid filename'. Replace the inline checks at og/mod.rs:413 and oembed.rs:109 with `!ironpad_common::is_safe_path_segment(id)`.

**Tests:** Existing: og/mod.rs tests (Class::parse("..") and notebook_id_from cases), oembed target-parsing tests, and get_public_notebook_core traversal tests. Add an ironpad-common unit test table covering "", "a/b", "a\\b", "..", "a..b" and "welcome". Check whether any get_public_notebook_core test depends on the old message for the empty case.

### server-fns-14: The public-notebook name <-> filename rule is re-derived at seven sites across two crates

*dry · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-app/src/lib.rs:350` (also `crates/ironpad-app/src/server_fns.rs:689-693`, `crates/ironpad-app/src/pages/public_notebook.rs:55-58`, `crates/ironpad-app/src/pages/home_page.rs:579`, `crates/ironpad-app/src/components/view_only_notebook.rs:42`, `crates/ironpad-server/src/oembed.rs:107`, `crates/ironpad-server/src/crawl.rs:62`)

**Problem:** PRD-0048's rule, that the canonical public name is the filename without `.ironpad`, is re-derived with `strip_suffix(".ironpad").unwrap_or(..)` at six sites across both crates. Its inverse (append the extension when missing) lives inline in get_public_notebook_core. The extension literal is the only thing that ties them together.

**Fix:** In ironpad-common/src/types.rs beside PublicNotebookSummary, add `pub const PUBLIC_NOTEBOOK_EXT: &str = ".ironpad";`, `#[must_use] pub fn public_notebook_name(filename: &str) -> &str { filename.strip_suffix(PUBLIC_NOTEBOOK_EXT).unwrap_or(filename) }` and `#[must_use] pub fn public_notebook_filename(name: &str) -> std::borrow::Cow<'_, str>` (Borrowed when it already ends with the extension, else Owned with the extension appended). Replace the six strip sites with public_notebook_name. Replace server_fns.rs:689-693 with `public_notebook_filename(filename)`, keeping its PRD-0048 comment. Do not touch ironpad-cli daemon.rs:29 (a `~/.ironpad` directory, unrelated).

**Tests:** Existing: crawl.rs sitemap test (asserts no `.ironpad` in the XML), oembed legacy-form tests, get_public_notebook_core tests for both name forms, and the legacy redirect e2e. Add ironpad-common unit tests for both fns: round-trip, already-suffixed input, and a name containing '.ironpad' mid-string.

### cp-11: Notebook SocialMeta block triplicated and `.ironpad` stripping spelled out at six sites

*dry · partial · value 2 · effort S*

**Where:** `crates/ironpad-app/src/pages/public_notebook.rs:62` (also `crates/ironpad-app/src/pages/shared_notebook.rs:66`, `crates/ironpad-app/src/pages/mutable_notebook.rs:223`, `crates/ironpad-app/src/pages/public_notebook.rs:55`, `crates/ironpad-app/src/pages/home_page.rs:579`, `crates/ironpad-app/src/components/view_only_notebook.rs:42`, `crates/ironpad-app/src/lib.rs:351`, `crates/ironpad-server/src/oembed.rs:107`, `crates/ironpad-server/src/crawl.rs:62`)

**Problem:** All three notebook pages emit the same `<SocialMeta>`: title and description from the notebook, `path=/{class}/{id}`, `image=og_image_path().map_or_else(|| format!("/og/{class}/{id}.png"), ..)`, `image_size=og_image_dimensions()` and `oembed=true`, differing only in class and `noindex`. The OG card path recipe is therefore written three times against the server's `/og/{class}/{file}` route. Separately, the canonical-public-name rule (strip `.ironpad`, PRD-0048) is re-implemented as `strip_suffix(".ironpad").unwrap_or(..)` in six places across three crates.

**Verifier's correction:** The duplication is true. `<SocialMeta ... image=og_image_path().map_or_else(|| format!("/og/{class}/{id}.png"), ..) image_size=.. oembed=true>` appears at public_notebook.rs:62-73, shared_notebook.rs:66-79 and mutable_notebook.rs:223-237. `strip_suffix(".ironpad").unwrap_or(..)` appears at the six cited sites (crawl.rs:62, oembed.rs:107, lib.rs:351, home_page.rs:579, public_notebook.rs:56, view_only_notebook.rs:42). The canonical-public-name rule (PRD-0048) and the `/og/{class}/{id}.png` recipe are the real single-source concerns. A wrapper component taking a reference prop, as proposed (`notebook: &IronpadNotebook`), does not work as a Leptos prop without a lifetime or a clone. The wrapper is also the least valuable part, since the three blocks differ in `noindex` and in which id they use.

**Fix:** Do the two helpers and skip the component. (1) Add `pub fn public_notebook_name(filename: &str) -> &str { filename.strip_suffix(".ironpad").unwrap_or(filename) }` in ironpad-common (next to the notebook types, re-exported at the crate root). Replace all six sites. (2) Add `pub fn og_image_for(&self, class: &str, id: &str) -> String { self.og_image_path().map_or_else(|| format!("/og/{class}/{id}.png"), str::to_string) }` on `IronpadNotebook`, which already owns `og_image_path`. Use it as `image=notebook.og_image_for("public", &meta_name)` etc. on the three pages. While there, fix the stale oembed doc at social_meta.rs:97-99 (cp-15).

**Tests:** Add unit tests in ironpad-common: `public_notebook_name("a.ironpad") == "a"`, `("a") == "a"`, `("a.ironpad.ironpad") == "a.ironpad"`, and `og_image_for` with and without an override. Existing guards: social-preview.spec.ts (raw-body og:image/og:url), oembed.spec.ts, routes.spec.ts (legacy .ironpad redirects), and the ironpad-server crawl/oembed unit tests.

### server-8: Cookie parsing is written twice even though the app's auth module says it is the single definition

*dry · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-server/src/auth.rs:333` (also `crates/ironpad-app/src/auth.rs:24`, `crates/ironpad-app/src/auth.rs:3`)

**Problem:** Server `cookie_value` (auth.rs:333-339) has the same body as the app's `session_token_from_cookie_header` (app auth.rs:24-29): split on ';', trim, split_once('='), then compare the name and reject empty values. The only difference is that the app version hard-codes SESSION_COOKIE. The app module doc (app auth.rs:3-5) says 'The cookie name and parsing live here so the Axum auth routes (ironpad-server) and the #[server] fns (this crate) agree on one definition'. That holds for the name but not for the parsing. A change to parsing on one side (quoting, whitespace around '=') would make logout and CSRF checks disagree with session resolution.

**Fix:** ironpad-app/src/auth.rs: add `pub fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str>` with the existing body, parameterized on name. `session_token_from_cookie_header` becomes `cookie_value(header, SESSION_COOKIE)`. ironpad-server/src/auth.rs:333: the body becomes `ironpad_app::auth::cookie_value(headers.get(header::COOKIE)?.to_str().ok()?, name)`.

**Tests:** Existing: ironpad-app auth.rs tests at 203-214 (prefix-name rejection, empty value, multi-cookie) now exercise the shared parser; server `callback_rejects_a_state_mismatch` (488) and `logout_clears_the_cookie_even_without_a_session` (500) exercise the server path. Add one app unit test calling `cookie_value("a=1; ironpad_oauth_state=n", "ironpad_oauth_state") == Some("n")`, so the generalized name path is covered.

### server-fns-13: ironpad-server's auth re-implements the cookie parsing and hex token minting this crate owns

Merged into **server-8** (the same finding, seen from the server-fns-and-db partition).

### server-9: max_concurrent_builds clap default is a literal copy of DEFAULT_MAX_CONCURRENT_BUILDS

*dry · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-server/src/config.rs:52` (also `crates/ironpad-app/src/compiler/admission.rs:25`, `crates/ironpad-server/src/state.rs:36`)

**Problem:** `default_value_t = 3` repeats `ironpad_app::compiler::admission::DEFAULT_MAX_CONCURRENT_BUILDS` (also 3), which `BuildAdmission::default` uses. The same file already avoids this for max_guests and guest_idle_timeout_secs by referencing the state.rs constants. state.rs:36-38 gives the reason: 'one constant, not two hand-synced literals'. max_concurrent_builds is the one knob that was missed.

**Fix:** config.rs:52: `#[arg(long, default_value_t = ironpad_app::compiler::admission::DEFAULT_MAX_CONCURRENT_BUILDS, env = "IRONPAD_MAX_CONCURRENT_BUILDS")]`.

**Tests:** config.rs `default_values` (202) does not assert max_concurrent_builds. Add `assert_eq!(args.max_concurrent_builds, ironpad_app::compiler::admission::DEFAULT_MAX_CONCURRENT_BUILDS);` there.

### server-1: Storage-class dispatch, class list and id validation duplicated between og and oembed; oembed is stringly typed

*dry · partial · value 3 · effort M*

**Where:** `crates/ironpad-server/src/oembed.rs:187` (also `crates/ironpad-server/src/og/mod.rs:451`, `crates/ironpad-server/src/og/mod.rs:411`, `crates/ironpad-server/src/og/mod.rs:79`, `crates/ironpad-server/src/oembed.rs:72`, `crates/ironpad-server/src/oembed.rs:99`, `crates/ironpad-server/src/oembed.rs:109`, `crates/ironpad-app/src/server_fns.rs:668`)

**Problem:** Two handlers load a notebook from a storage class plus an id. `notebook_card_handler` (og/mod.rs:451-466) and `oembed_handler` (oembed.rs:187-201) each write the same three-way dispatch: public goes to get_public_notebook_core(site_root), shared to get_shared_notebook_core(data_dir), and mutable to get_mutable_notebook_core(db), where Ok(None) becomes Err. The two copies spell that last conversion differently. oEmbed also keeps its own copy of the class list: `EmbedTarget.class` is a `&'static str` (oembed.rs:72-77) matched with a `_ =>` catch-all that sends anything other than public or mutable to the shared loader. A fourth class added in `embed_target` (oembed.rs:99-103) would therefore load silently as a shared hash, and nothing would force the match to be updated. Meanwhile `og::Class::route_prefix` (og/mod.rs:79-85) is dead code: nothing calls it, and it holds exactly the `/public` `/shared` `/mutable` list that oembed rebuilds by hand. The traversal check `is_empty() || contains('/') || contains('\\') || contains("..")` appears at og/mod.rs:413 and oembed.rs:109, and a third time (without the empty check) as `validate_safe_path_segment` at server_fns.rs:668.

**Verifier's correction:** The core claim holds. og/mod.rs:451-466 and oembed.rs:187-201 both write the public/shared/mutable dispatch, and they spell the Ok(None)->Err conversion differently. `EmbedTarget.class` is a `&'static str` (oembed.rs:72-77) matched with `_ =>`, which sends anything that is not public or mutable to `get_shared_notebook_core` (oembed.rs:197). `og::Class::route_prefix` (og/mod.rs:79) has no caller anywhere in the workspace: grep finds only its definition, and its doc still says it is 'used to build og:url'. The id check is repeated between og/mod.rs:413 and oembed.rs:109. The scope needs two corrections. (a) Leave server_fns' `validate_safe_path_segment` alone. It is private, ssr-gated, lives in another crate, and deliberately has no empty check, because the `_core` loaders simply fail to find an empty name. Making it pub would widen ironpad-app's API to save one line. Share the check inside the server crate only. (b) Leave the loader in og/ or put it in a new server-lib module. It must stay out of ironpad-app, because it needs AppState, which lives in the server crate.

**Fix:** 1. Create `crates/ironpad-server/src/notebook_class.rs` (and `pub mod notebook_class;` in lib.rs), or keep the enum in og/mod.rs. Put `og::Class` there, keeping `parse`/`label`, and add:
   - `pub const ALL: [Class; 3] = [Public, Shared, Mutable];`
   - `pub fn segment(self) -> &'static str` ("public"/"shared"/"mutable"). Make `parse` = `ALL.into_iter().find(|c| c.segment() == s)` and `route_prefix` = derived from segment, or delete route_prefix.
   - `pub async fn load(self, state: &AppState, db: &Db, id: &str) -> Option<IronpadNotebook>`, holding the one three-way dispatch. Mutable goes through `get_mutable_notebook_core(db, id).await.ok().flatten()`, and the comment says unpublished/private resolve to None, i.e. 404.
   - `pub(crate) fn is_safe_id(id: &str) -> bool { !id.is_empty() && !id.contains(['/', '\\']) && !id.contains("..") }`.
   Re-export the enum from og (`pub use crate::notebook_class::Class;`) so `og::Class` keeps its path.
2. og/mod.rs: `notebook_id_from` uses `is_safe_id`. `notebook_card_handler` becomes `let Some(notebook) = class.load(&state, &db, id).await else { 404 };`.
3. oembed.rs: change `EmbedTarget.class: Class`. In `embed_target`, replace the or_else chain with `Class::ALL.into_iter().find_map(|c| path.strip_prefix(&format!("/{}/", c.segment())).map(|id| (c, id)))?`, use `is_safe_id`, and build `embed_path` from `class.segment()`. The handler's match becomes `target.class.load(&state, &db, &target.id).await`. Delete the `_ =>` arm.

**Tests:** Existing guards: the oembed.rs unit tests on `embed_target` (roughly 7 `EmbedTarget { class: "..." }` literals become `Class::Public` etc., plus `refuses_classes_without_an_embed_route`); the og/mod.rs unit tests; and tests/unpublished_notebooks.rs, which drives /og and /oembed over raw bodies for anonymous and stranger identities with a positive control, so it catches a load regression on the mutable path. Add a unit test asserting `Class::ALL.iter().all(|c| Class::parse(c.segment()) == Some(*c))`, and an oembed test that each `Class::ALL` member round-trips through `embed_target` into a matching `embed_path`.

### server-5: Non-Leptos route table exists only in the binary, so integration tests copy it by hand

*soc · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-server/src/main.rs:214` (also `crates/ironpad-server/tests/relay.rs:54`, `crates/ironpad-server/tests/unpublished_notebooks.rs:103`, `crates/ironpad-server/tests/unpublished_notebooks.rs:105`)

**Problem:** main.rs:214-228 maps the path strings to the lib's handlers (/ws/host, /ws/connect, /og/ironpad.png, /og/{class}/{file}, /robots.txt, /sitemap.xml, /oembed). Because main.rs is a binary, the integration tests cannot reach that table. relay.rs:54-59 and unpublished_notebooks.rs:105-112 each rebuild it, and the second says outright it is 'wired exactly as `main.rs` wires them' (line 103), which is a promise to keep them in sync by hand. If a path or extractor changes in main.rs, the tests keep testing the old wiring and still pass.

**Fix:** Add `crates/ironpad-server/src/routes.rs` (and `pub mod routes;` in lib.rs) containing:
- `pub fn ws_routes() -> Router<AppState>`: /ws/host and /ws/connect.
- `pub fn crawler_routes() -> Router<AppState>`: /og/ironpad.png, /og/{class}/{file}, /robots.txt, /sitemap.xml, /oembed. The doc notes that callers must layer `axum::Extension(Db)`, because og and oembed extract it.
In main.rs, replace those seven `.route(...)` calls with `.merge(ironpad_server::routes::ws_routes()).merge(ironpad_server::routes::crawler_routes())` before `.leptos_routes_with_context`, keeping the PRD comments at the new definitions. In tests/relay.rs, `ws_router` becomes `ironpad_server::routes::ws_routes().with_state(state)`. In tests/unpublished_notebooks.rs, `router` becomes `routes::crawler_routes().with_state(state).layer(axum::Extension(db))`, and its doc becomes 'the production crawler routes'.

**Tests:** tests/relay.rs and tests/unpublished_notebooks.rs keep running and now exercise the production table. Add a small test in unpublished_notebooks.rs (or a new routes test) that GETs /robots.txt and /og/ironpad.png through `crawler_routes()` and asserts 200, so every route in the table is reached at least once.

### server-10: Three XML/HTML escapers in one crate; only one handles XML-illegal control characters

*dry · partial · value 2 · effort S*

**Where:** `crates/ironpad-server/src/og/svg.rs:461` (also `crates/ironpad-server/src/crawl.rs:83`, `crates/ironpad-server/src/oembed.rs:152`)

**Problem:** svg.rs:461, crawl.rs:83 and oembed.rs:152 each escape `&<>"'` character by character. The svg version also drops C0 controls. Its doc explains why: XML 1.0 cannot encode them, so one in the text makes the whole document unparseable. The sitemap is also XML and does not drop them, so a filename containing a control character produces a sitemap that crawlers discard. oembed writes `&#39;` where the others write `&apos;`. Both are valid in HTML5; the difference is only that the copies drifted apart.

**Verifier's correction:** The three escapers exist (svg.rs:461, crawl.rs:83, oembed.rs:152), and only svg drops XML-illegal C0 controls. The severity is overstated, though. Sitemap input is bundled filenames under public/notebooks (deployer-controlled, per the crawl.rs:80-81 doc), so a control character there is practically impossible. The proposed fix is also wrong on one detail: collapsing oembed onto an XML escaper that emits `&apos;` changes HTML handed to third-party pages, and `&apos;` is not an HTML4 entity. oEmbed `html` goes into arbitrary consumer pages, which is presumably why `&#39;` is used there. The DRY win is real but small. `&#39;` is valid in XML and in every HTML version, so one escaper emitting it serves all three.

**Fix:** Create `crates/ironpad-server/src/escape.rs` (with `mod escape;` in lib.rs) holding `pub(crate) fn markup_escape(s: &str) -> String`. It is the svg.rs body (drops C0 controls except \t\n\r) but emits `&#39;` for `'` (valid in XML 1.0 and in all HTML). Move the svg.rs doc explaining the control-char drop onto it. Replace `svg::escape`, `crawl::escape` and `oembed::escape_attr` with calls to it. Any svg/crawl tests that assert the literal `&apos;` need updating to `&#39;`.

**Tests:** Existing: svg.rs escape/control-char tests (the usvg-reject regression), crawl.rs sitemap tests, and oembed.rs tests for the escaped title in `html`. Add to escape.rs unit tests covering all five metacharacters plus a C0 control (dropped) and \t/\n (kept), and one crawl test asserting a filename containing `\u{1}` still yields a parseable `<loc>`.

### editor-6: "Runnable" predicate hand-rolled three times; Run All id collection copied five times

*dry · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/model.rs:296` (also `crates/ironpad-app/src/model.rs:309`, `crates/ironpad-app/src/pages/notebook_editor/state.rs:256-258`, `crates/ironpad-app/src/pages/notebook_editor/mod.rs:270-279`, `crates/ironpad-app/src/pages/notebook_editor/mod.rs:640-649`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:484-493`, `crates/ironpad-app/src/components/view_only_notebook.rs:173-182`, `crates/ironpad-app/src/components/view_only_notebook.rs:192-201`)

**Problem:** `CellManifest::is_runnable` is documented as 'The single definition of runnable' (ironpad-common types.rs:362-369). Yet `mark_downstream_stale`, `mark_all_code_cells_stale` and the reactive timer spell out `cell_type == CellType::Code && !shared` by hand. If the definition changes (e.g. Linux cells), stale-marking and reactive re-runs will silently diverge from Run All. Separately, the filter(is_runnable).map(id).collect-then-set-if-nonempty block exists in two editor keyboard/button handlers, Run All Below, and twice in the viewer.

**Fix:** Replace the three hand-rolled predicates with `c.is_runnable()` (and drop the now-unused CellType import in init_reactive_timer). Add `pub fn runnable_ids<C: PipingCell>(cells: &[C]) -> Vec<String> { cells.iter().filter(|c| c.is_runnable()).map(|c| c.id().to_owned()).collect() }` to components/executor.rs. Add to NotebookState (state.rs): `pub(super) fn enqueue_runnable_from(&self, from_id: Option<&str>)`, which computes the ids from `cells.with_untracked` starting at `from_id`'s index (or 0) and sets the queue only if it is non-empty. Call it from the Ctrl+Shift+Enter handler, the Run All button (`None`) and Run All Below (`Some(&cid)`). The viewer's two sites call `runnable_ids(&nb.cells)` inside `notebook.with_value`.

**Tests:** Guarded by stale_tests in model.rs, keyboard.spec.ts (Ctrl+Shift+Enter), execution.spec.ts (Run All) and public-notebooks.spec.ts (viewer autorun). Add an executor.rs unit test for `runnable_ids` that excludes markdown, shared and Linux cells and preserves order.

### cell-panels-1: From<T> for CellOutput re-derives each type's panel and tag instead of using its IntoPanels/TypeTag

*dry · partial · value 3 · effort M*

**Where:** `crates/ironpad-cell/src/lib.rs:594` (also `crates/ironpad-cell/src/lib.rs:756`, `crates/ironpad-cell/src/lib.rs:538`, `crates/ironpad-cell/src/lib.rs:854`, `crates/ironpad-cell/src/lib.rs:550`, `crates/ironpad-cell/src/lib.rs:860`, `crates/ironpad-cell/src/lib.rs:565`, `crates/ironpad-cell/src/lib.rs:866`, `crates/ironpad-cell/src/lib.rs:577`, +9 more)

**Problem:** Each type's display is written twice: once in its `From<T> for CellOutput` impl and once in its `IntoPanels` impl. Its type-tag literal is also written twice, in the From impl and in its `TypeTag` impl ("Svg" 538/854, "Html" 550/860, "Table" 565/866, "Md" 577/872, "Json" 589/878, "Canvas" 612/884). The Canvas BlobImage panel is built in both places (594-615 and 756-771), held together only by a "Must match" comment at 758. That drift has already happened once: CACHE_EPOCH 5->6 (cache_key.rs) was bumped because the IntoPanels path emitted an Html panel while From emitted BlobImage. The 15-type primitive list is also repeated in three macro invocations (483, 707, 836). The tuple impl (1052-1071) already has the generic shape: encode, `T::type_tag()`, `into_panels()`.

**Verifier's correction:** The duplication is real, and the Canvas case already drifted once (CACHE_EPOCH history, plus the "Must match" comment at lib.rs:758). The From/IntoPanels pairs for Canvas (594-615 vs 756-771) and Json (583-592 vs 750-754) do identical work, and the tag literals repeat the TypeTag impls (854-884). The primitive type list does appear in three macros (483, 707, 836). The fix needs two corrections. First, Table's From moves `headers`/`rows` (565-567), while `into_panels(&self)` clones them. Routing Table through `value.into_panels()` as proposed would add a full clone of a potentially large table, so Table must pass its moved panel the same way the string newtypes do. Second, Vec<T> diverges on purpose: From passes the type tag to format_vec_truncated (518), and IntoPanels passes None (717). Vec must stay out of the unification, and the folded primitive macro must keep that distinction. The output is byte-identical, so no epoch bump is needed.

**Fix:** lib.rs: add `fn typed_output<T: serde::Serialize + TypeTag>(value: &T, panels: Vec<DisplayPanel>) -> CellOutput { CellOutput { bytes: bincode::serde::encode_to_vec(value, bincode::config::standard()).expect("serialization of a TypeTag type cannot fail"), panels, type_tag: Some(T::type_tag()) } }`. Canvas and Json use `typed_output(&value, value.into_panels())`. Svg, Html, Md and Table encode first, then move their field into the panel: `let out = typed_output(&value, vec![]);` followed by a panel push would still need the value, so give the helper an `encode_then(value, |v| panels)` shape or compute bytes via the helper before destructuring. Prefer `fn typed_output_with<T>(value: T, panels: impl FnOnce(T) -> Vec<DisplayPanel>) -> CellOutput` that encodes &value, reads T::type_tag(), then calls panels(value). String keeps its move. &str and Vec<T> are unchanged. Merge `impl_from_for_cell_output!`, `impl_into_panels_for_primitive!` and `impl_type_tag_for_primitive!` into one `impl_primitive_output!(i8, ..., isize)` that emits all three impls, with From delegating to `typed_output(&value, value.into_panels())`, which is identical because `value.to_string()` equals `format!("{self}")`.

**Tests:** Before refactoring, add `from_matches_into_panels_and_type_tag`: for one value of each unified type (every primitive, String, Svg, Html, Md, Table, Json, Canvas), assert that `CellOutput::from(v.clone())` has the same panels as `v.into_panels()` (via `IntoPanels for CellOutput`), the same tag as `Some(T::type_tag())`, and the same bytes as `encode_to_vec(&v)`. This pins the invariant the Canvas drift broke. Existing tuple/Canvas round-trip tests in lib.rs cover the rest.

### cell-ui-1: Eight widgets each hand-write four impls, so kind, tag and value are spelled twice

*dry · confirmed · value 3 · effort M*

**Where:** `crates/ironpad-cell/src/ui.rs:90` (also `crates/ironpad-cell/src/ui.rs:106`, `crates/ironpad-cell/src/ui.rs:114`, `crates/ironpad-cell/src/ui.rs:185`, `crates/ironpad-cell/src/ui.rs:196`, `crates/ironpad-cell/src/ui.rs:263`, `crates/ironpad-cell/src/ui.rs:274`, `crates/ironpad-cell/src/ui.rs:341`, `crates/ironpad-cell/src/ui.rs:352`, +8 more)

**Problem:** Slider, Dropdown, Checkbox, TextInput, Number, Switch, Button and ProgressBar each have `From<W> for CellOutput`, `IntoPanels`, `TypeTag` and a hand-written `Serialize`, about 40 lines per widget and roughly 300 in total. In each one the kind literal appears twice (From and IntoPanels, e.g. "slider" 95/106), the type tag appears twice (From's `Some("f64".into())` 98 and TypeTag 114), and the piped value appears twice (`encode_bincode(&s.default)` in From and `self.default.serialize` in Serialize). These have to agree for tuple outputs such as `(slider, dropdown)` to pipe the same bytes and tags as a bare widget, and only copy-paste keeps them in agreement.

**Fix:** ui.rs: add a private `trait Widget { const KIND: &'static str; type Value: serde::Serialize + TypeTag; fn value(&self) -> &Self::Value; fn config_json(&self) -> String; }`. Move each existing inherent `config_json` into its impl, or have the trait method call the inherent one. The impls are: Slider/Number (f64, &self.default), Dropdown/TextInput (String), Checkbox/Switch (bool), Button (type Value = (); value returns &()), and ProgressBar (String, &self.id). Add `macro_rules! impl_widget { ($($w:ty),+) => { $( impl From<$w> for CellOutput { fn from(w: $w) -> Self { Self { bytes: encode_bincode(w.value()), panels: w.into_panels(), type_tag: Some(<<$w as Widget>::Value as TypeTag>::type_tag()) } } } impl IntoPanels for $w { fn into_panels(&self) -> Vec<DisplayPanel> { vec![DisplayPanel::Interactive { kind: <$w as Widget>::KIND.into(), config: Widget::config_json(self) }] } } impl TypeTag for $w { fn type_tag() -> String { <<$w as Widget>::Value as TypeTag>::type_tag() } } impl serde::Serialize for $w { fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> { self.value().serialize(s) } } )+ } }` and invoke it as `impl_widget!(Slider, Dropdown, Checkbox, TextInput, Number, Switch, Button, ProgressBar);`. Check that encode_bincode(&()) yields an empty Vec so Button's bytes are unchanged; it does under bincode 2 standard config, but assert it in the test.

**Tests:** Before the refactor, add `every_widget_pipes_identically_bare_and_in_a_tuple`: for each widget, assert `CellOutput::from(w)` bytes == `encode_bincode(&w)` == `encode_bincode(w's default)`, the tag equals the literal it has today ("f64"/"String"/"bool"/"()"), and the kind equals today's literal. Record the expected literals explicitly so the macro cannot silently change them. The existing ui.rs tests (795+ and 1110/1150) stay.

### cell-live-1: LiveContent kind is encoded twice in lib.rs (strings and 0/1/2), decoded twice in the app, and cloned needlessly

*dry · partial · value 2 · effort S*

**Where:** `crates/ironpad-cell/src/lib.rs:665` (also `crates/ironpad-cell/src/lib.rs:1031`, `crates/ironpad-app/src/components/live_view_panel.rs:195`, `crates/ironpad-app/src/components/live_view_panel.rs:272`, `public/executor-core.js:49`)

**Problem:** `From<LiveViewMeta>` maps LiveContent to "text"/"html"/"markdown" (665-669). `From<LiveContent> for LiveTickResult` maps the same variants to 0/1/2 (1031-1035). The app maps the numbers back to the same strings in two identical match blocks (live_view_panel.rs:195-200 and 272-277). The first mapping also borrows an owned `meta` and then `s.clone()`s the content and `to_string()`s the kind, copying the whole initial content for no reason.

**Verifier's correction:** Verified. From<LiveViewMeta> matches `&meta.initial_content` and clones the string (lib.rs:665-669) although `meta` is owned, which is a needless copy of the initial content. The app decodes kind codes with two identical match blocks (live_view_panel.rs:195-200 and 272-277). The proposed LiveKind type is over-built, though. The string mapping and the 0/1/2 mapping are two three-arm matches in one file, and a new public-ish type for them adds surface for little gain. The value is in removing the clone and folding the app's duplicate match.

**Fix:** lib.rs: in From<LiveViewMeta>, `match meta.initial_content { LiveContent::Text(s) => ("text", s), LiveContent::Html(s) => ("html", s), LiveContent::Markdown(s) => ("markdown", s) }`, then `kind: kind.into()`. That moves the content instead of cloning it. Leave the 0/1/2 match at 1031 as is, since it is already by value. live_view_panel.rs: add `fn live_kind_str(code: u32) -> &'static str { match code { 1 => "html", 2 => "markdown", _ => "text" } }`, keeping the comment that 0 and unknown codes mean plain text, and use it at both sites (195 and 272). Optionally add a doc line at LiveTickResult (lib.rs:1016-1020) naming live_kind_str as the decoder. No epoch bump.

**Tests:** Add a unit test in live_view_panel.rs for live_kind_str (0, 1, 2, and 99 maps to "text"). If cell-contract-1 lands, add a case there asserting `live_kind_str(LiveTickResult::from(LiveContent::X(..)).kind)` matches the string in `From<LiveViewMeta>`'s panel for each variant, freeing the content pointer. The existing lib.rs LiveTickResult tests (2802-2840) guard the cell side.

### cell-ffi-1: TickResult and LiveTickResult hand-roll the unsafe leak that vec_into_raw already provides

*dry · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-cell/src/lib.rs:963` (also `crates/ironpad-cell/src/lib.rs:1029`, `crates/ironpad-cell/src/lib.rs:1260`, `crates/ironpad-cell/src/lib.rs:1268`, `public/executor-core.js:885`)

**Problem:** The `into_boxed_slice` / `as_mut_ptr` / `mem::forget` sequence, whose `capacity == len` guarantee is what makes `ironpad_dealloc`'s `from_raw_parts(ptr, len, len)` sound, is written three times: `From<Canvas> for TickResult` (963-979), `From<LiveContent> for LiveTickResult` (1029-1046) and `vec_into_raw` (1260-1274). The comment at 1268 already notes the other two conversions as siblings that have to match. Soundness depends on keeping three unsafe blocks identical.

**Fix:** lib.rs: `impl From<canvas::Canvas> for TickResult { fn from(canvas) -> Self { let (width, height) = (canvas.width(), canvas.height()); let (rgb_ptr, rgb_len) = vec_into_raw(canvas.into_pixels()); TickResult { rgb_ptr, rgb_len, width, height } } }` and `let (content_ptr, content_len) = vec_into_raw(s.into_bytes());` in From<LiveContent>. Move vec_into_raw above both impls or leave it where it is, since order does not matter. Update its comment to say it is the one leak path for all three FFI result types. No CACHE_EPOCH bump.

**Tests:** The existing TickResult/LiveTickResult tests (lib.rs:2763-2840) cover non-empty payloads. Add `empty_live_content_is_null_zero` (LiveContent::Text(String::new()) gives a null pointer and 0) and the same for a 0x0 Canvas TickResult. This pins the new empty-case contract the JS readers already rely on.

### cell-plot-1: plot.rs repeats its constructor literal three times and the label/tooltip and caption blocks twice

*dry · partial · value 2 · effort S*

**Where:** `crates/ironpad-cell/src/plot.rs:101` (also `crates/ironpad-cell/src/plot.rs:116`, `crates/ironpad-cell/src/plot.rs:131`, `crates/ironpad-cell/src/plot.rs:246`, `crates/ironpad-cell/src/plot.rs:375`, `crates/ironpad-cell/src/plot.rs:287`, `crates/ironpad-cell/src/plot.rs:410`, `crates/ironpad-cell/src/plot.rs:294`, `crates/ironpad-cell/src/plot.rs:417`)

**Problem:** `Plot::line`, `bar` and `scatter` each write out the full default struct literal (800x400, no labels, flags off), so a new field or default has to be changed in three places. `render_line` and `render_scatter` differ only in the series they draw; the point-label block (246-256 vs 375-385) and the tooltip block (258-263 vs 387-392) are copied verbatim. The caption styling (287-292 vs 410-415) and label-area sizes (294-295 vs 417-418) are duplicated between `render_bar` and `build_chart_context`.

**Verifier's correction:** The constructor triplication (plot.rs:101-141) and the point-label and tooltip blocks in render_line vs render_scatter (246-263 vs 375-392) are verbatim copies, apart from an `expect` message. The caption block is duplicated between render_bar (287-292) and build_chart_context (410-415). The label-area claim is wrong: render_bar always sets both label areas (294-295), while build_chart_context sets them only when an axis label exists (417-420). Bars need a bottom area for their category labels, so these are deliberately different and must not be merged. The whole thing is cold code and low value.

**Fix:** plot.rs: add `fn with_kind(kind: ChartKind) -> Self { Self { kind, title: None, x_label: None, y_label: None, width: 800, height: 400, tooltips: false, point_labels: false } }`, and have line/bar/scatter call it. Add `fn apply_caption(&self, builder: &mut ChartBuilder<'_, '_, SVGBackend<'_>>)` holding the title block, and call it from render_bar and build_chart_context. Add `fn draw_point_extras(&self, chart: &mut ChartContext<'_, SVGBackend<'_>, Cartesian2d<RangedCoordf64, RangedCoordf64>>, data: &[(f64, f64)], tooltip_points: &mut Vec<(i32, i32, String)>)` holding the point_labels and tooltips blocks, called from render_line and render_scatter. Leave both label-area blocks as they are.

**Tests:** Existing plot.rs theming tests mostly check substrings. Before refactoring, add a golden test that renders line, scatter and bar with title, point_labels and tooltips all on and asserts the SVG string equals a captured literal (or compare to a pre-refactor snapshot). This makes the claim that the rendered SVG is unchanged a checked fact rather than an assumption.

### cli-2: IPC error codes are ad-hoc strings, and the CLI recovers exit codes by substring matching

*readability · partial · value 2 · effort S*

**Where:** `crates/ironpad-cli/src/main.rs:665` (also `crates/ironpad-cli/src/main.rs:668`, `crates/ironpad-cli/src/main.rs:672-675`, `crates/ironpad-cli/src/main.rs:694-697`, `crates/ironpad-cli/src/main.rs:711`, `crates/ironpad-cli/src/main.rs:717-718`, `crates/ironpad-cli/src/daemon.rs:480`, `crates/ironpad-cli/src/daemon.rs:541-555`, `crates/ironpad-cli/src/daemon.rs:773-776`, +3 more)

**Problem:** Codes come in three conventions: snake_case literals ("connection_error", "timeout"), a PascalCase literal ("CellNotFound"), and `format!("{code:?}")` of ErrorCode, which makes a Debug impl part of the wire. print_response then guesses: `c.contains("connect") || c.contains("disconnect")` (the second test is subsumed by the first), and it falls back to sniffing the human-readable message for "daemon"/"socket", because send_ipc's own failures (711, 717-718) are sent without any code. Changing an error message's wording can change the CLI's exit status.

**Verifier's correction:** The claim is accurate. Codes are "connection_error" and "timeout" (daemon.rs:776, 808, 822-823, and main.rs:694-697), "CellNotFound" (daemon.rs:480), and `format!("{code:?}")` of ErrorCode (997). print_response (main.rs:665-680) substring-matches "connect", where the "disconnect" test is subsumed by it, and falls back to sniffing the message for daemon or socket, because send_ipc's failures at 711 and 717-718 carry no code. The fix as written needs one correction: the code string is printed to stderr as `{"error": code}`, which agents parse, so the existing strings must stay byte-identical. The ErrorCode serde names equal its Debug names (PascalCase unit variants), so passing ErrorCode through serde changes nothing on the wire. The value is low: this is cleanup of fragile code, not a live bug.

**Fix:** In crates/ironpad-cli/src/ipc.rs add `#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)] pub enum IpcErrorCode { #[serde(rename = "connection_error")] ConnectionError, #[serde(rename = "timeout")] Timeout, #[serde(untagged)] Protocol(ironpad_common::protocol::ErrorCode) }` and change IpcResponse::code to Option<IpcErrorCode>, with error_with_code taking the enum. daemon.rs:480 becomes Protocol(ErrorCode::CellNotFound) (same string). 997 passes `Protocol(code)`. main.rs:711 and 717-718 use error_with_code(.., ConnectionError). print_response then does an exhaustive match: Protocol(VersionConflict) maps to VersionConflict, Protocol(PermissionDenied) to PermissionDenied, ConnectionError to ConnectionError, and everything else to GenericError. Delete the message sniffing. Note that timeout currently exits GenericError, and this keeps it that way.

**Tests:** ipc.rs error_with_code_round_trips guards serialization. Add a test that each IpcErrorCode serializes to exactly the strings shipped today ("connection_error", "timeout", "CellNotFound", "VersionConflict"). Add a unit test of a pure `exit_code_for(&IpcResponse) -> CliExitCode` extracted from print_response, covering the transport failures that used to rely on message sniffing.

## Wave 3: performance

Hot-path work first (per-autosave, per-keystroke, per-frame, per-SSR-request), then the paper cuts. server-fns-1 carries a verifier caution: step 1 (key fetches instead of a table scan) is safe now, the aggregate-view idea would create a single hot key under SurrealKV's optimistic concurrency and was dropped.

### storage-1: Every autosave deserializes the notebook's whole history ring just to read one timestamp

*performance · confirmed · value 4 · effort S*

**Where:** `public/storage.js:101` (also `public/storage.js:87-92`, `public/storage.js:113-115`, `public/storage.js:184`, `public/storage.js:204-208`, `crates/ironpad-app/src/pages/notebook_editor/state.rs:485`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:864`)

**Problem:** saveNotebook runs on the 1s typing debounce, and every call goes through writeHistorySnapshot -> historyEntries -> `index('notebookId').getAll(id)`. That pulls every snapshot record for the notebook out of IndexedDB, each one carrying a full notebook `json` string (up to HISTORY_CAP = 30), and then sorts them. All of that exists only to read `entries[0].savedAt`, and in the common case (still inside the 5-minute bucket) the function returns without writing anything. So a user who is typing pays for structured-clone reads of up to 30 full notebook copies about once per second. deleteNotebook has the same shape: it loads every snapshot and then deletes them one at a time with an await per key.

**Fix:** In public/storage.js add `function historyRange(id) { return IDBKeyRange.bound([id, -Infinity], [id, Infinity]); }` next to historyTx. Rewrite writeHistorySnapshot to use `const keys = await reqToPromise(historyTx(db, 'readonly').getAllKeys(historyRange(nb.id)));` (ascending, so the newest is `keys[keys.length - 1]`, and savedAt is `newest[1]`). Keep the bucket check, the same-millisecond nudge (`newestSavedAt >= now ? newestSavedAt + 1 : now`) and the put exactly as they are. Prune with `for (const key of keys.slice(0, Math.max(0, keys.length - (HISTORY_CAP - 1)))) await reqToPromise(store.delete(key));`, which keeps the newest CAP-1 old entries plus the new one, the same as `entries.slice(HISTORY_CAP - 1)` over the newest-first list. In deleteNotebook, replace the load-then-loop with `await reqToPromise(historyTx(db, 'readwrite').delete(historyRange(id)));`. historyEntries stays for listHistory only, and its doc comment should say so.

**Tests:** tests/e2e/local-history.spec.ts already covers the force snapshot (snapshotNow), newest-first listing, restore, and delete-wipes-history (the afterDelete === [] check). Nothing tests the cap or the bucket skip, so add both to local-history.spec.ts: (a) call IronpadStorage.snapshotNow(id) 32 times, then assert listHistory(id).length === 30 and that the newest entry's savedAt is the largest; (b) call saveNotebook twice back to back on a notebook that already has a snapshot and assert the history length does not change.

### server-fns-1: Every draft autosave scans and decodes the whole mutable_share table, twice

*performance · partial · value 3 · effort S*

**Where:** `crates/ironpad-app/src/db.rs:1224` (also `crates/ironpad-app/src/db.rs:1256-1260`, `crates/ironpad-app/src/db.rs:1182-1187`, `crates/ironpad-app/src/server_fns.rs:1324-1345`, `crates/ironpad-app/src/server_fns.rs:1492`, `crates/ironpad-app/src/server_fns.rs:1411`)

**Problem:** save_mutable_draft_core runs on the editor's 1.5s autosave debounce. It calls admit_mutable_write, which runs total_mutable_bytes (`SELECT math::sum(bytes + (draft_bytes ?? 0)) FROM mutable_share GROUP ALL`) and then total_mutable_bytes_for_user. SurrealKV stores each record as one serialized value, so summing two int fields decodes every row, including its notebook_json and draft_json strings (up to 4 MiB each, in a store capped at 512 MiB). The per-user total and list_shares_owned_by both filter with `WHERE record::id(id) IN $ids`. That wraps the key in a function, so no key lookup can serve the query and it scans the table as well. The result: each autosave costs O(bytes stored on the whole instance), and so does each signed-in home-page load. The cost grows with other users' data rather than with the request. It is small today and grows with adoption.

**Verifier's correction:** The core claim holds. save_mutable_draft_core (server_fns.rs:1466-1493) calls admit_mutable_write on every autosave. admit_mutable_write (1324-1345) runs total_mutable_bytes, an unfiltered `SELECT math::sum(...) FROM mutable_share GROUP ALL` (db.rs:1224-1241), and then total_mutable_bytes_for_user. Both total_mutable_bytes_for_user (db.rs:1260) and list_shares_owned_by (db.rs:1187) filter with `WHERE record::id(id) IN $ids`. Because the key sits inside a function, the planner cannot turn that into key fetches. Summing two int fields still decodes each whole document, including notebook_json/draft_json. Severity depends on scale: small now, but at the documented 512 MiB cap every 1.5s autosave would decode up to the whole store. Part (1) of the fix is cheap and correct. Part (2) needs changes. The `DEFINE TABLE ... AS SELECT math::sum ... GROUP ALL` view would put every write on the instance onto one aggregate record. Under SurrealKV's optimistic concurrency, which with_conflict_retry (db.rs:147-182) exists to handle, that turns every concurrent autosave on the instance into a conflict on one hot key. So drop that option. The slim per-share table works, but it is a dual-write denormalization across six writers, and nobody has measured the cost yet. CLAUDE.md's own method note says to measure before acting.

**Fix:** Step 1 (do now). In db.rs, add a private helper `fn share_record_ids(ids: Vec<String>) -> Vec<surrealdb::RecordId>` that maps each id to `RecordId::from(("mutable_share", id))` (adjust to the SDK 3.2.4 constructor). In total_mutable_bytes_for_user (db.rs:1256-1262), change the query to `SELECT math::sum(bytes + (draft_bytes ?? 0)) AS total FROM $rids GROUP ALL` bound with `("rids", share_record_ids(ids))`. In list_shares_owned_by (db.rs:1184-1190), change it to `SELECT record::id(id) AS id, notebook_json, draft_json, pushed_at, created_at FROM $rids`. Confirm with `EXPLAIN` that each is a key fetch rather than a table iterator, and confirm that a grant pointing at a deleted share still yields no row. Step 2 (defer until measured). Time total_mutable_bytes against a seeded store of about 100 MiB. Only if it is material, add a `share_size` table (id -> bytes, draft_bytes) written inside the same query/transaction as create_account_notebook, save_draft, promote_draft, discard_draft, unpublish_share and delete_mutable_share, and point total_mutable_bytes at it. Do not use a GROUP ALL materialized view: it creates a single hot record.

**Tests:** Existing guards: total_mutable_bytes_sums_notebook_json_and_drafts (db.rs:1621), share_lifecycle_with_owner, the per-user quota tests in server_fns.rs, and the owner-listing tests, including the both-slots-None warn-and-drop case. Add a db.rs test that creates shares for two users and asserts total_mutable_bytes_for_user and list_shares_owned_by return only user A's rows and bytes. Add a second test where A holds a grant to a deleted share id and assert that the row is skipped rather than erroring.

### cp-2: ViewOnlyNotebook clones the entire cell list once per cell (O(n^2)) on every SSR request

*performance · partial · value 4 · effort S*

**Where:** `crates/ironpad-app/src/components/view_only_notebook.rs:489` (also `crates/ironpad-app/src/components/view_only_notebook.rs:461`, `crates/ironpad-app/src/components/view_only_notebook.rs:492`, `crates/ironpad-app/src/components/view_only_notebook.rs:493`, `crates/ironpad-app/src/components/view_only_notebook.rs:494`, `crates/ironpad-app/src/components/view_only_notebook.rs:704`, `crates/ironpad-app/src/components/view_only_notebook.rs:729`, `crates/ironpad-app/src/components/view_only_notebook.rs:794`, `crates/ironpad-app/src/components/view_only_notebook.rs:800`, +2 more)

**Problem:** The cell loop clones `nb.cells` once (:461). Then, for EVERY cell, it clones the whole vector again (`all_cells = cells.clone()`, :492), along with the shared Cargo.toml and the full effective shared source (:493-494). Each `IronpadCell` carries its source plus a `saved_output` of up to 256 KiB, so a 40-cell public notebook with snapshots allocates dozens of MB per render. This body runs on every SSR request for /public, /shared, /mutable and all /embed routes (SsrMode::Async), and again on hydrate. Markdown, shared and Linux cells receive the copy and drop it. More full copies follow on the client: `all_cells.get_value()` per run (:794), per live output render (:1113, then captured by the WidgetSink derive at :1318), and `cell_outputs.get_untracked()` clones the whole outputs map, piping bytes included, per click and per run (:729, :800).

**Verifier's correction:** The O(n^2) is real: view_only_notebook.rs:461 clones `nb.cells`, then :492 clones the full Vec again for every cell, along with the shared Cargo.toml and effective shared source (:493-494). ViewOnlyCell hands it to Markdown/Shared/Linux arms that never read it (:628-660). The client-side full copies are also real: `all_cells.get_value()` at :794 and :1113, the WidgetSink derive at :1318, and `cell_outputs.get_untracked()` at :729 and :800. The severity is overstated, though. The worst public notebook is cannon.ironpad (28 cells, 121 KB of JSON), which is about 3.4 MB of copies per render, not 'dozens of MB'. Only /shared and /mutable notebooks near the 256 KiB-per-cell snapshot cap would reach that range. It is still an easy, clear win on the SSR path of every notebook route.

**Fix:** In `ViewOnlyNotebook`, before the `view!` block, build the notebook-wide data once: `let all_cells = StoredValue::new(notebook.with_value(|nb| nb.cells.clone()));`, `let shared_cargo = StoredValue::new(notebook.with_value(|nb| nb.shared_cargo_toml.clone()));` and `let shared_src = StoredValue::new(notebook.with_value(IronpadNotebook::effective_shared_source));`. Change the `all_cells`, `shared_cargo_toml` and `shared_source` props of `ViewOnlyCell`, `ViewOnlyCodeCell` (and the Linux cell's shared props) to those `StoredValue` types, and delete the per-cell `StoredValue::new(...)` wrappers at :704-706. In the loop at :461/:490, iterate `all_cells.with_value(|cells| cells.iter().zip(frame_indices)...)` and clone only the single `cell`. On the client: `run_cell` (:729) becomes `cell_outputs.with_untracked(|outputs| all_cells.with_value(|cells| unexecuted_dependencies(cells, &cid, outputs, ..)))`. The run flow (:794-800) becomes `let (input_buf, types, my_idx) = all_cells.with_value(|cells| cell_outputs.with_untracked(|o| { let i = ...; let (b, t) = assemble_cell_inputs(cells, i, o); (b, t, i) }))`. For `ViewOnlyOutput`, pass a notebook-level `StoredValue<Vec<(String, bool)>>` of `(id, is_runnable)`, computed once, and make `WidgetSink.cells` `Signal::derive(move || runnable.get_value())`. The `menu` removal (cp-12) touches the same component, so land them together.

**Tests:** The change should not alter behavior. Guards: public-notebooks.spec.ts (autorun + piping), dependency-cascade.spec.ts (the `unexecuted_dependencies` path from a view-only Run click), persisted-outputs.spec.ts, embed.spec.ts, and the widget-driven reruns in notebook-smoke.spec.ts. Add a unit test for the pure `(id, is_runnable)` projection only if it is extracted as a free fn; otherwise the existing e2e are the gate.

### storage-2: Every local blob-cache hit rewrites the whole WASM record to bump lastUsed

*performance · confirmed · value 3 · effort S*

**Where:** `public/storage.js:324` (also `public/storage.js:320-323`, `crates/ironpad-app/src/blob_cache.rs:133`, `crates/ironpad-app/src/components/run_flow.rs:109`)

**Problem:** getBlob opens a readwrite transaction and `put`s the full record back (wasm Uint8Array of hundreds of KB to MB, glue text, diagnostics) only to update `lastUsed`. run_flow probes the local store on every non-forced cell run, so every hit (Run All, reruns, reactive reruns) reads the blob and then writes all of it back to disk. The only purpose of that write is LRU ordering over a 64-entry cache.

**Fix:** Add `const BLOB_TOUCH_INTERVAL_MS = 60 * 1000;` next to MAX_BLOB_ENTRIES, with a why-comment: minute-level recency is enough to order 64 entries, and every touch rewrites megabytes. In getBlob, keep the single readwrite transaction, not two. Doing get and put in one transaction is what stops a stale record from overwriting a Force Recompile putBlob that lands in between, so the reviewer's readonly suggestion should be dropped. Only skip the put: `const now = Date.now(); if (!(now - (record.lastUsed || 0) < BLOB_TOUCH_INTERVAL_MS)) { record.lastUsed = now; await reqToPromise(store.put(record)); }`. Update the doc comment from 'touching its LRU timestamp on hit' to 'at most once per BLOB_TOUCH_INTERVAL_MS'.

**Tests:** tests/e2e/blob-cache.spec.ts covers hits (the editor's second run skips the compile, and Force Recompile then a local hit). Add a spec there: putBlob a record, use raw indexedDB in page.evaluate to rewrite its lastUsed to Date.now() - 120000, call getBlob, and assert lastUsed moved forward. Then call getBlob again at once and assert lastUsed did not change.

### compiler-3: Blocking std::fs on async request paths, including the compile cache-hit hot path

*performance · partial · value 3 · effort M*

**Where:** `crates/ironpad-app/src/compiler/cache.rs:86` (also `crates/ironpad-app/src/compiler/cache.rs:152`, `crates/ironpad-app/src/server_fns.rs:203`, `crates/ironpad-app/src/server_fns.rs:307`, `crates/ironpad-app/src/server_fns.rs:1117`, `crates/ironpad-app/src/compiler/scaffold.rs:56`, `crates/ironpad-app/src/compiler/build.rs:365`, `crates/ironpad-app/src/compiler/build.rs:402`, `crates/ironpad-app/src/compiler/build.rs:567`)

**Problem:** `try_cache_hit` reads the whole .wasm blob, the glue and the diagnostics with `std::fs` directly inside async `compile_cell_core`. The code's own comment at server_fns.rs:180 calls this path 'the common case, a hot path'. `store_blob` writes synchronously the same way. `scaffold_micro_crate` (canonicalize, create_dir_all, 2-3 writes, remove_file) runs synchronously inside async `check_cell_core` on every check-on-type debounce. Immediately after it, check_micro_crate switches to tokio::fs with the comment 'Async fs ... this is the latency-sensitive check-on-type route, the one that most needs to not block a worker', so the invariant it states does not hold one call earlier. `build_micro_crate` also mixes in `std::fs::create_dir_all` and `std::fs::read_to_string`. The share snapshot calls `try_cache_hit` per cell only to copy files, so it buffers each whole blob in memory and parses a diagnostics JSON it never uses. CLAUDE.md pitfall #4 forbids this pattern.

**Verifier's correction:** True as stated. try_cache_hit (cache.rs:86-126) does std::fs::read of the whole blob, glue and diag inside async compile_cell_core at server_fns.rs:203, the path the code itself calls a hot path. store_blob (cache.rs:152) writes synchronously at server_fns.rs:307. scaffold_micro_crate (scaffold.rs:56-150, create_dir_all, canonicalize, 2-4 writes, remove_file) runs synchronously in check_cell_core at 508, right before check_micro_crate's comment (build.rs:569-570) says this route 'most needs to not block a worker'. build.rs:365/402 use std::fs in async. The share snapshot (server_fns.rs:1117) loads the whole blob and parses diagnostics it never reads. Two corrections. First, converting cache.rs to async ripples into ~20 sync unit tests. Second, converting scaffold_micro_crate to async ripples into ~50 test call sites. The lower-churn fix is async cache helpers plus spawn_blocking for the scaffold. The build.rs pair sits on the cold build path, so fix it only because it is trivial.

**Fix:** 1. cache.rs: make `try_cache_hit` and `store_blob` `async fn` on tokio::fs (read/read_to_string/create_dir_all). Make `atomic_write` async as well; see compiler-9. Callers at server_fns.rs:203 and 307 add `.await`. Test callers in cache.rs, mod.rs:546-587/1497-1574 and server_fns tests add `.await`, and the cache.rs tests become #[tokio::test]. 2. Scaffold: in the `scaffold_request` helper from compiler-2, run scaffold_micro_crate under `tokio::task::spawn_blocking`, moving in owned clones (config.cache_dir, config.ironpad_cell_path, cell_id, source, cargo_toml, previous_cell_types, shared_*). A per-check String copy is cheaper than parking a worker on fs syscalls. Map JoinError to ServerFnError. Do NOT use block_in_place: the server_fns tests run on current-thread runtimes, where it panics. 3. build.rs:365 becomes `tokio::fs::create_dir_all(..).await?` and 402 becomes `tokio::fs::read_to_string(..).await.context(..)?`. 4. write_cell_blobs_capped (server_fns.rs:1096-1128): replace try_cache_hit with `tokio::fs::try_exists(cache_blob_path(cache_dir,&hash))`. Copy the .wasm, and the .js when cache_js_glue_path exists (that is what sets `has_js_glue`), with `tokio::fs::copy` into a uuid temp sibling followed by rename (the atomic_copy variant of the shared helper). Pass `cell.cargo_toml.as_deref().unwrap_or("")` instead of cloning.

**Tests:** Existing: cache.rs store/hit/round-trip/diagnostics/no-temp-files tests (535-730) become #[tokio::test]; mod.rs cache round-trip tests at 546-587 and 1497-1574; server_fns share snapshot tests (seed_cache users, the Linux-target snapshot test at ~2288, cap tests). Add a snapshot test asserting `has_js_glue` is false for a blob seeded without glue and true with it, since the copy path now derives it from file existence instead of a read.

### server-fns-11: Each Push or Share re-reads and re-writes every warm blob even when its content-addressed copy already exists

Merged into **compiler-3** (the same finding, seen from the server-fns-and-db partition).

### server-fns-6: Admin overview and tier wipe walk and delete multi-GB trees synchronously on a tokio worker; the walker is also duplicated

*performance · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/server_fns.rs:2081` (also `crates/ironpad-app/src/server_fns.rs:2096`, `crates/ironpad-app/src/server_fns.rs:2187`, `crates/ironpad-app/src/server_fns.rs:2210-2222`, `crates/ironpad-app/src/cache_tiers.rs:25-40`, `crates/ironpad-app/src/cache_tiers.rs:47-61`, `crates/ironpad-server/src/main.rs:127`)

**Problem:** Inside an async server fn, admin_overview walks every cache tier with synchronous std::fs recursion (tier_bytes). That includes `targets`: 3.2 GB of cargo target dirs with tens of thousands of entries. admin_wipe_cache_tier runs the same walk and then remove_dir_all on that tree. Both hold a tokio worker for seconds on a small Fly machine, stalling unrelated compiles and autosaves scheduled on it (CLAUDE.md pitfall 4). Two DRY problems ride along. dir_bytes (2210-2222) is a verbatim copy of tier_bytes's inner walk (cache_tiers.rs:26-38). And the "ironpad.db" filename is spelled out independently here and where main.rs opens the database, so renaming it would make the panel silently report 0 bytes.

**Fix:** In cache_tiers.rs, promote the inner walker to `#[must_use] pub fn dir_bytes(dir: &Path) -> u64`, make tier_bytes `dir_bytes(&tier.path(cache_dir))`, and delete server_fns.rs's dir_bytes (2209-2222). In db.rs, add `pub const DB_FILE: &str = "ironpad.db";` with a doc line, and use `config.data_dir.join(crate::db::DB_FILE)` at main.rs:127 and in admin_overview. In admin_overview, wrap the tier map and the database_bytes computation in `tokio::task::spawn_blocking(move || { ... })` with cloned cache_dir/data_dir, and map a JoinError to ServerFnError. In admin_wipe_cache_tier, do the same with `spawn_blocking(move || crate::cache_tiers::clear_tier(&cache_dir, tier))`. Leave the boot valve's synchronous call alone, because it runs before the listener binds.

**Tests:** Existing: the cache_tiers.rs tests (seed + tier_bytes/clear_tier) and admin_fns_are_all_gated, which scans server_fns.rs and is unaffected. Add a cache_tiers.rs unit test for dir_bytes over a nested tree, and one asserting it returns 0 for a missing path. spawn_blocking itself needs no dedicated test. The admin e2e and panel specs cover the wire behavior.

### server-3: Relay copies every forwarded frame's payload even though the inbound Utf8Bytes can be shared

*performance · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-server/src/state.rs:296` (also `crates/ironpad-server/src/state.rs:349`, `crates/ironpad-server/src/state.rs:367`, `crates/ironpad-server/src/state.rs:384`, `crates/ironpad-server/src/ws.rs:179`, `crates/ironpad-server/src/ws.rs:243`, `crates/ironpad-server/src/ws.rs:493`, `crates/ironpad-server/src/ws.rs:521`)

**Problem:** Each frame arrives as `Message::Text(text)`, where `text` is an axum `Utf8Bytes`: a reference-counted buffer that is free to clone. The read loops pass it on as `&str` (ws.rs:179 and 493), and every WsState send then rebuilds it with `Utf8Bytes::from(message)`. For a `&str`, that call allocates and copies the whole payload (axum 0.8.9 `From<&str>`). This runs on every relayed message: host events, which carry full cell source, and guest mutations and queries. broadcast_to_guests' own doc (state.rs:345-347) already treats a per-payload copy here as a real hot-path cost. It removed the per-recipient copy but kept this one. The same happens with `wire_msg`: it returns an owned String, but callers pass `&String`, which copies again instead of moving the buffer in with `From<String>`.

**Fix:** state.rs: change `send_to_host`, `send_to_guest`, `broadcast_to_guests` and `broadcast_to_notebook_guests` to take `message: impl Into<Utf8Bytes>` and convert once with `let payload: Utf8Bytes = message.into();`. In send_to_host, convert after the host lookup so a Disconnected result stays allocation-free. ws.rs: change `handle_host_message(text: &Utf8Bytes, ...)` and `handle_guest_message(text: &Utf8Bytes, ...)`, parse with `serde_json::from_str(text.as_str())`, and forward with `text.clone()` (a refcount bump) at the broadcast_to_notebook_guests, send_to_guest (ack) and send_to_guest (response) sites and at the send_to_host(guest mutation) site. The read loops pass `&text`. Pass wire_msg results by value (`state.ws.send_to_host(notebook_id, response)`, not `&response`) at all wire_msg call sites. The &str literal call sites in the state.rs tests keep compiling through `Into`. The ws.rs tests that call handle_*_message need `&Utf8Bytes::from(json)` (about 24 sites), or add a tiny test-only wrapper.

**Tests:** The state.rs send/broadcast tests (597-873) and the ws.rs handler tests guard routing semantics; tests/relay.rs covers end-to-end forwarding. This is not a behavioral change, so no new behavioral test is required. Optionally add a state.rs unit test that broadcasts a `Utf8Bytes` to two guests and asserts both receive `as_ptr()`-equal payloads, which locks in the zero-copy fan-out.

### editor-2: Per-cell effects clone page-level maps (including every cell's output bytes) on each change

*performance · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:649` (also `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:650`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:387`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:439`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:1145-1148`, `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs:137`, `crates/ironpad-app/src/components/output_render.rs:206`)

**Problem:** Every CellItem's autocomplete effect runs `state.cell_outputs.get()`, which deep-clones the whole `HashMap<String, CellOutputData>` including all output bytes, plus `state.cells.get_untracked()`. It then rebuilds the Monaco JS context, although it only needs type tags. It re-runs on every `cell_outputs` write: twice per cell run, and on every widget value change, because `update_cell_output` rewrites bytes on each slider input event. That makes it n cells x full map clone per drag tick, and O(n^2) byte copies across a Run All. The same `.get()`-to-clone pattern appears in the per-cell queue watcher (clones the queue on each advance), the blocked watcher (clones the blame map), and the stale header closure (clones the stale map on every edit).

**Fix:** Add `pub(super) type_tags: Memo<HashMap<String, String>>` to NotebookState. Build it in mod.rs next to the other state fields (~line 150): `Memo::new(move |_| cell_outputs.with(|m| m.iter().filter_map(|(k, d)| d.type_tag.clone().map(|t| (k.clone(), t))).collect()))`. The autocomplete effect reads `state.type_tags.with(|tags| ...)` and `state.cells.with_untracked(|cells| ...)`. Memo equality then stops byte-only widget updates from re-running n effects. Queue watcher: `let my_pos = state.run_all_queue.with(|q| q.iter().position(|id| *id == cid));` computed OUTSIDE the match. It must not stay borrowed, because the `Some(0)` arm calls `advance_queue`, which writes run_all_queue. Blocked watcher: `state.cell_blocked_by.with(|b| b.contains_key(&cid))`. Stale header: `state.cell_stale.with(|m| m.get(&id).copied().unwrap_or(false))`.

**Tests:** This is a refactor with no behaviour change. Guarded by the existing live-check.spec.ts and execution.spec.ts, plus dependency-cascade.spec.ts for the queue and blocked watchers. Add a unit test only if the tag projection is extracted as a pure fn (e.g. `type_tags_of(&HashMap<..>)` in output_render.rs), asserting untagged entries are skipped.

### editor-5: Whole-notebook and version-map deep clones on the mutation and persist path

*performance · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/model.rs:212` (also `crates/ironpad-app/src/model.rs:256`, `crates/ironpad-app/src/model.rs:285`, `crates/ironpad-app/src/model.rs:306`, `crates/ironpad-app/src/model.rs:185`, `crates/ironpad-app/src/pages/notebook_editor/state.rs:462`, `crates/ironpad-app/src/pages/notebook_editor/state.rs:466-468`, `crates/ironpad-app/src/pages/notebook_editor/state.rs:573`, `crates/ironpad-app/src/pages/notebook_editor/mod.rs:236`)

**Problem:** `cell_version` clones the whole `HashMap<String,u64>` to read one entry. It runs on every CellUpdate, i.e. once per cell per flush, so O(n^2). `sync_from_notebook` deep-clones the entire IronpadNotebook (all sources plus saved_output snapshots of up to 256 KiB per cell) just to project manifests, and it runs on every add/delete/reorder/label/collapse change. `mark_downstream_stale`/`mark_all_code_cells_stale` clone the manifest list per content edit, and `Query::CellGet` clones the notebook to return one cell. On the persist side, `persist_notebook_durable` clones the notebook and then clones it again to write back `updated_at`, `save_draft_now` clones it a third time in ServerDraft mode, and the layout-sync Effect (mod.rs:236) uses tracked `.get()`, another full clone per structural mutation. One 'add cell' on a forked public notebook copies the whole multi-MB document about four times.

**Fix:** model.rs: `cell_version` becomes `self.cell_versions.with_untracked(|v| v.get(cell_id).copied()).unwrap_or(0)`. `sync_from_notebook`: wrap the body in `self.notebook.with_untracked(|nb_opt| { let Some(nb) = nb_opt else { return }; ... })`; setting `cells` and updating `cell_versions` inside is fine because they are different signals. CellGet: `self.notebook.with_untracked(|nb| nb.as_ref().map(|nb| nb.cells.iter().find(|c| c.id == cell_id).cloned()))`, mapping the two None cases to the existing errors. `mark_downstream_stale`/`mark_all_code_cells_stale`: `self.cells.with_untracked(|cells| self.cell_stale.update(...))`. state.rs `persist_notebook_durable`: first `state.notebook.try_update_untracked(|nb| if let Some(nb) = nb { nb.updated_at = chrono::Utc::now(); })?`-style (return false on None), then ONE `try_get_untracked()` for the IndexedDB write, and drop the write-back clone. `save_draft_now`: when `!enrich_outputs`, serialize inside `state.notebook.try_with_untracked(|nb| nb.as_ref().map(serde_json::to_string))`. mod.rs:236: `state.notebook.with(|nb| if let Some(nb) = nb { ... })`.

**Tests:** Guarded by the model.rs tests (`cell_update_persists_collapse_defaults`, `cell_delete_unknown_id_errors_instead_of_phantom_success`, stale_tests) and by persistence e2e (notebook.spec.ts, account-notebooks.spec.ts for the draft path). Add a model unit test that `query(CellGet)` returns the right cell and CellNotFound for an unknown id, if one does not already exist.

### editor-8: last_compile keeps a full CompileResponse (wasm blob) per cell; panels clone blobs and output bytes on render

*performance · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/pages/notebook_editor/pipeline.rs:380` (also `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs:38`, `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs:53`, `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs:153`, `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs:167`, `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs:205`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:752-765`)

**Problem:** `last_compile.set(Some(response.clone()))` copies the whole wasm blob (often 1 MB or more for rayon/simd cells) on every successful run and keeps it resident per cell for the page's lifetime. The only readers need diagnostics, `wasm_blob.len()` and `cached`: the marker effect and CompileResultPanel. CompileResultPanel then clones the response again via `last_compile.get()` on each render, and clones ExecutionResult just to read `execution_time_ms`. CellOutputPanel clones ExecutionResult and then `output_bytes` twice more (lines 167, 205) on each render, including every collapse toggle.

**Fix:** In pipeline.rs define `#[derive(Clone)] pub(super) struct CompileSummary { pub diagnostics: Vec<Diagnostic>, pub blob_len: usize, pub cached: bool }` with `impl From<&CompileResponse>`. Change `last_compile: RwSignal<Option<CompileSummary>>` in cell_item.rs:83, CellRunCtx (pipeline.rs:44) and the CompileResultPanel prop (cell_output.rs:25). At 380 set `Some((&response).into())` without cloning the response. At 512 and 533 set `Some((&response).into())`. At 552 build the summary directly. Marker effect: `last_compile.try_with(|c| c.as_ref().map(|s| diagnostics_to_markers(&s.diagnostics)))`. CompileResultPanel: read `blob_len`, `cached` and the warnings through `last_compile.with(...)`, and get `execution_result.with(|r| r.as_ref().map(|r| r.execution_time_ms))`. CellOutputPanel: take the scalars and parse panels inside `execution_result.with`, keep only one owned `output_bytes` for the hex dump, and pass `&[u8]` if `format_hex_dump` accepts a slice.

**Tests:** Guarded by execution.spec.ts and live-check.spec.ts (the 'Compiled (… KB' summary and inline markers). Add a pipeline.rs unit test: `CompileSummary::from(&resp)` preserves diagnostics, `blob_len == resp.wasm_blob.len()`, and cached.

### editor-4: Run pipeline and live check clone the outputs map and manifest list per dispatch; session events built eagerly

*performance · partial · value 2 · effort S*

**Where:** `crates/ironpad-app/src/pages/notebook_editor/pipeline.rs:204` (also `crates/ironpad-app/src/pages/notebook_editor/pipeline.rs:203`, `crates/ironpad-app/src/pages/notebook_editor/pipeline.rs:233-236`, `crates/ironpad-app/src/pages/notebook_editor/pipeline.rs:252`, `crates/ironpad-app/src/pages/notebook_editor/pipeline.rs:754-756`, `crates/ironpad-app/src/pages/notebook_editor/pipeline.rs:345-349`, `crates/ironpad-app/src/pages/notebook_editor/pipeline.rs:416-424`, `crates/ironpad-app/src/pages/notebook_editor/state.rs:361-381`)

**Problem:** `wire_run_effect` deep-clones `state.cells` three times and `state.cell_outputs` (all output bytes) twice per run. It also builds `current_cell_sources()`, a full copy of every cell's source. `dispatch_live_check` repeats the cells + outputs clone on every 1s typing debounce, only to compute `previous_cell_types` from borrowed data. The helpers it calls (`assemble_cell_inputs`, `previous_cell_types`, `unexecuted_dependencies`) already take `&[C]`/`&HashMap`. `emit_session_event` receives a fully built Event, so `response.diagnostics.clone()` and `display_text.clone()` run on every compile/exec even with no session, and display_text can be hundreds of KB of panel JSON.

**Verifier's correction:** The facts are right. wire_run_effect clones `state.cells` three times (pipeline.rs:203, 233, 252) and `cell_outputs` twice (204, 235), and dispatch_live_check clones both again (754-756) for helpers that already take `&[C]`/`&HashMap`. The run path, though, is dominated by a network compile and a wasm instantiate, so its clones are not the hot part. The one that recurs is the live-check clone on every 1s typing debounce. The lazy-event change saves one display_text clone per exec when no session is active: real, but small next to execution. Value is lower than 'medium'.

**Fix:** Do the borrow changes. Cascade block: `state.cells.with_untracked(|cells| state.cell_outputs.with_untracked(|outputs| { let unexecuted = unexecuted_dependencies(cells, &cid, outputs, |id| sources.get(id).cloned()); ... }))`, returning the new queue (if any) and calling `run_all_queue.set` OUTSIDE the borrows. Input assembly (233-236) and live check (754-756): the same nested `with_untracked`, with the `position` lookup inside. Downstream-ids (252): `state.cells.with_untracked(|c| downstream_code_ids(c, &cid))`. Changing `CellRunCtx::emit_session_event` to take `impl FnOnce() -> Event` is optional. If done, update the local closure at pipeline.rs:162 and the `fail_and_report` caller.

**Tests:** No behaviour change. Guarded by dependency-cascade.spec.ts, execution.spec.ts, live-check.spec.ts and session.spec.ts (cells run events). No new test is required.

### cp-3: Per-frame `new Function(...)` and a JS->wasm->JS copy of every simulation frame

*performance · partial · value 3 · effort S*

**Where:** `crates/ironpad-app/src/components/animation_canvas.rs:131` (also `crates/ironpad-app/src/components/animation_canvas.rs:281`, `crates/ironpad-app/src/components/animation_canvas.rs:62`, `crates/ironpad-app/src/components/animation_canvas.rs:74`, `crates/ironpad-app/src/components/animation_canvas.rs:75`, `crates/ironpad-app/src/components/animation_canvas.rs:487`, `crates/ironpad-app/src/components/animation_canvas.rs:557`, `crates/ironpad-app/src/components/executor.rs:248`, `crates/ironpad-app/src/components/blob_url.rs:423`, +1 more)

**Problem:** `draw_js_frame_to_canvas` calls `js_sys::Function::new_with_args`, i.e. `new Function(src)`, every time it draws. AnimationCanvas calls it once per animation frame (:281), up to 60 times a second. `draw_rgb_to_canvas` does the same on every simulation tick (:487, :557). Each tick also moves the frame twice. `tick_cell` copies the JS `Uint8Array` into wasm with `.to_vec()` (executor.rs:248), and `draw_rgb_to_canvas` copies it straight back with `Uint8Array::from(rgb)`. The doc comment at :62-63 says that view is 'zero-copy into JS', which is false; linux_cell.rs:945 correctly notes that `Uint8Array::from` copies. The same runtime-compilation pattern appears per image panel (blob_url.rs:423) and per read-only editor mount (`Function::new_no_args("")`, monaco_editor.rs:257). The file already has the right tool: the `#[wasm_bindgen(inline_js)]` sim-bus shim at :151-165 is compiled once.

**Verifier's correction:** The two per-frame paths are confirmed. `draw_js_frame_to_canvas` calls `Function::new_with_args` (animation_canvas.rs:131) and is called from the rAF callback at :281, up to 60 times a second. `draw_rgb_to_canvas` (:75) runs on every simulation tick (:487, :557). `tick_cell` copies the frame into wasm with `Uint8Array::new(&rgb_val).to_vec()` (executor.rs:248), and `Uint8Array::from(rgb)` (:74) copies it back out. The ':62-63 zero-copy' doc is false. That is a genuine hot path. The rest of the scope is wrong: blob_url's `Function` is at blob_url.rs:17, not :423, and runs once per image panel. monaco_editor.rs:257 runs once per editor mount. Both are cold, and the Minimal Changes rule says leave them. One constraint the finding missed: js-sys is an optional, hydrate-only dependency (Cargo.toml:21,48), while `TickResult` is defined ungated, so a `js_sys::Uint8Array` field needs cfg gating.

**Fix:** 1. In animation_canvas.rs, add a hydrate-gated `mod draw` with one `#[wasm_bindgen(inline_js = "export function draw_rgba(ctx,a,w,h){ctx.putImageData(new ImageData(a,w,h),0,0)} export function draw_rgb(ctx,rgb,w,h){...current body...} export function draw_b64_rgb(ctx,b64,w,h){...} export function decode_frames(b64,fsz,fc){...}")] extern "C"` block. Declare the signatures with `&web_sys::CanvasRenderingContext2d`, `&JsValue`/`&js_sys::Uint8Array`, `u32`. Replace the four `Function::new_with_args` helpers with thin calls into it, or delete them. `draw_b64_rgb`/`decode_frames` are once-per-mount and could stay, but moving them into the same shim is free.
2. In executor.rs, gate the struct: make `TickResult.rgb_bytes` a `js_sys::Uint8Array` under `#[cfg(feature = "hydrate")]`, built as `js_sys::Uint8Array::new(&rgb_val)` without `.to_vec()`. The non-hydrate stub returns `Err`, so gate `TickResult` itself as hydrate-only; its only consumers (animation_canvas.rs:487, :557) are already inside hydrate blocks. Pass it straight to `draw::draw_rgb`.
3. Delete the false 'zero-copy' sentence.
Leave blob_url.rs and monaco_editor.rs alone.

**Tests:** notebook-smoke.spec.ts covers the Game of Life Simulation canvas, and persisted-outputs.spec.ts covers Animation replay and Snapshot first frames. Extend the smoke spec so it reads back a non-blank canvas pixel via `getImageData` after two ticks, which guards the new draw shim, and so it clicks the Step button and asserts the frame counter increments. `cargo make clippy` on the hydrate target catches the cfg gating.

### cp-4: Home page clones every full local notebook on every search keystroke

*performance · partial · value 3 · effort S*

**Where:** `crates/ironpad-app/src/pages/home_page.rs:433` (also `crates/ironpad-app/src/pages/home_page.rs:226`, `crates/ironpad-app/src/pages/home_page.rs:232`, `crates/ironpad-app/src/pages/home_page.rs:434`, `crates/ironpad-app/src/pages/home_page.rs:161`, `crates/ironpad-app/src/pages/home_page.rs:34`, `crates/ironpad-app/src/pages/home_page.rs:53`, `crates/ironpad-app/src/pages/home_page.rs:505`, `crates/ironpad-app/src/pages/home_page.rs:746`)

**Problem:** `private_notebooks` holds the complete `IronpadNotebook`s that `list_notebooks` returns: every cell's source, Cargo.toml and saved output. The home page needs only id, title, cell count, updated_at and tags. `filtered_items` runs on every `on:input` keystroke and calls `private_notebooks.get()` (:433) and `mutable_entries.get()` (:434), which deep-clone both vectors before `collect_items` clones the display fields out again. A user with many notebooks pays a full copy of their local corpus per character typed. The account side already avoids this with a `MutableEntry` summary (:53). `NotebookListItem::Mutable` (:34) then re-declares `MutableEntry`'s fields one by one.

**Verifier's correction:** Confirmed that `filtered_items` (home_page.rs:430-438) calls `private_notebooks.get()` and `mutable_entries.get()`, deep-cloning every local `IronpadNotebook` (cells, sources, saved outputs) on each search keystroke and chip change. `local_notebook_matches` (:136) and `collect_items` (:161-166) read only id, title, tags, cells.len() and updated_at. However, `.with()` is enough to kill the per-keystroke copy, and it is a two-line change. The `LocalEntry` projection and the enum restructure add a type plus four projection sites for a residual memory saving, which Minimal Changes argues against. IndexedDB `list_notebooks` returns full notebooks regardless.

**Fix:** In `NotebookGrid` (home_page.rs:430-438), replace the body with `private_notebooks.with(|local| mutable_entries.with(|account| collect_items(local, account, &public_notebooks, &query, filter_mode.get())))`. Nothing else is required. Skip `LocalEntry` and the `NotebookListItem` collapse unless memory becomes a problem.

**Tests:** The existing `#[cfg(test)]` module in home_page.rs (:785) covers `collect_items`, whose signature is unchanged, and home.spec.ts covers search and chips e2e. No new test is needed, because this is a pure borrow-versus-clone change with no behavior difference.

### cell-inputs-1: CellInputs::from_raw copies every upstream output, including slots the cell never binds

*performance · partial · value 3 · effort S*

**Where:** `crates/ironpad-cell/src/lib.rs:190` (also `crates/ironpad-cell/src/lib.rs:151`, `crates/ironpad-app/src/compiler/scaffold.rs:630`, `public/executor-core.js:629`)

**Problem:** On every cell call, `from_raw` does `data.push(segment.to_vec())` for each preceding cell's output. The scaffold binds only the slots the cell references, but every segment is still copied: a large upstream Canvas or `Vec<f64>` is duplicated in each downstream cell, and each copy is a separate allocation. None of this is needed. The input buffer stays allocated for the whole call, including across the await in async cells, because the host frees it only in `_readCellResult` after `cell_main` resolves (executor-core.js:629). The scaffold also builds `CellInputs` straight from that slice (scaffold.rs:630).

**Verifier's correction:** The core claim holds. from_raw does `data.push(segment.to_vec())` for every segment (lib.rs:190), so every upstream output is copied even though the scaffold binds only referenced slots (scaffold.rs:630-642). The copy is pure waste because every binding deserializes into an owned T immediately, and the host keeps the input buffer alive until _readCellResult after cell_main resolves (executor-core.js:627-628, freed there). A borrowing CellInputs<'a> is sound: the scaffold's slice comes from from_raw_parts with an unbounded lifetime, and all deserialization happens synchronously at the top of the inner fn. It runs once per cell execution, not per tick, so this is a memory-peak and copy paper cut on large upstream outputs (a Canvas or Animation bytes), not a hot loop. Two corrections to the fix. First, a CACHE_EPOCH bump is not needed: behavior is identical, and old blobs simply keep the old copy until the next bump for another reason. Bumping only for this would cold-compile the whole prod cache for a perf gain. Second, adding a lifetime to a public prelude type is a real API change for any user code that names `CellInputs` in a signature. No public notebook does (grep shows 0 hits), but the completions index lists it (scaffold.rs:1267), so gen-completions must be rerun.

**Fix:** lib.rs: change to `pub struct CellInputs<'a> { data: Vec<&'a [u8]> }` and `impl<'a> CellInputs<'a> { pub fn from_raw(bytes: &'a [u8]) -> Self`, pushing `segment` in place of `segment.to_vec()`. `get`/`last` return `CellInput<'a>` via `CellInput::new(bytes)` from the stored slice. `serialize` stays an associated fn. Update the struct doc to say it borrows the host's input buffer, which the executor frees only after cell_main resolves. The scaffold string at scaffold.rs:630 is unchanged. Run `cargo make gen-completions` and commit the regenerated index (`gen-completions-check` in ci will otherwise fail). No CACHE_EPOCH bump.

**Tests:** The existing lib.rs CellInputs tests (1396-2112: round trips, truncated/malformed buffers, empty) guard the parse and should compile unchanged. The scaffold tests at scaffold.rs:1079/1118 pin the call shape. Add `cell_inputs_borrow_the_wire_buffer`, which asserts `inputs.get(0).raw().as_ptr()` lies within the input buffer's address range. That proves no copy is made and fails if a future change reintroduces to_vec. Run `cargo make test-integration` once, since the e2e compile tests build a real cell against the new signature.

### cell-anim-1: Animation panel builds a full concatenated copy of every frame just to base64 it

*performance · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-cell/src/lib.rs:790` (also `crates/ironpad-cell/src/lib.rs:794`, `crates/ironpad-cell/src/canvas.rs:265`, `crates/ironpad-cell/src/canvas.rs:56`)

**Problem:** `IntoPanels for Animation` copies every frame into `rgb_bytes` (791-793) and then base64-encodes that buffer. Animations are the largest outputs in the crate: 100 frames at 400x400 is 48 MB. So peak memory holds the frames, a second 48 MB copy of them, the 64 MB base64 string, and then the JSON. Each frame is `w*h*3` bytes, always a multiple of 3, so base64 of the concatenation is exactly the concatenation of each frame's base64 and the intermediate buffer serves no purpose. The capacity expression `(w * h * 3) as usize` is also computed in u32 and can wrap. `canvas::rgb_byte_count` exists specifically to avoid that.

**Fix:** canvas.rs: rename the body of `base64_encode` to `pub(crate) fn base64_encode_into(out: &mut String, data: &[u8])`, which reserves nothing and pushes chars. Keep `pub(crate) fn base64_encode(data) -> String` as `let mut s = String::with_capacity(data.len().div_ceil(3) * 4); base64_encode_into(&mut s, data); s`. Make `rgb_byte_count` pub(crate). In lib.rs `IntoPanels for Animation`: `let frame_b64 = canvas::rgb_byte_count(w, h).div_ceil(3) * 4; let mut data = String::with_capacity(self.frames().len().saturating_mul(frame_b64)); for f in self.frames() { canvas::base64_encode_into(&mut data, f.pixels()); }`. Add a comment that this equals encoding the concatenation because each frame is a multiple of 3 bytes. No epoch bump, because the output is byte-identical.

**Tests:** Add `animation_per_frame_base64_equals_concatenated`: build a 3-frame Animation with odd dimensions (e.g. 5x3), compute the old result as `base64_encode(&concat(frames))`, and assert equality with the panel's `data`. Existing canvas base64 tests guard the encoder split.

### cell-json-1: render_json_html allocates a String per token and collects every object just to read its length

*performance · partial · value 2 · effort S*

**Where:** `crates/ironpad-cell/src/lib.rs:1168` (also `crates/ironpad-cell/src/lib.rs:1183`, `crates/ironpad-cell/src/lib.rs:1193`, `crates/ironpad-cell/src/lib.rs:1212`, `crates/ironpad-cell/src/lib.rs:1217`)

**Problem:** Each `span()` call builds a new String with `format!` and then copies it into `buf`. String and key tokens make a second throwaway allocation (`&format!("&quot;{escaped}&quot;")`), each line allocates padding with `" ".repeat()`, and each object is collected into a `Vec` (1212) only to compare `i + 1 < entries.len()`, although `map.len()` is available. A few-hundred-KB API response rendered with `Json(value)` does tens of thousands of small allocations for one panel. These are the "smart by default" paper cuts.

**Verifier's correction:** The allocation claims hold (lib.rs:1161-1230). Every `span()` formats a fresh String and then copies it. String and key tokens make an extra `format!`, and html_escape returns a new String. Each line builds padding with `" ".repeat()`. Objects are collected into a Vec (1212) only to read the length, although `map.len()` is available. This runs once per Json output, so it is a paper cut rather than a hot path. Low value. The finding's safety net is wrong: the existing tests (json_render_contains_pre_tag at 2582, json_render_colors_* at 2589) check substrings and do not pin the output, so "output HTML is identical" is currently unguarded.

**Fix:** lib.rs render_json_html: change `span` to `fn span(buf: &mut String, color: &str, text: &str) { buf.push_str("<span style=\"color:"); buf.push_str(color); buf.push_str("\">"); buf.push_str(text); buf.push_str("</span>"); }`. For strings and keys, write the opening tag, `&quot;`, the escaped text via a new `fn html_escape_into(out: &mut String, s: &str)` (make `html_escape` a wrapper over it), `&quot;`, and the closing tag. For numbers, keep `n.to_string()` or `write!` into buf. Iterate `map.iter().enumerate()` and compare `i + 1 < map.len()`. Write padding with `buf.extend(std::iter::repeat_n(' ', indent))`.

**Tests:** Before refactoring, add `json_render_golden`: render a nested fixture (an object containing an array, an empty object, an empty array, null, a bool, a number, a string needing escapes such as `<a href='x'>&`, and a key needing escapes) and assert byte equality with a literal captured from the current implementation. Keep the existing substring tests.

### js-5: New TextDecoder/TextEncoder allocated on every per-frame host call

*performance · confirmed · value 2 · effort S*

**Where:** `public/executor-core.js:202` (also `public/executor-core.js:166`, `public/executor-core.js:207`, `public/executor-core.js:234`, `public/executor-core.js:240`, `public/executor-core.js:769`, `public/executor-core.js:774`, `public/executor-core.js:919`, `public/executor-gpu.js:108`, +1 more)

**Problem:** _simRead/_simReadAll (called from simulation cells' cell_tick, i.e. once per animation frame per read), _dispatchHostMessage (sim_emit on every tick), _readLiveTickResult (every LiveView frame) and _readCellResult each build a fresh `new TextDecoder()` or `new TextEncoder()` per call. That is allocation inside the per-frame loop for objects that are stateless in non-streaming use.

**Fix:** At the top of the executor-core.js IIFE add `var UTF8_DECODER = new TextDecoder(); var UTF8_ENCODER = new TextEncoder();` and replace each per-call construction with them, keeping every existing `.slice()` because the copies are what make decode safe on shared (rayon) memory. Do the same with one module-level decoder in executor-gpu.js. The worker copy goes away with js-1. Best done in the same change as js-1 and js-2.

**Tests:** No behaviour changes. Existing coverage in execution.spec.ts and public-notebooks.spec.ts (sim and LiveView notebooks) guards it, and the js-1 and js-2 node tests exercise the shared decoder.

### cli-3: cells.run deep-clones the whole cached notebook for every foreign failure event

*performance · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-cli/src/daemon.rs:568` (also `crates/ironpad-cli/src/daemon.rs:561-566`, `crates/ironpad-cli/src/daemon.rs:572-574`)

**Problem:** While waiting on a run, each other-cell failure event triggers `state.notebook.read().await.clone()`, a full copy of every cell's source and saved_output, only to hand a borrow to target_depends_on. The separate `needs_dep_check` matches! re-states run_signal's two failure arms, and it has to be kept in step with them by hand.

**Fix:** Replace lines 561-574 with `let signal = { let nb = state.notebook.read().await; run_signal(cell_id, &event, nb.as_ref()) }; if let Some(outcome) = apply_signal(&mut held, signal) { return IpcResponse::success(outcome); }` and delete needs_dep_check. Taking an uncontended tokio read lock per event is negligible, and the writer (update_cache_from_event) runs on the WS task, so the scoped guard cannot deadlock.

**Tests:** run_signal_terminal_and_alive_shapes and run_signal_blocked_cancelled_deleted_are_terminal_for_the_target_only cover the classification. The #[tokio::test] cases near daemon.rs:1708 and 1727 cover the wait loop. Add a wait-loop test in which a foreign CellExecuted{success:false} arrives for a cell the target does not depend on, and assert the wait stays open.

### compiler-14: normalize_previous_types clones every retained type tag only to hash them

*performance · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-common/src/cell_deps.rs:131` (also `crates/ironpad-common/src/cache_key.rs:180`)

**Problem:** The only caller is the cache-key recipe, which needs `&str`. This runs per compile request, per browser blob-cache probe (every Run) and per cell per share, yet the function returns `Vec<String>`: it clones each referenced tag and, on the `last`/raw arm, clones the whole vector with `previous_types.to_vec()`. This is the gratuitous clone plus throwaway collect that the performance rules call out, in the one function every key computation passes through.

**Fix:** Change the signature to `pub fn normalize_previous_types<'a>(source: &str, previous_types: &'a [String]) -> Vec<&'a str>`. Arm 1: `return previous_types.iter().map(String::as_str).collect();`. Map arm: `.map(|(i, tag)| if refs.slots.contains(&i) { tag.as_str() } else { "" })`, and truncate with `while out.last().is_some_and(|s| s.is_empty())`. No change needed at cache_key.rs:180-188 (`t.as_bytes()` works on &&str via auto-deref; `previous_types.len()` unchanged). Adjust test expectations to `vec!["", "u32"]` style where they build Vec<String> literals, if needed.

**Tests:** Existing: cell_deps.rs normalize tests (250-290) and cache_key.rs hash_ignores_unreferenced_upstream_types. The compiler-1 golden-key test guards byte identity of the hash.

### compiler-6: Live check copies cargo stdout and stderr into Strings even when the check is clean

*performance · partial · value 2 · effort S*

**Where:** `crates/ironpad-app/src/compiler/build.rs:605` (also `crates/ironpad-app/src/compiler/build.rs:322`, `crates/ironpad-app/src/compiler/mod.rs:1983`)

**Problem:** `check_micro_crate` runs `String::from_utf8_lossy(..).to_string()` on both streams before testing `status.success()`. On the Clean path, which is most rounds once code compiles, both copies are made and dropped. This runs per check-on-type debounce. `from_utf8_lossy(..).to_string()` always copies, even when the bytes are valid UTF-8, and cargo's JSON stdout carries one compiler-artifact record per dependency (hundreds of KB for dependency-heavy cells). build.rs:322-323 does the same copy on every compile. `CheckResult::Failure.stderr` has no production reader (only the notebook gate test uses it).

**Verifier's correction:** True. check_micro_crate (build.rs:605-612) converts both streams with `from_utf8_lossy(..).to_string()`, which always allocates, before testing success, and throws them away on the Clean path. The only reader of CheckResult::Failure.stderr is the gate test (mod.rs:1983); server_fns.rs:544 ignores it. The impact is small next to a cargo check of hundreds of ms, so this is a paper cut, not a hot-path win. build.rs:322-323 needs both strings on success and failure anyway, so there the helper only saves a copy.

**Fix:** In check_micro_crate, early-return when successful: `if output.status.success() { return Ok(CheckResult::Ok); }`, then build the strings. Add `fn into_string_lossy(bytes: Vec<u8>) -> String { String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()) }` in build.rs and use it at build.rs:322-323 and 605-606, destructuring `output` to move stdout/stderr out. Keep `stderr` in CheckResult::Failure; the gate test uses it for failure reports.

**Tests:** Add a unit test for into_string_lossy: valid UTF-8 round-trips, and an invalid byte becomes U+FFFD. Existing behavioral coverage is the server_fns check_cell_core tests (Clean/Errors/Skipped) and all_public_notebook_cells_compile under test-integration.

## Wave 4: readability, dead code, and module boundaries

Stale comments first (compiler-4 matters most: the `CACHE_EPOCH` doc names two pins that no longer exist and omits the one that actually needs a manual bump), then dead code, then the copy-paste clusters and oversized units.

### compiler-4: Toolchain-pin comments still describe the pre-PRD-0067 world, and the CACHE_EPOCH caveat names nonexistent pins

*readability · confirmed · value 4 · effort S*

**Where:** `crates/ironpad-common/src/cache_key.rs:77` (also `crates/ironpad-app/src/compiler/build.rs:105`, `crates/ironpad-app/src/compiler/build.rs:113`, `crates/ironpad-app/src/compiler/build.rs:180`, `crates/ironpad-app/src/compiler/build.rs:529`, `crates/ironpad-app/src/compiler/build.rs:1045`, `crates/ironpad-app/src/compiler/build.rs:1081`, `crates/ironpad-app/src/compiler/build.rs:1100`, `crates/ironpad-app/src/lib.rs:75`, +2 more)

**Problem:** The CACHE_EPOCH doc is where you look to learn which pin bumps need a manual epoch bump. It says `AUTODIFF_TOOLCHAIN`/`ATOMICS_TOOLCHAIN` in build.rs; neither exists any more. It omits the pin that actually has this property: `BROWSERPOD_TOOLCHAIN`, whose own doc at build.rs:113-115 says it is outside the fingerprint and needs an epoch bump. Other stale text: build.rs:105 says 'the other three pins'. build.rs:180-184 says 'every cell except rayon/atomics ones compiles on that pin', which is false since `cell_toolchain` ignores features. build.rs:529 says check always targets wasm32-unknown-unknown. build.rs:1045/1081/1100 say 'three cell-toolchain pins', 'fourth pin' and 'three nightly constants'. lib.rs:75-76 cites a test `toolchain_pins_are_installed_by_the_image` that does not exist (it is `toolchain_pins_are_in_sync_across_dockerfile_ci_and_toolchain_toml`). server_fns.rs:237-239 says the scaffold returns `needs_atomics`, which it no longer does. cache.rs:3-4 lists a hash recipe missing half its fields.

**Fix:** Docs only: 1. cache_key.rs:77-81: 'The fingerprint tracks only `CELL_TOOLCHAIN`'s rustc (plus the wasm-bindgen CLI). `BROWSERPOD_TOOLCHAIN` (compiler/build.rs) is NOT in it: bumping the BrowserPod pack does not invalidate Linux blobs by itself, so bump this epoch with it.' 2. build.rs:105 'Unlike `CELL_TOOLCHAIN` this is not a nightly date'. 3. Replace the build.rs:180-184 orphan comment with 'Every Executor cell compiles on `CELL_TOOLCHAIN` whatever its features (see `cell_toolchain`)', or delete it as redundant with cell_toolchain's doc. 4. build.rs:529 'Runs `cargo check --target {triple} --release --message-format=json` for the cell's [`CellTarget`]'. 5. build.rs:1045 'The cell-toolchain pins'; 1081 'The BrowserPod pin'; 1100 'must be `CELL_TOOLCHAIN` or the BrowserPod pack's recorded nightly'. 6. lib.rs:76 names `toolchain_pins_are_in_sync_across_dockerfile_ci_and_toolchain_toml`. 7. server_fns.rs:236-238 becomes 'Cache miss (or forced recompile): scaffold the micro-crate now, the on-disk work a cache hit skips.' 8. cache.rs:1-9 becomes a one-paragraph 'Stores/retrieves compiled blobs under `{cache_dir}/blobs/{hash}.*`; the key recipe is `ironpad_common::cache_key` (see `content_hash`).' Separately, and only if wanted, file a follow-up to fold BROWSERPOD_TOOLCHAIN into the Linux-target key.

**Tests:** Comment-only, so no behavior change and no new test. `cargo make ci` (fmt + clippy doc lints) guards it. A `cargo doc` pass with intra-doc links checks that the renamed test/const references resolve where written as [`...`] links.

### editor-15: Stale comment in live-check request contradicts the current Run gating and the code below it

*readability · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/pages/notebook_editor/pipeline.rs:778` (also `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:1372-1375`, `crates/ironpad-app/src/pages/notebook_editor/pipeline.rs:787-790`)

**Problem:** Lines 778-786 claim that the per-cell Run button is gated on `is_markdown || is_shared`, that a Linux cell therefore 'sails through' and reports a green run that did nothing, and that 'if Linux cells ever become runnable ... this must become `cell.cell_type`'. All three are false now. cell_item.rs gates Run on `runs_in_editor` (its own comment at 1372 records the fix), `wire_run_effect` refuses non-Code cells, and the field already reads `cell_type`. A reader is left believing there is an open safety bug.

**Fix:** Delete pipeline.rs lines 778-786 (from '// Correct only while the editor's Run affordance' through '// from the editor, this must become `cell.cell_type`.'). Keep the 787-789 paragraph about shared cells reporting Code.

**Tests:** Comment-only change; no test needed.

### server-13: Stale comments that contradict the code

*readability · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-server/src/main.rs:104` (also `crates/ironpad-server/src/crawl.rs:23`, `crates/ironpad-server/src/og/mod.rs:37`, `crates/ironpad-server/src/oembed.rs:36`, `crates/ironpad-server/src/state.rs:390`)

**Problem:** (1) main.rs:104-105: 'Refuses combinations that are individually valid but unsafe together. Fail at startup…' sits above `let test_auth = args.test_auth;`. Nothing refuses anything, and config.rs `test_auth_and_admin_login_may_coexist` records that the refusal was removed on purpose. (2) crawl.rs:23-25: a leftover paragraph from an earlier constant is glued onto DISALLOWED's doc and repeats the next paragraph. (3) og/mod.rs:37-39: says evicting down to 3/4 'amortizes the directory walk instead of paying it on every write'. But `evict_if_needed` calls `cache_entries` (readdir plus a stat per file) on every cache miss before checking the total; only the deletions are amortized. (4) oembed.rs:36-37: CACHE_AGE's reason is 'Public notebooks change only at deploy; shared ones are immutable'. Since PRD-0057, mutable notebooks go through this handler and change on every Push. The TTL itself was deliberately left alone per the review decision; only the stated reason is wrong. (5) state.rs:390: disconnect_guests says 'sending them a close reason', but it sends nothing.

**Fix:** (1) main.rs:104-105: delete the two comment lines. (2) crawl.rs: delete lines 22-25 (the orphan paragraph plus the duplicated summary line) and keep the 'Paths kept out of search results by robots.txt' doc starting at the `/embed/` explanation. (3) og/mod.rs:36-39: 'Low-water mark the reaper evicts down to once MAX_CACHE_BYTES is hit. Reclaiming a quarter at a time amortizes the deletions; the size walk itself runs on every render miss, which is cheap beside the rasterize that miss is already paying.' (4) oembed.rs:36-37: 'How long a consumer may cache this response, in seconds. Public notebooks change only at deploy and shared ones are immutable; a mutable notebook's title can change on Push (or it can go private), and a day of staleness there is an accepted trade-off rather than an oversight.' (5) Folded into server-2's doc fix.

**Tests:** Comment-only changes; no test impact. `cargo make fmt-check` and `cargo make clippy` still apply.

### cp-15: Misattached and stale doc comments that now describe the wrong item

*readability · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-app/src/components/output_render.rs:275` (also `crates/ironpad-app/src/components/app_layout.rs:341`, `crates/ironpad-app/src/components/view_only_notebook.rs:1120`, `crates/ironpad-app/src/components/view_only_notebook.rs:58`, `crates/ironpad-app/src/components/view_only_notebook.rs:1244`, `crates/ironpad-app/src/components/linux_cell.rs:778`, `crates/ironpad-app/src/components/social_meta.rs:97`, `crates/ironpad-app/src/components/social_meta.rs:213`)

**Problem:** output_render.rs:275-277: `render_display_panel`'s doc line and its `#[allow(clippy::needless_pass_by_value)]` sit above `PanelMode`, so the enum's rustdoc opens with 'Render a single DisplayPanel', the allow lands on an enum, and the function has no doc. app_layout.rs:341-352: AuthStatus's doc block ('Sign-in element for the HEADER...') is attached to `AuthSilhouette`, which was inserted between the doc and its fn, leaving AuthStatus undocumented. view_only_notebook.rs:1120: the section banner reads `ViewOnlySharedCell` above `ViewOnlyInertCell`. :58-59 lists specs as shared/public only, omitting mutable. :1244-1248 claims ViewOnlyOutputCaption is 'the one place' captions render, while LinuxTerminal hand-rolls the same caption markup (linux_cell.rs:778-789). social_meta.rs:97-99 says oembed is set on /public and /shared, but /mutable sets it too (PRD-0057). :213-214 cites `window.IronpadStorage` exposing a 'mutable-share user key' that PRD-0053 deleted.

**Fix:** output_render.rs: move the two doc lines and the `#[allow]` from :275-277 down to directly above `pub(crate) fn render_display_panel` (:287). Keep the allow unless clippy on both targets is clean without it. app_layout.rs: move the AuthStatus doc block (:341-352) to directly above `#[component] fn AuthStatus` (:365). view_only_notebook.rs:1120: rename the banner to `ViewOnlyInertCell / ViewOnlySharedCell`. :58: `(shared/{hash}, public/{filename} or mutable/{id})`. :1247: soften to 'the one place code-cell output captions render (Linux cells draw their own Terminal caption)'. Do not route LinuxTerminal through it. social_meta.rs:97-99: '`/public`, `/shared` and `/mutable` (PRD-0057)'. social_meta.rs:213-214: replace the user-key clause with the current stake ('ironpad's own origin, where IndexedDB notebooks and the session-bearing requests live').

**Tests:** These are comment-only changes except the `#[allow]` move. `cargo make clippy` and `cargo doc` are the gates. No test is needed.

### server-fns-12: Db::update_mutable_share is dead in production: a pre-draft 'push' that bypasses the draft slot

*readability · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/db.rs:710` (also `crates/ironpad-app/src/db.rs:707-708`, `crates/ironpad-app/src/db.rs:1554-1564`, `crates/ironpad-app/src/db.rs:581-584`)

**Problem:** update_mutable_share has no production caller; only share_lifecycle_with_owner uses it. Push has been promote_draft since PRD-0054. The function is still `pub` and documented as '(a push)'. It writes notebook_json directly without touching draft_json or draft_bytes, producing a row shape the save-then-promote sequence cannot. create_account_notebook's doc (581-584) says no path should be able to write such a shape, 'including from a test fixture'. The test step labelled 'Push updates content + manifest' is exercising the obsolete path.

**Fix:** Delete Db::update_mutable_share (db.rs:707-738). In share_lifecycle_with_owner (db.rs:1554-1564), replace the 'Push updates content + manifest' step with `db.save_draft(&id, "{\"title\":\"nb2\"}").await.unwrap(); db.promote_draft(&id, Some("{\"version\":1}".to_string()), "{\"title\":\"nb2\"}".len() as u64).await.unwrap();` and keep the two assertions. Also add `assert!(row.draft_json.is_none())` after the promote, which shows the clean state that a real push produces.

**Tests:** The rewritten share_lifecycle_with_owner is the guard. The promote_draft tests and total_mutable_bytes_sums_notebook_json_and_drafts already cover the real push. Run `cargo make clippy` to confirm nothing else referenced the deleted fn.

### common-2: IronpadNotebook::content_matches is dead, and its doc describes a feature that was deleted

*readability · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-common/src/types.rs:558` (also `crates/ironpad-common/src/types.rs:1108`)

**Problem:** The only callers of `content_matches` are its own test (types.rs:1108-1137). It existed for the mutable-share divergence banner ("a local working copy and the published copy of a mutable share"), which PRD-0054 removed along with the local mutable store. It also clones the whole notebook, including every cell source and saved output, just to compare one timestamp, so anyone who revives it gets that cost silently.

**Fix:** Delete `IronpadNotebook::content_matches` (types.rs:550-562) and the test `content_matches_ignores_updated_at_but_not_content` (types.rs:1107-1137). Then grep docs and PRDs for stale references. CLAUDE.md's changelog mentions `IronpadNotebook::content_matches` historically, which is fine to leave as history.

**Tests:** No behavior change. `cargo make ci` (clippy -D warnings plus the build) guards that nothing referenced it.

### cp-12: Dead `menu` prop on ViewOnlyNotebook, left over from the deleted PRD-0049 rebind entry

*readability · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/components/view_only_notebook.rs:88` (also `crates/ironpad-app/src/components/view_only_notebook.rs:423`)

**Problem:** No caller passes `menu=`: all five `<ViewOnlyNotebook>` sites set only embed, embed_spec, autorun, hide_fork, cell_outputs, share_manifest or controls. The doc names 'the mutable reader's rebind entry, PRD-0049', and PRD-0053 deleted that rebind form. The 29-line dropdown renderer at :423-451, with its display-toggle comment, is unreachable. Its `view-only-menu` class has no rule in style/main.scss.

**Fix:** Delete the `menu` prop (view_only_notebook.rs:88-93) and the `{menu.map(|items| { ... })}` block (:423-451). Reword the `controls` doc at :94 to drop the '☰ menu' reference. Then check the imports: `icons::MENU` is probably still used elsewhere, so remove only an import that clippy flags as unused.

**Tests:** This deletes dead code, so behavior cannot change. `cargo make clippy` (both ssr and hydrate) is the gate. mutable-shares.spec.ts still covers the `controls` slot (the Edit link).

### compiler-10: scaffold_micro_crate returns an anonymous (PathBuf, u32, bool, bool) whose two bools no production caller reads

*readability · partial · value 2 · effort S*

**Where:** `crates/ironpad-app/src/compiler/scaffold.rs:67` (also `crates/ironpad-app/src/compiler/scaffold.rs:45`, `crates/ironpad-app/src/compiler/scaffold.rs:173`, `crates/ironpad-app/src/compiler/scaffold.rs:192`, `crates/ironpad-app/src/compiler/scaffold.rs:565`, `crates/ironpad-app/src/server_fns.rs:241`, `crates/ironpad-app/src/server_fns.rs:508`, `crates/ironpad-app/src/compiler/mod.rs:1721`, `crates/ironpad-app/src/compiler/mod.rs:2048`)

**Problem:** Both production call sites destructure `(crate_dir, preamble_lines, _is_async, _is_simulation)` and discard the bools. Only two tests read them. Positional bools in a public return type are easy to swap silently. The Linux path has to fabricate `false, false` to match the tuple shape, and the doc spends lines explaining the positions.

**Verifier's correction:** True. scaffold_micro_crate returns `(PathBuf, u32, bool, bool)` (scaffold.rs:67). Both production callers discard the two bools (server_fns.rs:241, 508), and scaffold_linux_crate fabricates `false, false` (scaffold.rs:192). The finding undercounts the churn, though: about 40 test sites destructure the tuple positionally, and four read the bools (scaffold.rs:1646/1661 is_async, 1721 and mod.rs:2048 is_sim/is_async). This is a readability gain in a pub(crate)-visible return type. On its own it is low value; it is worth doing when compiler-7 touches the same test code.

**Fix:** Add `pub struct Scaffolded { pub crate_dir: PathBuf, pub preamble_lines: u32 }` in scaffold.rs. Return it from scaffold_micro_crate and scaffold_linux_crate, dropping the bools and the doc lines that explain tuple positions. generate_lib_rs keeps its internal tuple. Rewrite the four tests that assert is_async/is_simulation (scaffold.rs:1642-1675, 1721-1737; mod.rs:2048-2062) against `generate_lib_rs(..)` directly. Mechanically update the remaining destructures to field access (`.crate_dir`, `.preamble_lines`). Callers in server_fns.rs:241/508 bind fields by name (or go through compiler-2's scaffold_request).

**Tests:** Existing scaffold unit tests (preamble/base_preamble tests around scaffold.rs:2326-2540, is_async tests at 1642) and the e2e/gate tests under test-integration cover the field values. No new behavior, so no new test beyond moving the is_async/is_sim assertions onto generate_lib_rs.

### editor-14: yield_for_cell_flush(ms) used as a general sleep at 5 of its 9 call sites

*readability · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-app/src/pages/notebook_editor/state.rs:506` (also `crates/ironpad-app/src/pages/notebook_editor/state.rs:479`, `crates/ironpad-app/src/pages/notebook_editor/pipeline.rs:834`, `crates/ironpad-app/src/pages/notebook_editor/metadata_panel.rs:246`, `crates/ironpad-app/src/pages/notebook_editor/shared_editor_panel.rs:137`, `crates/ironpad-app/src/pages/notebook_editor/mod.rs:73`)

**Problem:** The helper is documented as the yield that lets per-cell flush effects run, and all four real flush yields pass `CELL_FLUSH_YIELD_MS`. The other five callers use it as a timer for unrelated things: the draft-save debounce (1.5s/5s), the in-flight drain poll, the live-check skip-retry backoff (3s), and the Saving… floor. A reader of `schedule_draft_save` or the check retry sees a 'cell flush' where none happens.

**Fix:** Replace those five calls with `crate::components::run_flow::sleep_ms(ms).await`. The two Saving floors fold into editor-7's `persist_with_saving_floor`. Make `yield_for_cell_flush()` take no argument and always sleep `CELL_FLUSH_YIELD_MS`, ideally private to state.rs behind editor-3's `NotebookState::flush_cells`.

**Tests:** No behaviour change. Guarded by account-notebooks.spec.ts (draft debounce), live-check.spec.ts (skip retry) and notebook-metadata.spec.ts (Saving floor).

### editor-13: CellItem clones the cell id ~25 times into separate bindings and ~15 StoredValues

*readability · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:26` (also `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:27-34`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:41`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:63`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:144-145`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:171`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:202`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:246`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:296`, +13 more)

**Problem:** Every closure gets its own `cell_id_for_*` String clone or its own `StoredValue::new(cell.id.clone())`. Two of them (`cell_id_for_delete_sv`, `cell_id_for_delete_cleanup_sv`, lines 144-145) hold the same id for one closure, which then clones both. The pattern predates `StoredValue` being Copy. It adds ~40 lines of noise to a 1,500-line component and allocates per mount.

**Fix:** At the top of CellItem: `let cell_id = StoredValue::new(cell.id.clone());`. Delete every `cell_id_for_*` binding and per-closure `StoredValue::new(cell.id.clone())`. Inside closures use `cell_id.get_value()` when an owned String is needed (mutations, props) and `cell_id.with_value(|id| ...)` for comparisons (the order/shared derives, the queue position, the stale lookup). Pass `cell_id` directly as `CellRunCtx.cell_id`. Where a view prop needs a String, use `cell_id.get_value()`.

**Tests:** No behaviour change. cargo make clippy plus the existing editor e2e (editor-ux.spec.ts, cell-disposal.spec.ts, execution.spec.ts) guard it. No new test is needed.

### cp-5: requestAnimationFrame loop machinery copied three times across Animation, Simulation and LiveView

*dry · confirmed · value 3 · effort M*

**Where:** `crates/ironpad-app/src/components/live_view_panel.rs:98` (also `crates/ironpad-app/src/components/animation_canvas.rs:207`, `crates/ironpad-app/src/components/animation_canvas.rs:403`, `crates/ironpad-app/src/components/animation_canvas.rs:143`, `crates/ironpad-app/src/components/live_view_panel.rs:56`, `crates/ironpad-app/src/components/animation_canvas.rs:480`, `crates/ironpad-app/src/components/animation_canvas.rs:550`, `crates/ironpad-app/src/components/live_view_panel.rs:188`, `crates/ironpad-app/src/components/live_view_panel.rs:265`, +7 more)

**Problem:** Three components hand-roll the same loop: the `RafClosure` type, `raf_id_signal`, `cb_holder`, a 17-line `start_loop` (three identical copies), the 'request a frame and store its id' snippet (nine copies), the fps-to-interval computation, the cleanup cancel and `toggle_play`. `cancel_raf` is defined twice. Within SimulationCanvas and LiveViewPanel the per-tick body is duplicated verbatim between the loop and the Step button, including the `kind` 1/2/_ mapping in live_view_panel.rs, which appears twice. The play/pause/step/frame-counter controls markup appears three times, and the `simBusWrite` guard shim is duplicated between animation_canvas.rs and output_render.rs. This loop carries subtle, hard-won invariants (the weak self-reference, the paused-loop restart guard, disposal-safe reads). The file comments restate them three times, and a fix to one copy will not reach the others.

**Fix:** Add `components/raf_loop.rs` (hydrate-only, `pub(crate)`) with `pub(crate) struct RafLoop { raf_id: RwSignal<Option<i32>>, holder: StoredValue<Option<Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>>, LocalStorage> }`, which is Copy. Give it:
- `RafLoop::new(fps: u32, playing: RwSignal<bool>, on_frame: impl FnMut() + 'static) -> Self`. It builds the closure with the weak self-reference, applies the fps throttle (`1000/fps`, 1000 when fps == 0), stops rescheduling and clears the id when `!playing`, and registers `on_cleanup` to cancel.
- `start(self)`: the guarded restart.
- `kick(self)`: the first request.
Each component supplies only its per-frame body. The animation body advances the frame. The simulation and live bodies keep their own `tick_in_flight` guard and spawn_local tick, extracted into a local `tick_once` closure that is also called by the Step button. Move `fn live_kind_name(kind: u32) -> &'static str` into live_view_panel.rs next to `render_live_content` and use it at both sites. Keep a single `sim_bus_write` inline_js module in output_render.rs as `pub(crate) mod sim_bus_js`, and delete animation_canvas.rs's copy. The shared `PlaybackControls` component is optional; do it only if the markup is truly identical after the refactor.

**Tests:** Guards: notebook-smoke.spec.ts (Simulation canvas), persisted-outputs.spec.ts (Animation/LiveView snapshots), cell-disposal.spec.ts (navigate away mid-loop, which the weak-ref and cleanup invariants exist for). Add an e2e that clicks pause then play twice quickly on the Game of Life canvas and asserts the frame counter advances at roughly 1x rate, not 2x. That locks the restart guard that `RafLoop::start` now owns in one place.

### cp-6: `window.confirm` boilerplate hand-rolled at 10 sites, three of them unwrapping `window()`

*dry · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/pages/home_page.rs:497` (also `crates/ironpad-app/src/pages/admin.rs:96`, `crates/ironpad-app/src/pages/admin.rs:121`, `crates/ironpad-app/src/pages/notebook_editor/sharing.rs:171`, `crates/ironpad-app/src/pages/notebook_editor/sharing.rs:391`, `crates/ironpad-app/src/pages/notebook_editor/sharing.rs:446`, `crates/ironpad-app/src/pages/notebook_editor/sharing.rs:490`, `crates/ironpad-app/src/pages/notebook_editor/history.rs:158`, `crates/ironpad-app/src/pages/notebook_editor/mod.rs:967`, +1 more)

**Problem:** Every destructive action re-spells `web_sys::window().is_some_and(|w| w.confirm_with_message(..).unwrap_or(false))`. Three sites use `.unwrap()` on `window()` instead (home_page.rs:498, notebook_editor/mod.rs:967, cell_item.rs:341), in library code that the project rules say must not unwrap. The notebook-delete prompt string "Delete this notebook? This cannot be undone." is also duplicated between home_page.rs:499 and notebook_editor/mod.rs:969.

**Fix:** Add a hydrate-gated `pub(crate) fn confirm(message: &str) -> bool { web_sys::window().is_some_and(|w| w.confirm_with_message(message).unwrap_or(false)) }` in a new `components/dialog.rs` (`pub(crate) mod dialog;` in components/mod.rs). Replace all 10 sites with `crate::components::dialog::confirm(..)`. Add `pub(crate) const DELETE_NOTEBOOK_CONFIRM: &str = "Delete this notebook? This cannot be undone.";` in the same module and use it at home_page.rs:499 and notebook_editor/mod.rs:968. Leave sharing.rs:491's account-delete wording alone, since it is a different message.

**Tests:** Guards: home.spec.ts (delete local notebook), account-notebooks.spec.ts (Unpublish/Delete confirms), local-history.spec.ts (Restore confirm), admin.spec.ts (cache clear), mutable-shares.spec.ts (Discard Draft). All of them accept the `page.on('dialog')` prompt, so they exercise the helper. No new test is needed beyond these; the change is mechanical.

### cp-7: View-only cell frame header and body-collapse class copied across four cell kinds (five with the editor)

*dry · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/components/view_only_notebook.rs:931` (also `crates/ironpad-app/src/components/view_only_notebook.rs:965`, `crates/ironpad-app/src/components/view_only_notebook.rs:1137`, `crates/ironpad-app/src/components/view_only_notebook.rs:1147`, `crates/ironpad-app/src/components/view_only_notebook.rs:1178`, `crates/ironpad-app/src/components/view_only_notebook.rs:1188`, `crates/ironpad-app/src/components/linux_cell.rs:578`, `crates/ironpad-app/src/components/linux_cell.rs:612`, `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs:1044`)

**Problem:** ViewOnlyCodeCell, ViewOnlyInertCell, ViewOnlySharedCell and ViewOnlyLinuxCell each re-spell the same `collapsed` signal. Each also re-spells the same `body_class` derive ("ironpad-cell-body" / "--collapsed"), which appears five times counting cell_item.rs:1044. And each re-spells the same header lead: the collapse button with `Chevron`, the `[n]` index span and the label span, followed by a read-only rust MonacoEditor body. Any Studio-frame tweak, such as the index format or the collapse affordance, has to be made in four places.

**Fix:** In view_only_notebook.rs add `pub(crate) fn cell_body_class(collapsed: RwSignal<bool>) -> Signal<&'static str>` and a `#[component] pub(crate) fn ViewOnlyCellHeaderLead(collapsed: RwSignal<bool>, index: Option<usize>, #[prop(into)] label: String) -> impl IntoView` that returns the button + index + label fragment. Use both at the four view-only sites, and use `cell_body_class` at cell_item.rs:1044. Each cell keeps only its badge, pill, meta and run control after the lead. Separately, decide whether Linux/Unsupported frames should be numbered. If yes, change the predicate at :483 to `c.shared || matches!(c.cell_type, CellType::Code | CellType::Linux | CellType::Unsupported(_))`, which matches the dispatch. If no, drop `index` from those two components.

**Tests:** Guards: studio-chrome.spec.ts (frame headers), responsive.spec.ts, shared-cells.spec.ts (the shared frame), and linux-cells.spec.ts (the Linux frame without a pod). Add a studio-chrome assertion that clicking `.ironpad-cell-collapse-btn` toggles `.ironpad-cell-body--collapsed` on a code cell and on a shared cell. If the numbering predicate changes, add a unit-testable `frame_indices(cells: &[IronpadCell]) -> Vec<Option<usize>>` extraction with a test mixing markdown, code, linux and shared cells.

### cp-10: Shared/mutable notebook fetch resources duplicated between the full pages and their embeds

*dry · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-app/src/pages/embed_notebook.rs:31` (also `crates/ironpad-app/src/pages/shared_notebook.rs:28`, `crates/ironpad-app/src/pages/embed_notebook.rs:115`, `crates/ironpad-app/src/pages/mutable_notebook.rs:112`)

**Problem:** The shared loader (fetch the notebook, then fetch the manifest with a degrade-to-None policy) is copy-pasted between SharedNotebookPage and EmbedSharedPage, comment included. The mutable loader (access check, then fetch the manifest only when `Found`) is copy-pasted between MutableNotebookPage and EmbedMutablePage. These are the policies that decide which viewers get blob snapshots (PRD-0047) and when a manifest is withheld (PRD-0061/0064). The codebase has already shipped bugs from exactly this two-copies shape.

**Fix:** In pages/mod.rs (or a new `pages/load.rs`) add `pub(crate) async fn load_shared(hash: String) -> Result<(IronpadNotebook, Option<ShareManifest>), ServerFnError>` and `pub(crate) async fn load_mutable(id: String) -> Result<(MutableNotebookAccess, Option<ShareManifest>), ServerFnError>`, each carrying the one copy of the degrade comment. Use `Resource::new(move || hash.get(), load_shared)` in SharedNotebookPage and EmbedSharedPage, and `load_mutable` in EmbedMutablePage. In MutableNotebookPage, write `let (access, manifest) = load_mutable(id).await?;` and then compute `signed_in_hint` as today.

**Tests:** Guards: embed.spec.ts, mutable-shares.spec.ts, private-shares.spec.ts (manifest withheld for private), blob-cache.spec.ts (share snapshots used), and crates/ironpad-server/tests/unpublished_notebooks.rs (raw-body 404 surfaces, including embed). Nothing new is needed for a pure extraction.

### cp-13: Error-boundary and loading panels hand-written six and five times across the pages

*dry · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-app/src/pages/mutable_notebook.rs:296` (also `crates/ironpad-app/src/pages/public_notebook.rs:87`, `crates/ironpad-app/src/pages/shared_notebook.rs:90`, `crates/ironpad-app/src/pages/embed_notebook.rs:201`, `crates/ironpad-app/src/pages/mutable_notebook.rs:246`, `crates/ironpad-app/src/pages/mutable_notebook.rs:310`, `crates/ironpad-app/src/pages/public_notebook.rs:42`, `crates/ironpad-app/src/pages/shared_notebook.rs:49`, `crates/ironpad-app/src/pages/mutable_notebook.rs:210`, +2 more)

**Problem:** `<div class="ironpad-error-boundary"><div class="ironpad-error-boundary-icon"><Icon ../></div><p class="ironpad-error-boundary-message">..</p>[<p class="..-hint">..</p>]</div>` is written out six times, and the `ironpad-loading` wrapper five times with only the message changing. These are the deepest-nested parts of the page views and make up much of their length.

**Fix:** Add a new `components/notice.rs` (`pub(crate) mod notice;`) containing `#[component] pub(crate) fn ErrorNotice(icon: IconData, #[prop(into)] message: String, #[prop(optional)] children: Option<Children>) -> impl IntoView`, which renders the hint `<p class="ironpad-error-boundary-hint">{children}</p>` only when Some, and `#[component] pub(crate) fn LoadingNotice(#[prop(into)] message: String) -> impl IntoView`. Replace the eleven sites. Keep `mark_not_found()` at the call sites, since it controls the HTTP status. The mutable Private arm passes its signed-in/anonymous branch as children.

**Tests:** Guards: routes.spec.ts and private-shares.spec.ts (denial copy and sign-in link), account-notebooks.spec.ts (unpublished 404 copy), embed.spec.ts, and crates/ironpad-server/tests/unpublished_notebooks.rs (raw-body status). The mutable_notebook.rs unit tests on `LOADING_MESSAGE` still apply. No new test is needed.

### cp-14: Widget renderers: slider/number and checkbox/switch are near-identical, with a no-op label conditional repeated five times

*dry · partial · value 2 · effort S*

**Where:** `crates/ironpad-app/src/components/output_render.rs:789` (also `crates/ironpad-app/src/components/output_render.rs:501`, `crates/ironpad-app/src/components/output_render.rs:672`, `crates/ironpad-app/src/components/output_render.rs:866`, `crates/ironpad-app/src/components/output_render.rs:523`, `crates/ironpad-app/src/components/output_render.rs:600`, `crates/ironpad-app/src/components/output_render.rs:752`, `crates/ironpad-app/src/components/output_render.rs:811`, `crates/ironpad-app/src/components/output_render.rs:989`, +2 more)

**Problem:** `render_number` repeats `render_slider` line for line: the same min/max/step/default parsing, bus key, initial bus write and `on_input` with bincode f64 plus a bus write. It differs only in `type=` and the value readout. `render_switch` likewise repeats `render_checkbox` except for the switch-slider span. Five renderers spell `if label.is_empty() { String::new() } else { label.to_owned() }`, which is exactly `label.to_owned()`, although `InteractiveWidget` already owns `label` as a String (:466). `render_progress` re-implements `widget_label` at :997-1001.

**Verifier's correction:** `render_number` (output_render.rs:789-857) duplicates `render_slider`'s parsing, bus-key handling, initial bus write and `on_input` almost line for line. `render_switch` (:866) duplicates `render_checkbox`. The `if label.is_empty() { String::new() } else { label.to_owned() }` no-op is real at the cited sites. One part of the proposed fix is wrong: `render_progress` (:997-1001) deliberately renders NOTHING for an empty label, while `widget_label` renders an empty `<span />` (:490-496). Swapping them adds a DOM node inside `.ironpad-interactive-widget`, which can shift the flex/grid layout. It is not a pure refactor, so drop it.

**Fix:** (1) Delete the five `label_text` conditionals and use `label.to_owned()`. Better, change the renderers to take `label: String` by value from `InteractiveWidget`, which already owns it at :466. (2) Add `fn cfg_f64(cfg: &serde_json::Value, key: &str, default: f64) -> f64` and `fn bus_key(cfg: &serde_json::Value) -> Option<String>` near `widget_label`. (3) Merge the slider and number renderers into `fn render_numeric(cfg, label, cell_id, sink, input_type: &'static str, show_value: bool)`, and the checkbox and switch renderers into `fn render_toggle(cfg, label, cell_id, sink, switch: bool)`. Keep each variant's exact markup behind the flag. Leave `render_progress`'s label handling as it is, apart from the no-op conditional.

**Tests:** The existing `#[cfg(test)] mod tests` in output_render.rs (:1015) covers the bincode encoders. Widget e2e coverage is in notebook-smoke.spec.ts and public-notebooks.spec.ts (widget-driven reruns). Add an SSR render unit test (`view.to_html()`) per widget kind asserting `type="range"` and `type="number"`, the presence of the value readout, and that the switch has `.ironpad-switch-slider` and the checkbox does not. That locks the merge.

### js-2: The bindgen-vs-raw export ternary is inlined six times beside an existing `_cellMemory` helper

*dry · confirmed · value 3 · effort S*

**Where:** `public/executor-core.js:160` (also `public/executor-core.js:193-198`, `public/executor-core.js:225-230`, `public/executor-core.js:265-267`, `public/executor-core.js:276-278`, `public/executor-core.js:395-401`, `public/executor-core.js:207-216`, `public/executor-core.js:240-249`, `public/executor-worker.js:158-160`)

**Problem:** `entry.type === "bindgen" ? entry.wasm.X : entry.instance.exports.X` is repeated for `memory` in _dispatchHostMessage, _simRead, _simReadAll, both GPU shims and the worker, and for `ironpad_alloc` in both sim readers. `_cellMemory` (395) is the named helper for exactly this, but only the JSPI methods use it, and it is less defensive than the inline copies. _simRead and _simReadAll also share an identical 'alloc 4+n, write u32-LE length, copy bytes' tail.

**Fix:** Add `CellExecutor.prototype._cellExports = function (cellId) { var e = this.modules.get(cellId); if (!e) return null; return (e.type === "bindgen" ? e.wasm : (e.instance && e.instance.exports)) || null; };` beside _cellMemory, and make _cellMemory `var x = this._cellExports(cellId); return x ? x.memory || null : null;`, which also gains the null guards. Convert _dispatchHostMessage (or the new _readHostText from js-1), _simRead, _simReadAll, _gpuWriteBufferForCell and _gpuDispatchComputeForCell to read memory and ironpad_alloc from _cellExports. Add `function _writeLengthPrefixed(memory, alloc, bytes)` at module scope that returns ptr, or 0 if the alloc fails, and call it from both sim readers. Land this together with js-1, which removes the worker copy.

**Tests:** public-notebooks.spec.ts and execution.spec.ts cover simulation, sim-bus, GPU and blocking cells end to end. Extend the js-1 node test file with a _simRead case (a fake raw entry exporting memory plus a bump ironpad_alloc) that asserts the u32-LE length prefix and payload bytes.

### js-3: Four copies of the worker-to-main-thread fallback in the executor bridge

*dry · confirmed · value 3 · effort S*

**Where:** `public/executor-bridge.js:298` (also `public/executor-bridge.js:261-283`, `public/executor-bridge.js:337-362`, `public/executor-bridge.js:375-399`)

**Problem:** loadBlob, execute, tick and tickLive each carry the same catch body: rethrow on AbortError, console.warn, await _ensureMainExecutor, look up _blobCache, throw if missing, loadBlob-if-not-loaded, check the method exists, call it, tag `fallback: true`. The comments already treat this as one policy ("see execute()"). Any change to it, such as the AbortError rule added for terminate(), has to be applied four times.

**Fix:** Add `BridgeExecutor.prototype._mainThreadFallback = async function (cellId, what, workerError, method, args)` that does the AbortError rethrow, the console.warn ("ironpad: worker " + what + " failed for " + cellId + ", retrying on main thread. Worker error: " + workerError.message), the executor, blob and load steps, and then: when `method` is null, returns undefined (the loadBlob case, leaving _loadedCache untouched as its comment requires); otherwise throws workerError if `typeof exec[method] !== "function"`, and does `var r = await exec[method].apply(exec, args); r.fallback = true; return r;`. Each public method becomes `.catch(function (e) { return self._mainThreadFallback(cellId, "tick", e, "tick", [cellId]); })`, with execute passing `[cellId, inputBytes]` and loadBlob passing `null`. Keep the loadBlob-specific comment about _loadedCache on the helper call.

**Tests:** tests/e2e/worker-recovery.spec.ts drives the respawn cap and the main-thread fallback through synthetic ErrorEvents. Extend it to assert that a simulation cell's tick also lands on the fallback, marked fallback:true, once the worker is parked, because tick and tickLive are the paths it does not currently exercise.

### js-4: tick and tickLive are two full copies of the same bindgen/raw tick paths

*dry · confirmed · value 3 · effort M*

**Where:** `public/executor-core.js:938` (also `public/executor-core.js:799-810`, `public/executor-core.js:814-839`, `public/executor-core.js:843-881`, `public/executor-core.js:953-977`, `public/executor-core.js:981-1018`)

**Problem:** tick/tickLive (dispatch), _tickBindgen/_tickLiveBindgen and _tickRaw/_tickLiveRaw are identical except for the result-struct size (16 vs 12), the reader (_readTickResult vs _readLiveTickResult) and one error string. About 120 lines exist twice. Any hardening of one path, like the GPU `finally` teardown that execute() got, has to be remembered for the other.

**Fix:** Add `CellExecutor.prototype._tickWith = async function (cellId, resultSize, reader)`, which throws "Cell X not loaded" when the entry is missing and dispatches to `_tickBindgenWith(entry, reader)` or `_tickRawWith(entry, resultSize, reader)`. Build those two from the current _tickBindgen and _tickRaw, with TICK_RESULT_SIZE replaced by `resultSize` and `this._readTickResult(...)` by `reader.call(this, ...)`. Then `tick = function (cellId) { return this._tickWith(cellId, TICK_RESULT_SIZE, this._readTickResult); }`, and tickLive does the same with LIVE_TICK_RESULT_SIZE and _readLiveTickResult. Delete the four per-kind path functions, first grepping executor-worker.js and executor-bridge.js to confirm nothing calls them by name. Use one generic alloc-failure message: "ironpad_alloc failed for tick return struct".

**Tests:** public-notebooks.spec.ts and execution.spec.ts run the simulation and LiveView public notebooks through both tick kinds. Add a node test in tests/js with a fake raw entry exporting a 0-arg cell_tick that returns a pointer to a hand-written TickResult and to a LiveTickResult, and assert both readers return the decoded shape and dealloc retptr with the right size.

### editor-7: Floored Saving-then-toast flow and collapsible appendix section duplicated across panels

*dry · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-app/src/pages/notebook_editor/shared_editor_panel.rs:128` (also `crates/ironpad-app/src/pages/notebook_editor/shared_editor_panel.rs:17`, `crates/ironpad-app/src/pages/notebook_editor/metadata_panel.rs:33`, `crates/ironpad-app/src/pages/notebook_editor/metadata_panel.rs:237-258`, `crates/ironpad-app/src/pages/notebook_editor/shared_editor_panel.rs:42-62`, `crates/ironpad-app/src/pages/notebook_editor/metadata_panel.rs:63-83`, `crates/ironpad-app/src/components/view_only_notebook.rs:569-588`)

**Problem:** `MIN_SAVING_MS = 500` is defined twice. The block that sets saving, runs `persist_notebook_durable`, sleeps the remainder, does a `try_set(false)` and toasts is copied line for line between SharedEditorPanel and NotebookMetadataPanel. The collapsed-by-default section markup (view-only-shared-section, header button with Chevron + IconLabel, lazily mounted body) is written out three times: SharedEditorSection, NotebookMetadataSection and the viewer's SharedAppendixSection.

**Fix:** Add to state.rs: `pub(super) fn persist_with_saving_floor(state: &NotebookState, saving: RwSignal<bool>, on_done: impl FnOnce() + 'static)`. It owns the single `MIN_SAVING_MS`. The hydrate arm does set(true), spawn_local, durable persist, sleep the remainder via `run_flow::sleep_ms`, `saving.try_set(false)`, `on_done()`. The SSR arm does `persist_notebook` then `on_done()`. Both panels call it with their dispatch_saved_toast. Add `components/collapsible_section.rs` exporting `#[component] pub fn CollapsibleSection(icon: IconData, label: &'static str, children: ChildrenFn) -> impl IntoView`, rendering the `view-only-shared-section` / header / Chevron markup and `view-only-shared-body` around `children()` only while expanded. Use it from SharedEditorSection, NotebookMetadataSection and SharedAppendixSection, and export it from components/mod.rs.

**Tests:** Guarded by notebook-metadata.spec.ts and shared-appendix.spec.ts, which exercise expand plus Save plus toast. No new unit test is needed for a markup-only component. If helpful, add one e2e assertion that each section starts collapsed and mounts its body on click.

### editor-11: Share type tags hand-roll the positional projection; Push serializes the full notebook only to discard it

*dry · partial · value 2 · effort S*

**Where:** `crates/ironpad-app/src/pages/notebook_editor/sharing.rs:61` (also `crates/ironpad-app/src/components/executor.rs:359-378`, `crates/ironpad-app/src/pages/notebook_editor/sharing.rs:307`, `crates/ironpad-app/src/pages/notebook_editor/sharing.rs:189`, `crates/ironpad-app/src/pages/notebook_editor/state.rs:576-581`)

**Problem:** `flush_serialize_tags` rebuilds the one-tag-per-cell vector by hand. Its comment says it must match 'the same source the compile path hashes with', but it skips the `is_code()` gate that `executor::previous_cell_types` (documented as 'THE shared projection' for cache keys) applies. The function also bundles flush, embed-outputs + JSON serialize, and tag projection. Push (line 307) keeps only the tags and discards the outputs-embedded JSON, then `persist_notebook_durable` -> `save_draft_now(_, true)` embeds and serializes again. Save to Account computes tags and discards them (line 189).

**Verifier's correction:** The tag projection at sharing.rs:61-74 is hand-rolled and omits the `is_code()` gate of `executor::previous_cell_types` (executor.rs:359-378). The server hashes `cell_type_tags[..idx]` directly (server_fns.rs:1103), so the two must agree. They do agree today: only Code cells insert into cell_outputs (linux_cell.rs never writes it), and `is_code` includes shared cells. So this is a drift hazard rather than a bug. Push discarding the outputs-embedded JSON (sharing.rs:307) and Save to Account discarding the tags (189) are real waste, but on a user click. The split is low value.

**Fix:** Replace the manual tag block with `state.cell_outputs.try_with_untracked(|outputs| crate::components::executor::previous_cell_types(&nb.cells, nb.cells.len(), outputs)).unwrap_or_else(|| vec![String::new(); nb.cells.len()])`. Optional: split `flush_serialize_tags` into `share_tags(state, &nb)` and `outgoing_json(state, &mut nb, toaster, fail_title)`, so that Push calls `flush_and_read` + `share_tags` only and Save to Account calls `flush_and_read` + `outgoing_json` only.

**Tests:** Guarded by blob-cache.spec.ts and mutable-shares.spec.ts (share snapshots hit the cache). The projection is already unit-tested through `previous_cell_types`. No new test is needed beyond confirming the snapshot manifests still populate.

### editor-12: Display-panel JSON parse fallback and main-thread badge duplicated between editor and viewer

*dry · partial · value 2 · effort S*

**Where:** `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs:170` (also `crates/ironpad-app/src/components/view_only_notebook.rs:1065-1066`, `crates/ironpad-app/src/components/view_only_notebook.rs:1305-1310`, `crates/ironpad-app/src/pages/notebook_editor/export.rs:146`, `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs:188-199`, `crates/ironpad-app/src/components/view_only_notebook.rs:1346-1357`, `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs:136-142`, `crates/ironpad-app/src/components/view_only_notebook.rs:1318-1323`)

**Problem:** `serde_json::from_str::<Vec<DisplayPanel>>(json).unwrap_or_else(|_| vec![DisplayPanel::Text(json)])` (the backward-compat rule for display text) appears three times. export.rs parses without the fallback, so a legacy plain-text output vanishes from exports. The main-thread fallback badge, with its identical tooltip sentence, is written out in both CellOutputPanel and ViewOnlyOutput. The WidgetSink `(id, is_runnable)` projection is also duplicated.

**Verifier's correction:** The parse-with-fallback expression does appear three times (cell_output.rs:169-173, view_only_notebook.rs:1065-1066 and 1305-1310). The main-thread badge markup and its tooltip are duplicated (cell_output.rs:188-199, view_only_notebook.rs:1346-1357). The WidgetSink `(id, is_runnable)` projection is duplicated (cell_output.rs:136-142, view_only_notebook.rs:1318-1323). The export.rs:146 'vanishes' claim is only theoretical: the export reads `cell_display_texts`, which this session's executor always writes as a JSON panel array, so the legacy plain-text form cannot reach it there. Still worth a small helper for consistency.

**Fix:** Add `pub(crate) fn parse_panels(display_text: &str) -> Vec<DisplayPanel>` to components/output_render.rs: `serde_json::from_str(display_text).unwrap_or_else(|_| vec![DisplayPanel::Text(display_text.to_owned())])`. Use it at cell_output.rs:169, view_only_notebook.rs:1065 and 1305, and export.rs:146 (replacing `if let Ok(panels)` with a non-empty check). Add `#[component] pub(crate) fn MainThreadBadge() -> impl IntoView` in output_render.rs holding the one tooltip string, used by both panels. The `(id, is_runnable)` projection is optional: `fn sink_cells<C: PipingCell>(cells: &[C]) -> Vec<(String, bool)>`.

**Tests:** Add an output_render.rs unit test: `parse_panels` on a JSON array yields its panels, and on plain text yields one Text panel. Existing persisted-outputs.spec.ts and import-export.spec.ts guard rendering.

### server-12: Host and guest socket pumps are copied line for line

*readability · partial · value 2 · effort M*

**Where:** `crates/ironpad-server/src/ws.rs:151` (also `crates/ironpad-server/src/ws.rs:163`, `crates/ironpad-server/src/ws.rs:188`, `crates/ironpad-server/src/ws.rs:459`, `crates/ironpad-server/src/ws.rs:474`, `crates/ironpad-server/src/ws.rs:501`, `crates/ironpad-server/src/ws.rs:524`)

**Problem:** handle_host and handle_guest repeat three blocks: the channel-to-socket forwarder task (151-157 / 459-465), the read loop with an idle timeout and its `Ok(Some(Ok(msg)))` / idle-warn / Close handling (163-185 / 474-499), and the `select!` abort pair (188-191 / 501-504). They differ only in the timeout value, one log field and which handler is called. The guest copy also renames `permissions` to `perms` for no reason (471), and `handle_guest_message` takes a `_session_id` parameter it never uses (524); every caller and test passes it anyway.

**Verifier's correction:** The duplication is real but smaller than claimed. The forwarder task is identical (ws.rs:151-157 / 459-465), as is the select! pair (188-191 / 501-504). The read loops (163-185 / 474-499) differ in their timeout and in the log field (notebook_id vs client_id) inside the idle warn, so a `next_text` helper that logs internally would need a label parameter or would have to return an idle marker. That can be done, but it hides the per-role log context. The two small cleanups are clear wins: `handle_guest_message`'s `_session_id` (ws.rs:524) is never used, and `let perms = permissions;` (ws.rs:471) is a pointless alias. The value is modest.

**Fix:** 1. ws.rs: add `fn spawn_forwarder(mut sink: SplitSink<WebSocket, Message>, mut rx: mpsc::Receiver<Utf8Bytes>) -> JoinHandle<()>` and use it at 151 and 459.
2. Add `enum ReadEnd { Closed, Idle }` and `async fn next_text(rx: &mut SplitStream<WebSocket>, idle: Duration) -> Result<Utf8Bytes, ReadEnd>`. It loops internally, skips non-Text/non-Close frames, returns Closed on stream end, error or Close, and Idle on timeout. Each read loop becomes `loop { match next_text(&mut ws_receiver, T).await { Ok(t) => handle_*(...), Err(ReadEnd::Idle) => { tracing::warn!(<role field>, "... idle timeout ..."); break } Err(ReadEnd::Closed) => break } }`, so each role keeps its own log field.
3. Remove `_session_id` from `handle_guest_message` and its callers (ws.rs:493 and the test call sites), and drop the `perms` alias in favor of moving `permissions` directly.

**Tests:** Existing: tests/relay.rs `guest_idle_timeout_closes_stale_connection` (660), `host_first_frame_must_be_claim`, `relay_integration_round_trip` and `host_disconnect_ends_guest_session` cover both pumps end to end. The ws.rs handler tests cover handle_guest_message after the signature change. No new behavior, so no new test beyond keeping these green.

### server-14: github_callback repeats the same send, error_for_status, JSON decode and 502 block twice

*readability · partial · value 2 · effort S*

**Where:** `crates/ironpad-server/src/auth.rs:155` (also `crates/ironpad-server/src/auth.rs:189`)

**Problem:** The token exchange (155-179) and the user lookup (189-210) are each about 25 lines of nested matches that differ only in the request and a log label. The handler is hard to scan, and a fix to one copy (a timeout, a status mapping) can easily be missed in the other.

**Verifier's correction:** Accurate: the token exchange (auth.rs:155-179) and user lookup (189-210) share the same send -> error_for_status -> json -> 502 shape, and their 502 bodies already follow a 'GitHub {what} failed' pattern. The value is lower than implied. There are exactly two copies, both in one function, and nothing can test the helper without mocking GitHub (the URLs are hard-coded), so the benefit is readability only. Worth doing if someone is touching auth.rs anyway; not urgent.

**Fix:** In auth.rs, add `async fn github_json<T: serde::de::DeserializeOwned>(req: reqwest::RequestBuilder, what: &'static str) -> Result<T, Response>`. It sends, applies `error_for_status`, decodes JSON, and on either failure logs `tracing::error!(error = %e, what, "GitHub request failed"/"GitHub response malformed")` and returns `(StatusCode::BAD_GATEWAY, format!("GitHub {what} failed")).into_response()`. In github_callback: `let token: TokenResponse = match github_json(state.http.post(...).header(...).form(...), "token exchange").await { Ok(t) => t, Err(r) => return r };`, and the same for `GithubUser` with "user lookup". The response bodies stay byte-identical; the log messages gain a `what` field.

**Tests:** Existing auth tests (`callback_rejects_a_state_mismatch`, `github_redirect_requires_configuration`) run before any network call and still pass. The helper's network path cannot be unit-tested without injecting base URLs; do not add that seam for this refactor.

### compiler-13: BuildAdmission also owns the BrowserPod key rate limiter

*soc · partial · value 2 · effort S*

**Where:** `crates/ironpad-app/src/compiler/admission.rs:94` (also `crates/ironpad-app/src/compiler/admission.rs:36`, `crates/ironpad-app/src/compiler/admission.rs:51`, `crates/ironpad-app/src/compiler/admission.rs:146`, `crates/ironpad-app/src/compiler/admission.rs:166`, `crates/ironpad-app/src/compiler/admission.rs:213`, `crates/ironpad-app/src/compiler/admission.rs:246`, `crates/ironpad-app/src/server_fns.rs:929`)

**Problem:** The key-fetch limiter's own doc says 'The two are unrelated costs', yet it lives inside `BuildAdmission` as four extra fields (`key_buckets`, `key_rate_burst`, `key_rate_per_sec`) plus a `with_key_rate` builder. The key/burst/per-sec triple is passed loose into the free `take_token`. `new()` silently hard-codes the key defaults. `DEFAULT_RATE_PER_MIN` (line 51) is stranded below the key constants, away from `DEFAULT_RATE_BURST` (34), whose doc describes both.

**Verifier's correction:** Mostly true. The key limiter lives in BuildAdmission as key_buckets/key_rate_burst/key_rate_per_sec plus with_key_rate (admission.rs:99-107, 168-172). `new()` silently uses the key defaults (160-161). take_token takes the loose (table, burst, per_sec) triple (246-251). DEFAULT_RATE_PER_MIN (line 51) sits undocumented right under DEFAULT_KEY_RATE_PER_MIN, whose doc makes it look like a pair; that misplacement is a real readability bug. The shared free function is already deliberate and documented (240-245), so the drift risk the finding implies is low. Moving the key bucket into its own context type is scope creep: it adds server wiring (provide_context, tests) for no behavioral gain. The struct grouping inside BuildAdmission is the right size.

**Fix:** 1. Move `const DEFAULT_RATE_PER_MIN: f64 = 30.0;` up to sit directly under DEFAULT_RATE_BURST (line 34), whose doc already describes both. 2. Add `struct TokenBuckets { table: Mutex<HashMap<String, Bucket>>, burst: f64, per_sec: f64 }` with `fn new(burst: f64, per_min: f64) -> Self` and `fn take(&self, client_ip: &str) -> bool` (take_token's body, moved as-is together with its doc). 3. BuildAdmission's fields become `builds: Arc<TokenBuckets>` and `keys: Arc<TokenBuckets>`, replacing buckets/key_buckets/rate_*/key_rate_*. try_take_token becomes `self.builds.take(ip)`, try_admit_key calls `self.keys.take(ip)`, and with_key_rate becomes `self.keys = Arc::new(TokenBuckets::new(burst, per_min))`. Keep it inside BuildAdmission; no new context type.

**Tests:** Existing: admission.rs key_requests_are_rate_limited_per_client (374), key_and_build_budgets_are_independent (402), plus the build rate/sweep tests. They cover both buckets' behavior and should pass unchanged. No new test required; optionally add one asserting `BuildAdmission::new(..)` gives the key bucket DEFAULT_KEY_RATE_BURST tokens, to pin the default.

### editor-10: NotebookContent is a ~780-line component with the whole toolbar and two inline workflows

*soc · partial · value 3 · effort S*

**Where:** `crates/ironpad-app/src/pages/notebook_editor/mod.rs:633` (also `crates/ironpad-app/src/pages/notebook_editor/mod.rs:845-882`, `crates/ironpad-app/src/pages/notebook_editor/mod.rs:957-984`, `crates/ironpad-app/src/pages/notebook_editor/sharing.rs:1-7`, `crates/ironpad-app/src/pages/notebook_editor/pipeline.rs:3-7`)

**Problem:** mod.rs:633-1080 is about 450 lines of toolbar view (Run All, Push + indicator, session, hamburger menu with nested mode-dependent arms, gear menu, close) inside NotebookContent, alongside the cell list, Sortable wiring, reactive scheduling and the view-mode swap. Two workflows are still written inline in the view: Export HTML (flush + serialize + download) and local Delete (confirm + IndexedDB delete + navigate, with a `window().unwrap()`). pipeline.rs:3-7 and sharing.rs:4-7 both say this is where the shipped bugs lived ('workflow logic written inline in view code'), and those two flows are the ones left behind.

**Verifier's correction:** The toolbar is indeed inline in NotebookContent (mod.rs ~633-1080). Export HTML (mod.rs:845-882) and local Delete (mod.rs:957-984, with `web_sys::window().unwrap()`) are the only workflows still written in view code. sharing.rs:1-7 explicitly frames the inline-workflow pattern as the historical bug source. Moving those two workflows is clearly worth doing. The ~450-line `NotebookToolbar` extraction is M-effort churn with no bug behind it, so it is optional at this point.

**Fix:** Required: add `pub(super) fn export_html_current_notebook(state: &NotebookState)` to sharing.rs. It uses `state.flush_and_read()` (editor-3), `cell_display_texts.try_get_untracked()`, `export::build_export_html` and `export::trigger_html_download`. Add `pub(super) fn delete_local_current_notebook(state: &NotebookState, navigate: impl Fn(&str, NavigateOptions) + 'static)`, which confirms via `web_sys::window().is_some_and(|w| w.confirm_with_message(..).unwrap_or(false))` (no unwrap), then spawn_local, delete_notebook and navigate("/"). Update the module doc list at sharing.rs:1-2. The two menu buttons then only call these. Optional follow-up: extract `notebook_editor/toolbar.rs::NotebookToolbar`.

**Tests:** Guarded by import-export.spec.ts (Export HTML) and notebook.spec.ts/home.spec.ts (local delete). If local Delete lacks coverage, add a Playwright case: accept the confirm dialog, assert navigation to / and that the notebook is gone from the Local list.

### server-fns-15: server_fns.rs is a 2,240-line production module spanning six unrelated domains

*soc · partial · value 2 · effort S*

**Where:** `crates/ironpad-app/src/server_fns.rs:1` (also `crates/ironpad-app/src/server_fns.rs:27`, `crates/ironpad-app/src/server_fns.rs:574`, `crates/ironpad-app/src/server_fns.rs:719`, `crates/ironpad-app/src/server_fns.rs:941`, `crates/ironpad-app/src/server_fns.rs:1264`, `crates/ironpad-app/src/server_fns.rs:2052`, `crates/ironpad-app/src/server_fns.rs:2224`, `crates/ironpad-app/src/server_fns.rs:2344`)

**Problem:** server_fns.rs has 2,240 lines of production code (4,343 with tests) covering six unrelated domains: the compile/check pipeline; public notebook listing; immutable shares plus the blob-snapshot store and its filesystem helpers (atomic_write_async, dir_total_bytes, share path fns); the mutable/account-share lifecycle and access gates; the admin panel; and auth info. Their tests all share one module. Domain-private helpers are visible to every other domain, and finding a function means scrolling past the rest. The module doc (1-11) still describes only the first three domains.

**Verifier's correction:** The size and domain claims hold. The file is 4,343 lines, with section markers at 27, 404, 574, 719, 879, 894, 941, 1264, 2052, 2224 and tests from 2241. The module doc (1-11) lists only the compile, public and shared endpoints. The caveat is also verified: server_fn_macro-0.8.10 lib.rs:502-518 derives the default endpoint from `xxh64(CARGO_MANIFEST_DIR:module_path!())`, so moving fns changes every endpoint URL. That is worse than the finding suggests. A tab left open across the deploy would have its draft autosaves fail ('Draft not saved; retrying' forever) until it reloads, which risks losing typing on the primary storage path. Weighed against Minimal Changes, the file is already well sectioned, admin_fns_are_all_gated's include_str! scan is coupled to it, and the split is a large move with no behavioral payoff. It should not be done now. Fixing the stale module doc is cheap and worthwhile on its own.

**Fix:** Now: rewrite the server_fns.rs module doc (lines 1-11) to list all current domains (compile/check, public notebooks, immutable shares and blob snapshots, toolchain fingerprint and BrowserPod key, mutable/account shares, admin, auth info), and add a sentence that the default #[server] endpoint URL is derived from this module path, so moving a fn to another module changes its URL. Later, if a split is wanted: first pin `#[server(endpoint = "<current name>")]` on every #[server] fn in a separate deploy, then split into server_fns/{compile,public,shares,mutable,admin,auth}.rs with `pub use` re-exports in server_fns/mod.rs, move each domain's tests, and point admin_fns_are_all_gated at admin.rs.

**Tests:** The doc-only change needs no test. For a future split, admin_fns_are_all_gated must be repointed and re-verified by injecting an ungated fn and watching it fail, and the Playwright suite (mutable, account-notebooks, admin specs) guards the endpoint wiring. Add a test that asserts the pinned endpoint paths of the autosave and push fns so a later move cannot silently change them.

### scss-1: Single-line ellipsis and surface-card declaration triples repeated across the stylesheet

*dry · partial · value 2 · effort S*

**Where:** `style/main.scss:519` (also `style/main.scss:2397`, `style/main.scss:2950`, `style/main.scss:3205`, `style/main.scss:3254`, `style/main.scss:3921`, `style/main.scss:4049`, `style/main.scss:448`, `style/main.scss:882`, +6 more)

**Problem:** `overflow: hidden; text-overflow: ellipsis; white-space: nowrap;` appears as a unit at 7 sites. `background: var(--ip-bg-surface); border: 1px solid var(--ip-border); border-radius: var(--ip-radius-md);` appears at 8 sites, plus 5 more with --ip-radius-lg (962, 2416, 2439, 2905, 3858). The file already uses mixins (ip-button-shape, ip-icon-button-shape) to single-source shared geometry, and these are the next obvious candidates.

**Verifier's correction:** The counts check out. The ellipsis triple appears at main.scss:519, 2397, 2950, 3205, 3254, 3921 and 4049. The surface triple with --ip-radius-md appears at 10 sites (446, 882, 1183, 1214, 1228, 1421, 3399, 3522, 4514, 4762), and with --ip-radius-lg at 962, 2416, 2439, 2905 and 3858. The ellipsis mixin is a standard SCSS idiom and a clear, zero-risk win. The surface triple is less clear-cut: the three tokens co-occur but are independent design decisions, and some sites will override one of them, so a surface mixin adds indirection for little gain. The value of either is low.

**Fix:** Add `@mixin ip-ellipsis { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }` beside ip-touch-target near main.scss:259, and replace the triple at the seven sites with `@include ip-ellipsis;`. Keep each site's neighbouring declarations (min-width: 0, etc.) in place. Add ip-surface only at sites that are unambiguously a surface card, and only if Aaron wants it.

**Tests:** There is no behavioural change. Compile style/main.scss before and after the change and diff the CSS output, which should be identical apart from declaration order within each rule. css-vars-check in `cargo make ci` still runs, and responsive.spec.ts, which measures truncation widths, covers the rendered result.

## Wave 5: test-code duplication

Worth doing when the surrounding production code is already open, not on its own.

### compiler-7: E2E compile tests repeat about 60 lines of scaffold/build/match boilerplate 17 times and hard-code feature flags

*dry · partial · value 3 · effort S*

**Where:** `crates/ironpad-app/src/compiler/mod.rs:702` (also `crates/ironpad-app/src/compiler/mod.rs:830`, `crates/ironpad-app/src/compiler/mod.rs:887`, `crates/ironpad-app/src/compiler/mod.rs:965`, `crates/ironpad-app/src/compiler/mod.rs:1045`, `crates/ironpad-app/src/compiler/mod.rs:1103`, `crates/ironpad-app/src/compiler/mod.rs:1167`, `crates/ironpad-app/src/compiler/mod.rs:1223`, `crates/ironpad-app/src/compiler/mod.rs:1282`, +12 more)

**Problem:** Every e2e test hand-writes a 10-argument `scaffold_micro_crate`, a 9-argument `build_micro_crate` and a `match BuildResult { Failure => panic!(tail…) }`. Most feature flags are hard-coded: autodiff `true` at 838, atomics `true` at 972, simd `true` at 1112. That bypasses the production detection the tests are meant to cover, although the fearless_simd test (1022-1029) points out that deriving it is the correct way. The eight `&stdout[stdout.len().saturating_sub(2000)..]` panics slice bytes, so a multi-byte character at the cut would replace the real failure with a slicing panic (the gate test at 1988-1991 already uses a char-safe tail). Also, the PRD-0031 T-004 doc block (753-772) is orphaned onto the autodiff test instead of `compile_cell_with_host_imports_links_successfully`.

**Verifier's correction:** Mostly true. There are 17 build_micro_crate calls in e2e_tests, and 16 sites use the byte-slicing tail `&stdout[stdout.len().saturating_sub(2000)..]`, which panics on a char boundary and hides the real failure. The gate test (mod.rs:1988-1991) already has a char-safe tail. The PRD-0031 T-004 doc block (mod.rs:753-772) sits on compile_cell_with_std_autodiff_builds_successfully instead of compile_cell_with_host_imports_links_successfully (1198), so rustdoc merges two unrelated docs. Some hard-coded flags (autodiff true at 838, simd true at 1112) are fair targets for detection, but a full build_cell/expect_built rewrite of 17 slow #[ignore] tests is churn for moderate value. The high-value parts are the misplaced doc, the panicking tail and the shared tail helper.

**Fix:** 1. Move the doc lines at mod.rs:753-772 (the PRD-0031 T-004 regression block) to sit directly above `compile_cell_with_host_imports_links_successfully` at mod.rs:1198. 2. In `e2e_tests`, add `fn tail(s: &str, n: usize) -> &str { let start = s.char_indices().rev().nth(n.saturating_sub(1)).map_or(0, |(i, _)| i); &s[start..] }`. Replace all 16 `&x[x.len().saturating_sub(N)..]` sites with it, and make the gate test's closure at 1988 call it too. 3. Optional, same PR: add `fn expect_built(result: BuildResult, label: &str) -> (PathBuf, Option<String>, String)` that panics with `tail(..)` output. Derive feature flags with CellFeatures::detect (compiler-1) in tests that are not deliberately pinning a flag. Skip the full `build_cell` wrapper unless compiler-10 lands at the same time.

**Tests:** These are the tests. Run `cargo make test-integration` to confirm all 17 still pass. For `tail`, add a unit test (non-ignored) with a multi-byte string cut mid-char, asserting no panic and the right suffix.

### server-6: AppConfig / AppState test fixtures are hand-built in 10 places across three crates

*dry · confirmed · value 3 · effort S*

**Where:** `crates/ironpad-server/src/ws.rs:611` (also `crates/ironpad-server/tests/relay.rs:31`, `crates/ironpad-server/tests/unpublished_notebooks.rs:86`, `crates/ironpad-app/src/server_fns.rs:2445`, `crates/ironpad-app/src/server_fns.rs:2519`, `crates/ironpad-app/src/server_fns.rs:2633`, `crates/ironpad-app/src/server_fns.rs:2748`, `crates/ironpad-app/src/server_fns.rs:2817`, `crates/ironpad-app/src/server_fns.rs:2859`, +1 more)

**Problem:** There are ten struct literals of every AppConfig field (data_dir, cache_dir, port, ironpad_cell_path, compilation_proxy, public_url, admin_login, browserpod_key). Three of them are also wrapped in the same `AppState { leptos_options: LeptosOptions::builder().output_name("ironpad-test").build(), .. }`. Every new config field has to be added to all ten; admin_login and browserpod_key were each added this way.

**Fix:** 1. ironpad-common/src/config.rs: add `#[doc(hidden)] #[must_use] pub fn for_tests(dir: &std::path::Path) -> AppConfig` returning data_dir = cache_dir = dir, port 0, ironpad_cell_path = dir.join("nonexistent-ironpad-cell"), compilation_proxy None, public_url "http://localhost", admin_login None, browserpod_key None. The doc comment says it is test support and why it must be pub (tests/ and other crates cannot see cfg(test)).
2. ironpad-server/src/state.rs: add `#[doc(hidden)] pub fn AppState::for_tests(config: AppConfig, ws: WsState) -> AppState` with the `output_name("ironpad-test")` LeptosOptions.
3. Replace: ws.rs:611 test_state with `AppState::for_tests(AppConfig::for_tests(Path::new("/tmp")), WsState::default())`; relay.rs:31 state_with_ws with `AppState::for_tests(AppConfig::for_tests(Path::new("/tmp")), ws)`; unpublished_notebooks.rs:86 with `AppState::for_tests(AppConfig { cache_dir, ..AppConfig::for_tests(&data_dir) }, WsState::default())`; the six server_fns.rs literals with `AppConfig::for_tests(cache.path())` / `for_tests(dir.path())`. The ironpad-common config.rs:78 helper becomes `AppConfig { public_url: public_url.into(), ..AppConfig::for_tests(Path::new("/data")) }`, which is fine because its tests only read public_url.

**Tests:** This is a pure test refactor. All the rewritten suites (ws.rs tests, relay.rs, unpublished_notebooks.rs, the server_fns compile/admission tests, ironpad-common config tests) must stay green. No new test is needed beyond `cargo make test`, plus `cargo make clippy` because pedantic `must_use` applies to the new pub fns.

### server-15: WS handler tests and relay integration tests rebuild the same host/session/guest setup more than 15 times

*dry · partial · value 2 · effort M*

**Where:** `crates/ironpad-server/src/ws.rs:636` (also `crates/ironpad-server/src/ws.rs:680`, `crates/ironpad-server/src/ws.rs:797`, `crates/ironpad-server/src/ws.rs:898`, `crates/ironpad-server/src/ws.rs:1062`, `crates/ironpad-server/src/ws.rs:1173`, `crates/ironpad-server/tests/relay.rs:159`, `crates/ironpad-server/tests/relay.rs:357`, `crates/ironpad-server/tests/relay.rs:427`, +3 more)

**Problem:** About 12 ws.rs unit tests start with the same ~15 lines: an `mpsc::channel::<axum::extract::ws::Utf8Bytes>(WS_CHANNEL_BOUND)` for the host, register_host, create_session and register_guest. relay.rs repeats a ~25-line block six times: connect_async to /ws/host, send_claim, send CreateSession, then match SessionCreated for the token. That is roughly 300 lines of boilerplate, and it hides what each test actually exercises.

**Verifier's correction:** The boilerplate is real: 17 register_host and 15 create_session calls in ws.rs tests, with the same host-channel/session/guest-channel prelude (e.g. 636-656, 680-697), and the repeated connect -> send_claim -> CreateSession -> SessionCreated block in relay.rs (164, 362, 432, 495, 562). The relay.rs claim of six copies overstates it: 731 skips the handshake deliberately, and 760/782 use two hosts with different secrets, so they are not the shared shape. It is test-only readability with no behavioral risk, and at about 300 lines of churn its priority is low.

**Fix:** ws.rs tests: add `async fn hosted_session(state: &AppState, perms: Permissions) -> (mpsc::Receiver<Utf8Bytes> /*host*/, String /*session_id*/, mpsc::Receiver<Utf8Bytes> /*guest-1*/)`, which registers host "nb-1"/"conn-1", creates the session and registers guest "guest-1". Replace the prelude in the tests that use exactly that shape; leave tests with multiple guests or sessions explicit. relay.rs: add `async fn connect_host_with_session(base: &str, notebook_id: &str, secret: &str, perms: Permissions) -> (SplitSink, SplitStream, String /*session_id*/, String /*token*/)` and use it at 164, 362, 432, 495 and 562. Leave 731 (no handshake) and 760/782 (two hosts) as they are.

**Tests:** This is the test refactor itself. Every rewritten test must still pass, and it is worth confirming each one still fails if its asserted behavior is broken. For example, temporarily removing the read gate in broadcast_to_notebook_guests should still fail `read_denied_guest_does_not_receive_content_events`.

### cell-tests-1: lib.rs tests repeat the unsafe CellResult reclaim-and-decode block about thirty times

*dry · confirmed · value 2 · effort S*

**Where:** `crates/ironpad-cell/src/lib.rs:1480` (also `crates/ironpad-cell/src/lib.rs:1590`, `crates/ironpad-cell/src/lib.rs:1630`, `crates/ironpad-cell/src/lib.rs:1707`, `crates/ironpad-cell/src/lib.rs:1759`, `crates/ironpad-cell/src/lib.rs:1800`, `crates/ironpad-cell/src/lib.rs:1839`, `crates/ironpad-cell/src/lib.rs:1880`, `crates/ironpad-cell/src/lib.rs:1946`, +1 more)

**Problem:** There are 29 inline `unsafe { drop(Vec::from_raw_parts(p, n, n)) }` blocks and 10 copies of the `String::from_utf8_lossy(slice::from_raw_parts(display_ptr, display_len))` + `serde_json::from_str` decode, about 250 lines of repeated unsafe code. Every new FFI test copies it again, and a test that forgets one reclaim leaks memory without anyone noticing.

**Fix:** In lib.rs `mod tests`: add `struct Taken { bytes: Vec<u8>, panels: Option<Vec<DisplayPanel>>, tag: Option<String> }` and `fn take(r: CellResult) -> Taken`. For each non-null field, take ownership with `unsafe { Vec::from_raw_parts(ptr, len, len) }`, the same contract as ironpad_dealloc and vec_into_raw's comment. panels comes from serde_json::from_slice of the display bytes, and tag from String::from_utf8. Add `unsafe fn reclaim(ptr: *mut u8, len: usize) -> Vec<u8>` (empty Vec for null or 0) for the TickResult/LiveTickResult tests. Convert the round-trip tests to `let t = take(output.into());` followed by assertions on t.

**Tests:** Test-only refactor. `cargo make test` for ironpad-cell must stay green with the same test count. Optionally run the ironpad-cell tests once under Miri, or with a leak-checking allocator, to confirm the helper reclaims everything.

## Refuted

### compiler-11: Simulation and LiveView lib.rs templates are near-identical copies

**Why refuted:** The templates are not near-identical in the way a shared template needs. generate_simulation_lib_rs (scaffold.rs:746-797) and generate_live_view_lib_rs (805-856) differ in the static name, the local identifiers (sim/first_frame/frame vs view/first_content/content), the multi-line meta construction (SimulationMeta with width/height/sliders vs LiveViewMeta), and the tick result type. A TickKind-parameterized template would substitute five or more fragments into a format string, so a reader could no longer see the emitted Rust as plain text. The requirement to 'keep the emitted text byte-identical' forces every identifier to be a parameter, and that makes it worse. The duplicated static_mut_refs comment is three lines in generated code. Two explicit 40-line templates are more readable than one templated template, and the Minimal Changes rule tips this to not worth it.

### server-11: Relay fully deserializes (and, because of flatten, double-buffers) every frame just to read id and type

**Why refuted:** There are no measurements, and the proposed fix goes against the constitution rather than improving on it. Relay frames are small protocol messages. The double parse from `#[serde(flatten)]` (protocol.rs:65) is real, but it is microseconds against a network hop, and nobody has profiled the relay (CLAUDE.md: measure with criterion before any non-obvious optimization). The fix adds a relay-private `Category` enum that mirrors the five `MessageKind` variants, which is a second hand-synced copy of the category list, the kind of duplication this review is removing elsewhere. It also needs a second check_permission (sessions.rs:225) keyed on it. And it changes behavior: frames with malformed payloads would be forwarded to the host instead of dropped at the relay, weakening validation at the boundary. Parsing fully at the relay is how it validates what it routes, and protocol.rs's forward-compat doc (drop at the decode site) relies on that.

## Appendix: partition notes and lower-value observations

These were the reviewers' overflow beyond their top 15. Nobody verified them, so check each one before acting on it.

### compiler

The compiler partition is well tested and heavily commented. The main debt is the loose parameters threaded between stages: source/cargo/shared inputs go to hash and scaffold, and the atomics/autodiff/simd bools go to hash, build, check and RUSTFLAGS. That repeats the feature detection at five call sites, spreads `too_many_arguments` allows everywhere, and makes about 50 test hash calls spell out `false, false, false`. A `CellFeatures` struct derived inside the hash fixes most of it with byte-identical keys. The other themes are blocking std::fs on the async compile/check paths, where one comment claims an async-fs invariant the code one call earlier breaks, and several PRD-0067-era comments that still describe four toolchains, including the CACHE_EPOCH caveat that names pins which no longer exist.

- scaffold.rs:107-137: the four crate-root gates each do `lib_rs.insert_str(0, ..)` + `preamble_lines += 1` with the 'same preamble bump rule' comment repeated. A `[(bool, &str); 4]` table joined once would read better; keep the current reverse order so output stays byte-identical.
- build.rs:326-332 logs full cargo stdout (JSON for every dependency artifact) and stderr at WARN on every failed compile, and server_fns.rs:343-351 logs stderr at WARN again. Every user typo produces duplicate, potentially large OTel events; log a stdout tail once.
- cache_key.rs:323 `extract_user_dependencies` has no production caller (only tests). It stays pub and is re-exported from scaffold.rs:15-18 solely for tests; the scaffold.rs dependency-parser tests (887-963, 1500-1591, 2263-2288) exercise ironpad-common functions from ironpad-app and would belong in cache_key.rs's test module.
- build.rs:845-854 and 1088-1107: two tests parse BROWSERPOD_NIGHTLY out of docker/browserpod.env with identical code; extract a helper. build.rs:758-791 (`every_cell_build_pins...`, `normal_and_simd...`) are fully subsumed by `every_feature_combination_lands_on_the_one_cell_pin` (793-811).
- scaffold.rs:1493 doc says the test `tempdir()` is 'cleaned up on drop', but it returns a PathBuf that is never removed. The same hand-rolled helper exists at mod.rs:592 and mod.rs:621; `tempfile::tempdir()` is already used in cache.rs/optimize.rs. The e2e dirs are kept on purpose for debugging per CLAUDE.md, but the scaffold and pipeline ones are not.
- blob_cache.rs:187 clones the whole JS glue String per store (`response.js_glue.clone()`) to pass it to `js_put_blob`; `Option<&str>` would avoid the copy. blob_cache.rs:47-61 has no single-flight, so concurrent first Runs each fetch the fingerprint.
- server_fns.rs:1064 `dir_total_bytes` walks the whole share-blobs directory on every share/push to enforce the cap. It is O(files) per editorial action and could be cached or tracked incrementally.
- diagnostics.rs:117-118 fully deserializes every cargo stdout line (including hundreds of compiler-artifact records) into `CargoMessage`; a `line.contains("compiler-message")` prefilter would skip them. This path is cold relative to the cargo run, so it is only worth doing alongside other diagnostics work.
- diagnostics.rs:289 test comment references a removed `WRAPPER_PREAMBLE_LINES` constant; mod.rs:431 comment says 'preamble of 5' for a base that is now 7.
- server_fns.rs:1074-1077 share snapshot says it must 'Mirror the editor's CompileRequest exactly (cell_item.rs)'. That mirror is enforced by hand across pipeline.rs:284, view_only_notebook.rs:804 and linux_cell.rs:456. A `CompileRequest::for_cell(notebook, idx, tags)` constructor in ironpad-common would make it structural.

### server-fns-and-db

The partition is in good shape: the access gates, quota admission and schema migration are deliberate, well documented and tested. The main weaknesses are rules restated at several call sites instead of derived from one: the owner gate, upload validation, the cache-key feature flags, the path-segment check and the public-name mapping. The largest cost is DB scans on the per-autosave admission path, which grow with the whole instance's stored bytes. Two incidental bugs are worth fixing soon: the session-renewal conflict that makes concurrent requests resolve as anonymous, and Linux-cell snapshot keys that can miss.

- server_fns.rs:1909: delete_mutable_share's doc says 'Unpublish a mutable share', but it deletes the share, and unpublish_mutable_share is the next function.
- db.rs:52-60: AdminUserRow duplicates ironpad_common::AdminUser field for field and is copied again at db.rs:426-433 and server_fns.rs:2120-2127. The db could return AdminUser directly.
- server_fns.rs:1632: on every signed-in home load, list_mutable_core fully deserializes each owned notebook (all cells, sources and saved outputs) just to read title, cells.len() and tags. A lightweight listing view in ironpad-common (cells as Vec<IgnoredAny>) would serve it and list_public_notebooks_core (596-611).
- server_fns.rs has 25 copies of `.map_err(|e| ServerFnError::new(e.to_string()))`. They can be `.map_err(ServerFnError::new)`, because ServerFnError::new takes impl ToString (server_fn 0.8.11 error.rs:201).
- server_fns.rs:108-111 and 925-928 repeat the same client-IP extraction block, and its "local" fallback duplicates admission.rs:298. One request_client_ip() helper would cover both.
- og/mod.rs:397-403: write_atomic duplicates server_fns.rs:970-986 atomic_write_async, minus the temp-file cleanup on a failed rename, and compiler/cache.rs:133 has a third, synchronous copy.
- compiler/cache.rs:86: try_cache_hit reads multi-MB blobs with synchronous std::fs from async compile_cell_core (server_fns.rs:203) on every cached Run. The fix belongs to the compiler partition.
- db.rs:420-422: list_users_for_admin looks up each user's group counts with a linear scan, O(users x groups). A HashMap built from the two group vecs removes it.
- db.rs:609, 728, 762, 884 repeat `i64::try_from(len).unwrap_or(i64::MAX)`; one small helper would do.
- db.rs:979-1000 user_owns_share and 1115-1138 user_can_read_share are the same grant query with one role vs two. One has_grant(uid, id, &[roles]) could serve both.
- types.rs:1705-1717: CacheTierUsage.name and .valve_may_clear are derivable from .tier, and pages/admin.rs mixes the wire field (t.valve_may_clear, line 207) with tier.valve_may_clear() (line 90).
- server_fns.rs:1164-1171 and 1367 spell out `ShareManifest { version: 1, cells: BTreeMap::new() }` three times. A ShareManifest::new(cells) plus a version const would replace them and collapse the 1163-1172 match.
- server_fns.rs:721-725: MAX_SHARE_BYTES has two stacked doc comments split by #[cfg], and it now also caps account saves and drafts, so 'shared-notebook upload' misdescribes it.
- server_fns.rs:1074: the comment 'Mirror the editor's CompileRequest exactly (cell_item.rs)' is stale; the request has been built in pages/notebook_editor/pipeline.rs:284 since PRD-0055.
- server_fns.rs:1096: `cell.cargo_toml.clone().unwrap_or_default()` allocates per cell just to get a &str; `as_deref().unwrap_or("")` avoids it.
- sanitize.rs:417: a test comment cites 'sanitize.rs:41-71', which is a stale line reference.
- lib.rs:198 re-imports wasm_bindgen::JsCast, which lib.rs:94 already imports at module level.
- server_fns.rs:605/608: PublicNotebookSummary.id and .filename always hold the same value.
- server_fns.rs:1442-1450 -> 1514-1527: create_mutable_share_core's publish half re-reads the just-uploaded draft from the DB and parses it a second time.

### server-crate

The server crate is in good shape: modules are small, the reasoning is commented, and most earlier traps are already guarded by constants or tests. The main issue is copy-paste between neighbors: og and oembed each load notebooks by storage class, ws and state each end sessions and pump sockets, and the tests rebuild fixtures and routes that production defines in one place. On the relay hot path, every frame is still copied once and parsed twice for routing, and the 4 MiB frame cap rests on an assumption that NotebookGet responses break.

- Three random-hex/token-hash helpers: sessions.rs:238/247, app db.rs:1288/1298, server auth.rs:342. sessions' generate_token also passes random bytes through blake3::Hash::from_bytes just to hex-encode them, which misleadingly types them as a hash.
- site_root is rebuilt as an owned PathBuf per request via `Path::new(..).to_path_buf()` at crawl.rs:112, oembed.rs:189, og/mod.rs:453 and og/mod.rs:484, though every callee takes &Path. Add an `AppState::site_root() -> &Path` helper.
- http_policy.rs:105-110 re-derives the private server_fns.rs:953 `share_blobs_dir`, so the blob writer and the /share-blobs/ reader agree on the path only by hand. og/mod.rs:398 `write_atomic` re-implements server_fns.rs:970 `atomic_write_async` with a fixed tmp name and no cleanup when the rename fails.
- The browser heartbeat period (app session/connection.rs:281, 30_000 ms) and the server's HOST_IDLE_TIMEOUT (ws.rs:51, 2 min) are tied together only by comments. A shared constant in protocol.rs would let the server derive its timeout from the heartbeat.
- protocol::Message is built by hand at ws.rs:23 (wire_msg), tests/relay.rs:77 (to_json), daemon.rs:161, daemon.rs:788 and app connection.rs:271. A `Message::new(id, kind)` in ironpad-common would serve all of them.
- sessions.rs:69 `SessionStore::new()` does the same thing as the derived Default.
- sessions.rs:161 invalidate_by_connection collects the ids and then removes them one by one, while sweep_expired (207) does retain plus push. Use one private drain_where.
- The session expiry check is written `expires_at < now` in some places (sessions.rs:115, 127) and `expires_at > now` in others (182, 196, 212), so they disagree when the two are equal. Add a `Session::is_live(now)`.
- state.rs:277 forget_secret_if_idle clones every Session through all_sessions() just to run `.any(notebook_id ==)`. A `SessionStore::has_session_for(notebook_id)` would avoid the clones.
- state.rs:453 guest_can_read scans every guest and then clones the Session for each guest-originated Event. Permissions are fixed at connect time, so GuestHandle could carry the read flag.
- ws.rs:343 EndSession is not scoped to the requesting host's connection or notebook: any host socket that knows a session id can end it. Ids are UUIDs shown only to the owner, so this is hardening, not an exploit.
- main() (main.rs:44-313) is about 270 lines. The tracing setup (45-77) belongs in otel.rs and the sweeper loop (166-182) could become WsState::spawn_sweeper.
- The COOP and COEP header layers are inline literals in main.rs:267-274, while CSP lives in http_policy.rs. Move them next to CONTENT_SECURITY_POLICY.
- og/mod.rs:473-479 and 490-496 share an identical render_cached-to-PNG-or-500 tail; extract a card_response helper.
- oembed.rs:217 hard-codes "public, max-age=3600", which is the value of og/mod.rs:52 CACHE_CONTROL.
- The `/auth` mount prefix is written separately at main.rs:213, auth.rs:92 and auth.rs:327 (the cookie Path).
- ws.rs:402-404 clones id, notebook_id and permissions out of an owned Session; destructuring it would avoid the clones.
- The lib.rs:1-7 module doc describes the crate as only the relay, but it now also contains auth, crawl, oembed and og.
- og/text.rs:59-62 looks up the .notdef advance again for every character measured; it could be computed once in Font::parse.

### editor

The editor partition has already been consolidated in the right places (run_flow for acquisition and queue bookkeeping, notebook_ops for structural edits, sharing.rs for most lifecycle flows). The biggest remaining costs come from reactive reads written as `.get()`/`get_untracked()` on page-level collections: per-cell effects, the run pipeline, live check and the persist path all deep-clone maps and whole notebooks on hot paths, and the fixes are mechanical `.with()` rewrites. The one real correctness bug is the unconditional per-cell flush (editor-1), which re-stales every cell on any save, share or preview and re-runs the whole notebook in reactive mode. Most of the rest are DRY wins with small, local fixes.

- pipeline.rs:19-20,320-321,335-336,509-530: the not(hydrate) arms of wire_run_effect can never run, because Effect bodies only execute with reactive_graph's `effects` feature, which leptos enables only for hydrate/csr. They mirror the success bookkeeping (stale-clear, blame-clear, advance) and can drift. Gate the body on hydrate the way ViewOnlyCodeCell does (view_only_notebook.rs:783).
- cell_output.rs:107-147: CellOutputPanel's five #[prop(optional)] Option props and the '(the read-only viewer's usage)' comments are dead generality. The only caller is CellItem (cell_item.rs:1352), which passes all of them; the viewer uses ViewOnlyOutput.
- cell_item.rs:403,413,1381,1388,1414 and pipeline.rs:183,689 repeat matches!(Compiling|Running), and cell_item.rs:1192-1200 and 1213-1229 are parallel matches over CellStatus. Add is_busy()/css_suffix()/badge() methods on CellStatus in state.rs.
- cell_item.rs:794-875 vs 882-943: the source and Cargo.toml debounced savers are near-identical (handle signal, arena-held closure, cleanup, Callback). Extract a debounced-saver helper.
- cell_item.rs:260-292: on_move_up/on_move_down differ only in direction. Use one move_by(delta).
- cell_item.rs:107: selected_tab is a RwSignal<String> compared against "code"/"cargo-toml" literals, with to_string() on every click. Use a Copy enum.
- mod.rs:240-241: NotebookState.shared_source/shared_cargo_toml mirror notebook fields via an Effect. shared_source has one untracked reader (shared_editor_panel.rs:82), and pipeline.rs:296 vs 300-303 mixes the lagging mirror with the live notebook in one CompileRequest.
- metadata_panel.rs:145 and 165-169 re-implement IronpadNotebook::og_image_path / og_image_dimensions predicates (types.rs:547,583). Expose the predicates from ironpad-common.
- session/connection.rs:172-182 and 246-258 duplicate the forward-browser-events loop. model.apply buffers events with no session, so on_open replays up to 64 pre-session full-source mutations, the stale backlog pipeline.rs:50-54 says it avoids for execution events. Drain-and-discard in start_session before opening the socket.
- session/connection.rs:375-378: the comment says a client id is extracted from the message id prefix, but the code always uses ClientId::agent("remote").
- session/connection.rs:352: every request_cell_run refusal (view mode, Linux, not runnable) is reported as ErrorCode::CellNotFound.
- export.rs:169: markdown output panels use class ironpad-markdown-cell-preview and tables use ironpad-output-table, and EXPORT_CSS styles neither (only .markdown-content), so they render unstyled in the exported file.
- storage/client.rs:137 (and the js_search_notebooks binding at 34): search_notebooks has no callers.
- storage/client.rs:161: list_history silently swallows deserialize errors via unwrap_or_default, unlike every sibling, which logs.
- confirm dialog boilerplate appears 7x in the partition (sharing.rs:170,390,445,489; history.rs:157; mod.rs:965 and cell_item.rs:339, the last two with window().unwrap()). Add one confirm(msg) -> bool helper.
- sharing.rs:79 window_origin and metadata_panel.rs:89 published_origin are the same helper, and the /mutable/{id} URL is formatted in both (sharing.rs:268, metadata_panel.rs:269).
- mod.rs:1086: `state.cells.get().is_empty()` clones the manifest list for an emptiness check. Use with(Vec::is_empty).
- pipeline.rs:403: output bytes are held twice per cell, once in cell_outputs and once in execution_result.
- state.rs:128,141-142: field docs are stale. notebook is not only 'loaded from IndexedDB', and save_generation flushes into the model, not 'to the server'. pipeline.rs:30 still says '2,000-line component' for a 1,533-line file.
- Collapsed body_class Signal::derive is copied 4x (cell_item.rs:1044; view_only_notebook.rs:931,1137,1178).
- mod.rs:1019-1025 and 1050-1056 duplicate the IconLabel in both branches just to append a checkmark.

### components-and-pages

The components and pages are carefully commented and mostly well factored where it counted: run_flow, dismiss, icon and output_render each exist to kill an earlier fork. The remaining debt splits three ways. The view-only renderer does needless O(n^2) cloning on the SSR hot path. A shipped rail feature was never wired, so its status dots never change. And there are copy-paste clusters of markup and loop machinery that have not yet had their PRD-0059-style unification: the three rAF loops, four cell headers, six error panels and ten confirm dialogs.

- Viewer and Linux cell duplicate the compile-result-to-error recipe (empty blob -> join diagnostic messages; `Compile error: {e}`): view_only_notebook.rs:829-885 vs linux_cell.rs:508-528. Could be `run_flow::compiled_or_error(result) -> Result<CompileResponse, String>`. The `crossOriginIsolated` read is also duplicated (view_only_notebook.rs:122-128, linux_cell.rs:383-389).
- executor.rs:449/473: `source_of: impl Fn(&str) -> Option<String>` forces an owned String per cell. The viewer's closure (view_only_notebook.rs:731-733, :900) also does an O(n) `find` per cell, making each Run click and each failure O(n^2) plus a clone of every source. The editor side clones from its map (pipeline.rs:210, state.rs:421). Returning `Option<&'a str>` fixes both.
- Per-cell queue watchers call `run_all_queue.get()`, cloning the whole queue for every cell on every queue change (view_only_notebook.rs:155, :751, :760; editor cell_item.rs:387). `.with()` avoids the allocation.
- `format_cell_count` (home_page.rs:467) and `cell_count_label` (notebook_rail.rs:320) are the same pluralizer.
- Clipboard write is duplicated at copy_button.rs:273-275, session_panel.rs:288-294 and notebook_editor/sharing.rs:94. copy_button.rs:273 unwraps `window()`.
- The two-segment toggle markup and class conditional are duplicated between the header theme toggle (app_layout.rs:278-305) and the Cached/Fresh toggle (view_only_notebook.rs:361-374). The shared class naming is documented as deliberate; the duplicated markup is not.
- `LayoutContext.compiler_version` is an `RwSignal<String>` that is never written after construction (app_layout.rs:25, :38, :493). A `&'static str` read of CELL_TOOLCHAIN would do.
- Home cards format dates two ways side by side: local uses `%b %d, %Y` (home_page.rs:167), account a raw ISO slice `get(..10)` (home_page.rs:254). admin.rs:265 truncates with `chars().take(10)`.
- ViewOnlyNotebook's `embed_spec` round-trips Option -> "" -> Option (callers pass `.unwrap_or_default()` at public_notebook.rs:77 and shared_notebook.rs:82, then it is re-filtered at view_only_notebook.rs:103). `#[prop(default = None)]`, already used for `share_manifest` for exactly this reason, removes it.
- Run-all id collection is written twice in ViewOnlyNotebook (view_only_notebook.rs:173-179 and :192-198).
- mutable_notebook.rs:196 clones `edit.notebook` although `edit` is an owned clone dropped right after; move it instead.
- linux_cell.rs:252: `Outcome.status` is a stringly-typed state machine ("exited"/"finished"/"cancelled"/...) matched in four methods; an enum parsed once in `execute_in_pod` would be safer.
- `///` outer docs sit on `use` items instead of `//!` module docs: error_panel.rs:2-8, markdown_cell.rs:1-2, view_only_notebook.rs:1-4, monaco_editor.rs:1-9.
- Remaining `web_sys::window().unwrap()` in library code: animation_canvas.rs:145/215/289/301/411/506/518, live_view_panel.rs:58/106/222/234, app_layout.rs:462.
- Unmeasured: every SSR render of /public re-runs pulldown-cmark plus ammonia for each markdown cell (about half of all cells) of a static notebook. Worth profiling before caching.

### cell-runtime-and-common

ironpad-common is in good shape. The protocol has been deliberately hardened for forward compatibility, and NotebookMetaPatch / notebook_ops already show the one-definition pattern; the CellUpdate/CellUpdated field set is the only place that pattern hasn't been applied. ironpad-cell is correct but repetitive: From / IntoPanels / TypeTag triples and per-widget impls re-spell the same literals, and that drift has already cost one CACHE_EPOCH bump. Its contracts with the app (DisplayPanel, SimSliderMeta, the CellInputs format, widget configs) are mirrored by hand with no test binding the two sides. The real per-frame cost, and the one real bug, are on the tick paths: sim::emit's json! trees, and GpuSimulation::tick, which leaks GPU buffers and never reads back its output. Every behavior-changing fix here also needs a manual CACHE_EPOCH bump.

- types.rs:367 and types.rs:717 both contain the `cell_type == CellType::Code && !self.shared` rule, and the first calls itself "the single definition". Have both call one free fn, and add `From<&IronpadCell> for CellManifest` for the only projection site (model.rs:260).
- types.rs:1701-1807: the admin/CacheTier production types sit after `mod tests` and are followed by a second test module (`cell_type_tests`). `CacheTierUsage.name` and `.valve_may_clear` duplicate `tier.dir_name()` and `tier.valve_may_clear()` (server_fns.rs:2083-2088), and admin.rs reads the method at :90 but the field at :207.
- gpu.rs:176/178/180 pass buffer usages as bare 1/2/3 with explanatory comments, mirrored by the switch at executor-gpu.js:72-80. Named consts would make the ABI readable.
- lib.rs:776: `IntoPanels for GpuCanvas` returns placeholder text because rendering supposedly needs `self`, but `render_gpu`/`render_cpu_inner` only read fields. With `render(&self)`, a tuple like `(Table, GpuCanvas)` would show the image instead of the placeholder.
- plot.rs:606 `xml_escape` makes three `replace` passes and duplicates the crate's own `html_escape` (lib.rs:1145).
- lib.rs:385 `with_display(String)` duplicates `with_text(impl Into<String>)` (390), and the four display-only constructors (423-457) re-spell the struct literal instead of `Self::empty().with_*`.
- lib.rs:692: `IntoPanels::into_panels(&self)`, under a `wrong_self_convention` allow, makes the tuple impl (1066-1068) clone every String/Svg/Html/Table/CellOutput payload it has already destructured by value. A provided by-value method would avoid one copy per tuple output.
- plot.rs:218 + 521-544: `themify` plus the black-fill replace make nine full-string passes, each producing a copy of the SVG. This only matters for scatter plots with thousands of points; a single scan for `"#RRGGBB"` would do it.
- canvas.rs:102-119 and 178-216: `from_fn`/`to_bmp` push three bytes per pixel with a capacity check on each push. `from_fn` runs every frame in Simulation ticks; profile with criterion before changing.
- executor-gpu.js:180-213 hand-writes a second BMP encoder that mirrors canvas.rs:178-216 (it also sets biSizeImage, which Rust leaves at 0).
- Outside partition: cache_key.rs:67-71 does not hash the injected ironpad-cell source, so every behavior change in this partition needs a manual CACHE_EPOCH bump that is easy to forget.
- Outside partition: model.rs:255 `sync_from_notebook` calls `notebook.get_untracked()`, cloning the whole notebook with every cell source just to project manifests, on each label/shared/collapse change.
- Outside partition: executor-core.js:188-247 `_simRead`/`_simReadAll` duplicate memory/alloc resolution and the length-prefix framing that sim.rs decodes.

### cli-js-tools-config

The Rust side of this partition (CLI daemon, IPC framing, proxy) is well bounded and thoroughly tested. Its main structural debt is that the CLI and daemon hand-mirror protocol types instead of serializing them, which has already let CellType::Linux fall out of reach of the CLI. The JS executor layer has grown by copy-and-edit: tick variants, fallback arms, memory lookups and worker arms each exist two to four times, and storage.js does real work on hot paths it doesn't need (history reads on every autosave, full blob rewrites on every cache hit). The build config is mostly guarded by sync tests, but the wasm-bindgen CLI pin, the BrowserPod version in CI and the atomics RUSTFLAGS fall outside them, and the local warmup-atomics task is currently broken outright.

- public/storage.js:37-76: openDb() opens and closes a new IndexedDB connection on every call (save, getBlob, putBlob, ...); memoize one connection and close/reset it on `onversionchange`.
- crates/ironpad-cli/src/daemon.rs:58,298-301,313-316,818-821: pending oneshots carry the raw String, so forward_to_server re-parses a Message that handle_ws_message already parsed; send the parsed protocol::Message instead.
- crates/ironpad-cli/src/daemon.rs:54,157: `connected: RwLock<bool>` just duplicates `ws_tx.is_some()`; `pending` (58) is only ever write-locked, so a Mutex fits; the 10s timeout literal is repeated at 198 and 812.
- crates/ironpad-cli/src/daemon.rs:446-462 hand-builds the CellManifest JSON shape and crates/ironpad-app/src/model.rs:260-268 builds CellManifest field by field; a `From<&IronpadCell> for CellManifest` plus `#[serde(flatten)]` would serve both.
- public/executor-bridge.js:38-53,517-544 + public/executor.js:20-48: the host handlers and updateSimBus copy are 'intentionally duplicated' because there is 'no shared home', but script-loader.js entries are now ordered multi-file lists (script-loader.js:27) and the fallback chain already injects scripts in order (bridge 162-166), so a small main-thread executor-host.js could own them.
- public/executor-worker.js:206-249: four identical try/runWithPanicCapture/postMessage arms; a {type: [op, transferOf]} table would collapse them.
- public/executor-core.js:785,902,925: `if (useSret || retptr)` reduces to `if (retptr)`; the extra condition misleads.
- public/executor-core.js:312-316 builds GPU base64 by per-byte string concatenation (chunked String.fromCharCode.apply is faster); the BlobImage panel literal is duplicated at 336-343 and 347-354.
- public/executor-core.js:31,43,55: ABI struct sizes are hard-coded with no assertion against ironpad-cell's repr(C) structs, unlike the env-import table, which has a sync test.
- public/executor-glue.js:202-203: comment line duplicated; the rewriteBindgenGlue body (86-222) is deliberately left mis-indented.
- public/browserpod-runtime.js:856-859 re-implements finish()'s teardown in the catch arm; retain() (940) ignores its notebookId parameter; feed() calls lines.splice(0, ...) on every newline once at MAX_LINES (450-452).
- public/embed.js:15 doc lists only shared|public, but the regex (42) also accepts mutable; each mounted embed adds its own window 'message' listener (72).
- crates/ironpad-proxy/src/main.rs:51-54 allocates a lowercase hostname plus format!(".{domain}") per domain per CONNECT; the 502 response (154) is an inline literal beside the RESPONSE_* consts; on header-drain EOF/error (122) it still dials upstream.
- tools/capture-outputs.mjs:49,101-103,169: the runnable-cell predicate is written three times; tools/capture-outputs-check.py parses each notebook twice (47, 66).
- tools/lint-notebooks.py is referenced by no Makefile task, CI job or doc; its em-dash check scans Code-cell sources, contradicting the documented 'code comments are exempt' voice rule; it parses each file twice (210, 249).
- tools/warm-prod.sh:287-289: the Scope header omits the Linux lane added later; the URL-encode python one-liner is repeated (321, 330) while plain_body is hand-encoded.
- Makefile.toml: dead 'once T-004/T-045 creates...' fallback branches in playwright and docker-* (269-273, 375-385, 394-404, 413-421, 431-435); the header's ci description (line 6) is stale (missing capture/glyph/css checks and test-js).
- docker/docker-compose.yml re-declares five ENV values the image already sets (docker/Dockerfile:256-262); docker/entrypoint.sh's two seed blocks (56-66, 72-80) could be one function.
- docker/Dockerfile: the nightly date literal appears 9 times (100-104, 179, 181, 206, 227); the sync test guards it, but an ARG would make a bump one edit.
