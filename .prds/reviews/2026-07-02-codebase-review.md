# ironpad codebase review — 2026-07-02

**Method:** six parallel code reviewers (UI components, pages + stylesheet, JS/executor/storage, collab/session layer, compiler/backend, notebook editor) plus a hands-on browser audit of the running app (home, editor, view mode, session panel, public notebooks, light theme, 390 px mobile). Every code finding cites `file:line`; items marked **[confirmed live]** were reproduced in the browser.

**Environment changes made during testing** (all within the repo's own tooling paths):
- Updated `wasm-bindgen-cli` 0.2.114 → 0.2.126 (`cargo binstall`, same as `cargo make install-tools`).
- Cleared `cache/{workspaces,blobs,targets}` — stale artifacts were built against the old CLI and can never link again (see P0-2). `cache/cargo-home` kept.
- Stopped the dev server afterwards; deleted my test share (`data/shares/602efcf22cfb7774.json`) and scratch screenshots.

---

## P0 — Cell execution is broken on the current toolchain

These two together broke *every* cell run and *every* live-simulation demo before my environment fixes; the code-level fixes below are needed to keep it from regressing.

### P0-1. Bare `extern "C"` imports fail to link on current nightly **[confirmed live]**
`crates/ironpad-cell/src/sim.rs:8-22` declares `ironpad_sim_read` / `ironpad_sim_read_all` in a plain `#[cfg(target_arch="wasm32")] extern "C"` block. Newer nightlies no longer pass `--allow-undefined` to `rust-lld` for `wasm32-unknown-unknown`, so any cell that calls `sim::read` fails with `rust-lld: error: undefined symbol: ironpad_sim_read`. This is why the Lagrange Points and Fractal Tree public demos fail; the UI shows only `linking with rust-lld failed: exit status: 1` (the misleading "mutable static" line above it is just a warning).
**Fix:** the executor supplies these functions under `imports.env` (`public/executor-core.js:662,719`), so annotate the block with `#[link(wasm_import_module = "env")]`. Apply the same to the identical bare block for `ironpad_host_message` at `crates/ironpad-cell/src/lib.rs:18-22`, and audit `gpu.rs` for more. The repo `CLAUDE.md` "Known Issue: rust-lld linking failures" section describes this symptom with the wrong cause — worth updating.

### P0-2. wasm-bindgen version drift breaks all cells, in both directions **[confirmed live]**
`crates/ironpad-app/src/compiler/scaffold.rs:123` injects floating `wasm-bindgen = "0.2"` into every micro-crate while `build.rs:182` shells out to whatever `wasm-bindgen` CLI is on the host. wasm-bindgen requires an exact schema match. Observed both failure modes live: a fresh crate resolving 0.2.126 vs CLI 0.2.114 (every new cell 500s), and — after updating the CLI — cached workspaces whose `Cargo.lock` pins 0.2.114 vs CLI 0.2.126 (every previously-compiled cell fails). The blake3 cache key (`cache.rs:29-62`) includes neither the crate version, the CLI version, the rustc version, nor the ironpad-cell sources, so the breakage is sticky and invisible.
**Fix:** read `wasm-bindgen --version` at startup and inject an exact pin (`=X.Y.Z`) in the scaffold; fold `rustc --version`, the wasm-bindgen CLI version, and a hash of `ironpad-cell` sources into the cache key. Add an integration test that compiles a `sim::read` cell so CI catches toolchain rot.

---

## P1 — Editor UX bugs (all user-visible)

### E1. Adding/deleting/reordering a cell wipes every cell's status, output and diagnostics **[confirmed live]**
Root cause `pages/notebook_editor/mod.rs:260-266`: the load gate closure tracks the whole `state.notebook` signal, and `cell_add`/`cell_delete`/`cell_reorder`/meta updates mutate it tracked (`model.rs:265,394,418,451`), so the entire `<NotebookContent/>` subtree is torn down and rebuilt. Each `CellItem`'s local signals (`cell_status`, `last_compile`, `execution_result`, collapse, tab) reset; page-level maps survive, which is why toggling view mode restores things. The rebuild also re-registers the outside-click listener (`mod.rs:336-357`) and re-inits SortableJS (`mod.rs:362-443`) each time — accumulating listeners and duplicate Sortable instances.
**Fix:** gate on `Memo::new(move |_| state.notebook.with(|n| n.is_some()))` in a `<Show>` so the subtree only rebuilds when presence changes. **Land together with E5** (the memo will unmask it).

### E2. Notebook title rename never persists **[confirmed live]**
`components/app_layout.rs:107-111`: the title save `Action` is an empty stub (its comment claims IndexedDB persistence that doesn't exist), and committing never bumps `layout.save_generation`, so the watcher at `mod.rs:212-221` (which would persist via `NotebookUpdateMeta`) never fires. Header shows the new name; reload reverts to "Untitled Notebook".
**Fix:** have `commit_edit` bump `layout.save_generation` (the persistence path from there already works).

### E3. View mode leaves an in-edit markdown cell as a broken Monaco showing "#" **[confirmed live]**
`components/markdown_cell.rs:46,56,67`: `editing` is a private signal with no awareness of view mode, so ◉ doesn't force preview; the remounted Monaco renders broken.
**Fix:** pass `is_view_mode: Signal<bool>` into `MarkdownCell` and add an effect that sets `editing.set(false)` when it flips on.

### E4. Markdown edits don't commit on blur
`components/markdown_cell.rs:39-40,61-99`: the only commit path is the Monaco Escape action; clicking away leaves the cell in edit mode and the parent model never hears the change (contradicts the component's own doc).
**Fix:** wire container blur/focusout → `commit`.

### E5. Order badge `[N]` and label captured by value (latent, exposed by E1's fix)
`cell_item.rs:26,143,1229`: badge/label are seeded from the `cell` captured at mount. Today the constant subtree rebuild masks it; once E1 is fixed, reorder leaves stale badges.
**Fix:** derive position live from `state.cells`.

### E6. Widget changes don't invalidate downstream cells (reactive mode silently no-ops)
`cell_output.rs:407-411`: staleness marking only re-flags keys *already* in `cell_stale`, and successful runs remove entries (`cell_item.rs:615-617`), so after a clean run the map is empty and slider/checkbox changes mark nothing stale → downstream never re-executes.
**Fix:** insert `true` for downstream code cells by position (mirror `model.rs::mark_downstream_stale`).

### E7. Share/Export/Download can serialize stale content; Share gives zero feedback **[silent-share confirmed live]**
`mod.rs:522,626-633,646-665`: none of the three flush the 1 s debounce (`cell_item.rs:1028-1031`) before serializing — type-then-share publishes content missing the last edits. Separately, clicking Share fires `share_notebook` and then shows *nothing* — no dialog, toast, or URL (and the session panel's copy buttons swallow the clipboard promise the same way, `session_panel.rs:109-139`, `:216`).
**Fix:** bump `save_generation` and await persistence before serializing; show a dialog/toast with the share URL and use the existing `CopyButton` (which has "Copied!" feedback) everywhere.

### E8. Cancel re-runs the runaway cell on the main thread — freezes the tab
`public/executor-bridge.js:199-218` (also `tick` 232-253, `tickLive` 266-287): `terminate()` rejects in-flight promises with `AbortError`, but `execute()`'s unconditional `.catch` treats *any* worker rejection as "worker failed → retry on main thread" and re-executes the same cell. Cancelling an infinite loop kills the worker, then re-runs the loop where it can't be killed.
**Fix:** detect `AbortError` in the catch and rethrow instead of falling back.

### E9. Run-all queue stalls if the running cell is deleted
`cell_item.rs:305-349` vs `:116-139`: `delete_cell_fn` never removes the id from `run_all_queue`, so remaining cells stick on "Queued".
**Fix:** `retain(|id| id != &cid)` in delete cleanup.

### E10. Stale downstream output after upstream re-run
`cell_item.rs:487-506` clears page-level output maps but not the downstream `CellItem`'s local `execution_result`, so the old output stays visible with only the ⟳ badge changing.

### E11. Failed cell shows a success badge; Run All aborts silently **[confirmed live]**
In view-only notebooks a cell whose build failed displayed "✓ 1001ms" next to its compile-error panel, and Run All stopped partway with no indication at the top of the page (button never disabled, status bar unchanged).

---

## P1 — Security

The server compiles untrusted input; these matter before any non-localhost deployment.

- **S1. Unsandboxed compilation = arbitrary host code execution/file read.** `server_fns.rs:11` + `build.rs:76-152`: `compile_cell` is an unauthenticated endpoint running `cargo build` in-process. Build scripts/proc-macros in user `cargo_toml` run native code; `include_str!("/etc/passwd")` / `env!()` exfiltrate host data through the returned WASM. Fix: sandbox builds (rootless container/gVisor/firejail), scrub env, restrict fs.
- **S2. `cell_id` path traversal + TOML injection.** `scaffold.rs:39,112`: unvalidated `cell_id` is joined into filesystem paths (`../../..` escapes the workspace → arbitrary file write) and interpolated unescaped into the generated `Cargo.toml`. Fix: validate `[A-Za-z0-9_-]{1,64}` in `compile_cell`.
- **S3. Stored XSS in shared/public notebooks.** `cell_output.rs:231,241` (`inner_html` of cell HTML/SVG output) + `export.rs:191-194`; view mode auto-runs all cells and `/shared/{hash}` is viewed by others. Fix: sanitize or sandboxed iframe.
- **S4. Unbounded share upload.** `server_fns.rs:294-332`: no size cap/rate limit; disk-fill + CPU-blocking parse. Fix: body-size limit before parse.
- **S5. Relay hardening.** Unbounded WS channels, no frame-size cap (`ws.rs:57,264`, `state.rs:38-42`); unauthenticated host slot is last-writer-wins claimable by `notebook_id` (`ws.rs:47-53`); session token logged in plaintext in the daemon URL (`daemon.rs:115-116`).

---

## P2 — Collaboration/session layer

- **C1. Session start loses buffered edits **[confirmed live]**.** `session/connection.rs:119-137` + `ws_send:21-32`: the event-bridge `Effect` runs immediately inside `start_session`, *before* `onopen`, draining `pending_events` (`model.rs:175-181` `mem::take`) and sending into a CONNECTING socket → `InvalidStateError` (four observed live), events permanently lost, never retried. Fix: don't drain unless `ready_state() == OPEN` (or create the bridge in `on_open`).
- **C2. Mutation error responses dropped → agents hang 10 s on every OCC conflict.** `ws.rs:357-369` + `:140-144`: only queries are tracked, so a `Response::Error` for a mutation id has no route; the daemon's oneshot times out instead of surfacing `VersionConflict`. Fix: track mutations too.
- **C3. Host reconnect/two-tab race unregisters the live host.** `ws.rs:104` + `state.rs:88-94`: `unregister_host` removes by notebook_id with no connection-id check, so the old connection's cleanup removes the new host. Fix: compare-and-remove.
- **C4. Agent source edits don't refresh the host's editor.** `model.rs:336-364`: content-only `cell_update` uses `update_untracked` and skips `sync_from_notebook` unless the label changed — fine for local Monaco edits, but agent mutations flow through the same path, so the host UI silently desyncs. Fix: notify on agent-originated edits.
- **C5. `client_id` = first 8 hex chars of the token.** `ws.rs:240-241` + `state.rs:127-169`: two connections on one token collide — responses misroute, and either disconnect strands the survivor. Fix: per-connection UUID.
- **C6. No heartbeat** (`ws.rs:84-94,298-308`): half-open connections keep dead hosts registered; sends "succeed" into the void (compounds C2). Fix: ping/pong + idle timeout.
- **C7. Minor:** `sweep_expired` never scheduled (`sessions.rs:170`); event buffer overflow silently drops events past 64 (`model.rs:113-118`); pending-query map leaks (`state.rs:181-191`); daemon exits on WS close with no reconnect (`daemon.rs:158-214`); `execute` permission gates an op the model rejects (`model.rs:100-105`); `Query::SessionStatus` is dead protocol surface (`model.rs:152-155`).

---

## P2 — Storage & executor robustness

- **ST1. One malformed record hides ALL notebooks.** `storage/client.rs:36-39`: `from_value::<Vec<_>>(...).unwrap_or_default()` — one bad record → empty list → everything looks lost. Fix: deserialize per element, skip failures.
- **ST2. IndexedDB rejections silently kill futures.** `storage/client.rs:9-31`: no `catch` on any async extern; quota errors abort the awaiting task mid-save with no UI error (can sit on "Saving…" forever). Fix: `catch` → `Result`, surface errors. Also `.expect()` in `save_notebook` (`client.rs:52`).
- **ST3. storage.js hygiene:** connections leak on error paths — no `try/finally` around `db.close()` (`storage.js:52-102`); no `onblocked` on open (`:20-34`); NaN-sort on bad `updated_at` (`:58-62`); mutates caller's object (`:85,132-136`).
- **EX1. Dead worker strands all in-flight requests.** `executor-bridge.js:55-58`: `onerror` only logs; pending promises never settle → cells stuck "Compiling/Running" forever. Fix: reject all pending + respawn.
- **EX2. Main-thread fallback calls the wrong global.** `executor.js:12` + `executor-bridge.js:104-105`: fallback is constructed with `"window.IronpadExecutor"`, which the bridge re-claims — so fallback cells using sim/GPU trap on `_simRead is not a function`. Fix: dedicated global for the fallback.
- **EX3. Fallback loader not memoized** (`executor-bridge.js:87-123`): concurrent fallbacks double-inject and can leave `window.IronpadExecutor` pointing at the wrong object (breaks `terminate`).
- **EX4. GPU state is cross-cell.** `executor-core.js:824,903,342-345,457-524`: shared handle map + per-execute cleanup destroys other cells' buffers; readbacks queued by a trapped cell leak into the next result (`:806-815`).
- **EX5. Rayon glue global clobbered by concurrent loads** (`executor-core.js:685,611,638-639`); worker panic message cross-attributed between interleaved executes (`worker-executor.js:16,91-139`); sub-worker blob URL revoked racily (`executor-core.js:636-637`); `_executeOnMainThread` dead code (`bridge.js:125-139`); `"WASM tick trapped: undefined"` for non-Error throws (`executor-core.js:993,1039`).

---

## P2 — Compiler/server correctness

- **B1. Timeout doesn't kill the build.** `build.rs:109-142`: `timeout()` drops the future; cargo + rustc children keep running (no `kill_on_drop`, no process group). Compile bombs burn CPU after "timeout". Fix: `.kill_on_drop(true)` + process group kill.
- **B2. Shared workspace race / cache poisoning.** `server_fns.rs:23,27-48` + `scaffold.rs:39`: session is hardcoded `"default"`, crates keyed only by `cell_id`, scaffold happens outside cargo's lock — two requests with the same cell_id can cache B's WASM under A's hash. One shared `CARGO_TARGET_DIR` also globally serializes all builds. Fix: per-request workspace or per-cell mutex.
- **B3. wasm-opt uses fixed temp names in a shared dir.** `server_fns.rs:100-105` + `optimize.rs:40-41`: `pre_opt.wasm`/`post_opt.wasm` in the shared workspaces dir, outside any lock — concurrent compiles clobber each other. Fix: per-compile temp dir.
- **B4. Torn cache blobs.** `cache.rs:132`: non-atomic `fs::write`; concurrent reader can get truncated WASM as a "hit". Fix: write-to-temp + rename.
- **B5. Diagnostics gaps:** rustc `children` (help/note) never parsed and `shared.rs` spans dropped (`diagnostics.rs:26-31,96`); warnings lost on cache hit (`server_fns.rs:56-62`); raw `anyhow` backtraces and server filesystem paths dumped into user-facing panels **[confirmed live]** — trim to the diagnostic message.
- **B6. Misc:** blocking `std::fs` on the async runtime throughout the compile path; `get_public_notebook` never enforces the `.ironpad` extension its doc promises (`server_fns.rs:251-277` — traversal itself is blocked); `CellInputs::from_raw` panics on truncated buffers (`ironpad-cell/src/lib.rs:161-177`); CLI daemon-stop can SIGTERM a reused PID from a stale pidfile (`ironpad-cli/src/main.rs:180-205`).

---

## P3 — Leaks (long-session degradation)

- StatusBar 30 s interval never cleared; re-created per notebook visit, ticks disposed signals forever (`app_layout.rs:340-354`).
- Monaco `on_change` closure `.forget()`-ed per editor; comment claims otherwise (`monaco_editor.rs:243-253`).
- Global keydown listener leaks per notebook mount (`mod.rs:113-182`); outside-click listener re-registered per structural mutation (E1 cascade).
- Markdown Escape-keybinding closure leaked per edit entry (`markdown_cell.rs:87-98`).
- rAF closure Rc-cycle never broken on cleanup; loop also keeps scheduling at 60 fps while paused (`animation_canvas.rs:231-271,389-447`, `live_view_panel.rs:134-195`).
- CopyButton timeout closure leaked, can fire after unmount (`copy_button.rs:29-37`).
- Cell debounce timers not cancelled on disposal — fire against disposed signals / deleted cells (`cell_item.rs:993-1084`).
- `editor_handles` never pruned on cell delete (`cell_item.rs:765-767`).

---

## P3 — UI polish / "prettying up" (visual)

**Stylesheet bugs:**
1. **Drag handle permanently invisible** — `.ironpad-drag-handle` sets its own `opacity:0` (`main.scss:2526`) and the reveal rule at `:2577` uses a descendant selector that can never match (the handle is in a *sibling* of `.ironpad-cell-card`). Drag-to-reorder is undiscoverable. Fix: remove the self `opacity:0`, delete the dead rule, merge the two conflicting `.ironpad-drag-handle` blocks.
2. **Light theme hover states invisible (systemic)** — 28 hardcoded `rgba(255,255,255,…)` hover overlays plus three hovers that set text to `--ip-text-on-accent` (#fff in both themes) on transparent backgrounds (`:2255,:2287,:2606`) → white-on-white. Fix: add `--ip-hover-overlay` (white-alpha dark / black-alpha light) and swap all 28; hover text → `--ip-text-primary`.
3. **Unbounded output height** — `.ironpad-output-body:1401` has `overflow-y:auto` but no `max-height`; huge stdout grows the card unboundedly. Fix: `max-height:480px`. Also `.ironpad-output-visual img` needs `max-width:100%;height:auto` (`cell_output.rs:288` emits fixed pixel sizes).
4. **Unstyled classes:** `.ironpad-shared-source-panel` (`shared_editor_panel.rs:35`), `.ironpad-widget-button` (`cell_output.rs:937`), `.ironpad-loading` (`public_notebook.rs:36`) have no selectors — bare native rendering. Progress bar uses undefined `--ip-bg-tertiary` + dead `#4fc3f7` blue (`:2556,:2563`). Focus glow hardcodes dark-theme rgba (`:1541`).
5. Dead modifier classes (`ironpad-notebook-badge private/public`, `ironpad-cell-type-badge` base) — drop or style.

**Observed visual/UX rough edges [all confirmed live]:**
6. Empty notebook is a void — two faint "+ Code / + Markdown" text buttons floating in dark space; needs a real empty state (CTA card, keyboard hints).
7. Cell action rail is hover-only (`.ironpad-cell-side-actions` opacity 0) — undiscoverable and unusable on touch; consider always-visible at reduced opacity.
8. Cryptic compile badge "✓ 468+7MS" — spell it out ("468 ms compile · 7 ms run") or tooltip; also "1 bytes".
9. Red is overloaded: focused-cell border, error border, and brand accent are all the same red — a focused cell looks broken. Give focus a neutral/accent-blue border.
10. Accent inconsistency between themes: "New Notebook"/primary buttons are blue in dark theme, red in light theme.
11. Duplicate title on public/shared pages (header + h1); header shows the *filename* (`lagrange-points`) rather than the notebook title; "Cached/Fresh" toggle reuses the theme-toggle look (`view_only_notebook.rs:184-197`).
12. Saved light-theme preference isn't applied to Monaco on load (`app_layout.rs:135-149` vs `:226-241`) — toggle shows light, editors render dark until re-toggled.
13. Notebook title disappears from the header at mobile widths (no responsive slot for it).
14. Icon-only toolbar buttons (☰, ⚙) lack `title`/`aria-label` (`mod.rs:502-510,707-716`).
15. Home cards: add a subtle hover lift (`translateY(-2px)` + shadow); rows have ragged card heights.
16. `<For>` keys can collide in the error panel for duplicate diagnostics (`error_panel.rs:64,156`).
17. Fork button not disabled during async fork — double-click creates two notebooks; save failure still navigates (`view_only_notebook.rs:158-178,205-207`).
18. View-only pages have nested scroll containers (`main.scss:358` vs `:1951`) — potential double scrollbar.

---

## Suggested landing order

1. **Toolchain pair (P0-1, P0-2)** + an integration test that compiles a `sim::read` cell. This un-breaks the product and all public demos.
2. **Editor state batch:** E1+E5 (memo gate + live badges), E2 (title save), E3+E4 (markdown view/blur), E8 (cancel fallback). These are the bugs users hit within minutes.
3. **Data-safety batch:** E7 (flush-before-share + share feedback), ST1/ST2 (storage error handling), EX1 (dead-worker recovery).
4. **SCSS polish batch (P3 items 1-5, 8-15):** one focused PR — token work, selector fixes, empty state. Biggest visual payoff per line changed.
5. **Security batch (S2, S3, S4 quickly; S1 as a project):** validate `cell_id`, sanitize output HTML, cap share size. Sandboxing builds is a prerequisite for any public deployment.
6. **Collab batch (C1-C6)** when agent collaboration is the focus.
7. **Leak sweep (P3 leaks)** — mechanical, one PR.
