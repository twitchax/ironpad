---
id: PRD-0043
title: "JSPI blocking host calls: synchronous Rust that suspends the WASM stack"
status: done
owner: "Aaron Roney"
created: 2026-07-10
updated: 2026-07-11

principles:
- "The point is the absence of coloring: plain synchronous Rust calls that suspend the whole WASM stack. Cells can already do async I/O via .await; JSPI removes the async requirement entirely."
- "Zero compiler-pipeline changes: suspension is JS-side wiring (WebAssembly.Suspending imports + WebAssembly.promising entry). No RUSTFLAGS, no toolchain, no cache-key changes."
- "Chrome/Edge 137+ only for now (JSPI is phase 4 and an Interop 2026 focus area); other browsers get a clear, friendly error, never a cryptic trap."
- "No re-entry into a suspended instance: the suspending import stashes the payload JS-side and returns its length; a separate synchronous import copies into a cell-allocated buffer (the established ironpad_sim_read shape)."

references:
- name: "caniuse: WebAssembly JSPI"
  url: https://caniuse.com/wf-wasm-jspi
- name: "V8: Introducing the WebAssembly JavaScript Promise Integration API"
  url: https://v8.dev/blog/jspi
- name: "JSPI proposal (phase 4)"
  url: https://github.com/WebAssembly/js-promise-integration

acceptance_tests:
- id: uat-001
  name: "A cell using ironpad_cell::blocking compiles and links (imports resolve under wasm_import_module env)"
  command: cargo make test-integration
  uat_status: verified
- id: uat-002
  name: "Playwright (Chromium >= 137): blocking sleep suspends and resumes; blocking fetch returns same-origin content from plain sync Rust"
  command: cargo make playwright
  uat_status: verified
- id: uat-003
  name: "Full gate passes with JSPI infra and notebook in place"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "ironpad-cell blocking module: sleep_ms, fetch_text, fetch_json, fetch_bytes over ironpad_blocking_* env imports"
  priority: 1
  status: done
  notes: "#[link(wasm_import_module = \"env\")] (PRD-0031 lesson). Two-phase fetch protocol: ironpad_blocking_fetch(url_ptr, url_len) -> payload_len (suspending), ironpad_blocking_fetch_ok() -> u32, ironpad_blocking_read(buf_ptr, buf_len) -> copied (sync). Native stubs: sleep via std::thread::sleep, fetch returns Err (sim.rs pattern)."
- id: T-002
  title: "executor-core.js: Suspending imports + promising entry + needs-JSPI detection + friendly unsupported-browser error"
  priority: 1
  status: done
  notes: "Pre-compile module, scan WebAssembly.Module.imports for ironpad_blocking_ prefix, store needsJspi on the entry. Add shims to all three import sites (ESM env rewrite, __wbg_get_imports patch, raw path). execute(): needsJspi + JSPI available -> WebAssembly.promising(entry.wasm.cell_main); needsJspi + unavailable -> throw 'requires Chrome/Edge 137+' before invoking."
- id: T-003
  title: "Integration test: blocking cell compiles and links"
  priority: 2
  status: done
  notes: "e2e_tests in compiler/mod.rs, follows compile_cell_with_host_imports_links_successfully."
- id: T-004
  title: "Playwright e2e: sleep suspends (Stopwatch elapsed >= sleep) and same-origin blocking fetch returns content"
  priority: 2
  status: done
  notes: "Playwright Chromium ships JSPI; feature-detect in the test and fail loudly if the bundled Chromium is too old."
- id: T-005
  title: "Public notebook: function-coloring story, Chrome/Edge-only banner"
  priority: 3
  status: done
  notes: "Contrast existing async cell (ironpad_cell::http::get + .await) with the JSPI version in fully synchronous code (e.g. inside an Iterator chain). Prominent Chrome/Edge 137+ note in description + first markdown cell. Same-origin fetch primary demo; api.github.com secondary (per-visitor rate limits, CORS-enabled)."
---

# Summary

Give cells blocking host calls via the WebAssembly JavaScript Promise Integration API (JSPI): plain synchronous Rust functions (`blocking::sleep_ms`, `blocking::fetch_text`) whose calls suspend the entire WASM stack while a JS promise settles. Ship a public notebook telling the function-coloring story, gated to Chrome/Edge 137+ with a friendly error elsewhere.

# Problem

Cells can already await network I/O, but only by becoming async: `.await` colors the cell wrapper, and no synchronous call path (trait impls, iterator adapters, plain helper functions) can perform I/O. JSPI (phase 4, shipped by default in Chrome/Edge 137) suspends a WASM stack at any depth, making blocking calls free of coloring. That is both a genuinely new capability for cells and one of the best current demos of where WASM is heading.

# Goals

1. `ironpad_cell::blocking::{sleep_ms, fetch_text, fetch_json, fetch_bytes}` work from fully synchronous cell code on JSPI-capable browsers.
2. Unsupported browsers get a clear error naming the requirement, not a trap.
3. Zero changes to the compile pipeline; cells that do not use `blocking::*` are untouched at load and execute time.

# Technical Approach

The executor pre-compiles the cell module and scans `WebAssembly.Module.imports()` for the `ironpad_blocking_` prefix. For such cells, the env import shims are `new WebAssembly.Suspending(async fn)` wrappers and the entry point is invoked via `WebAssembly.promising(entry.wasm.cell_main)` (the raw export beneath the wasm-bindgen wrapper — sync cells only, where the raw ABI is `(i32, i32) -> i32`). The fetch protocol is two-phase to avoid re-entering a suspended instance: the suspending import performs the fetch and stashes the payload per-cell JS-side, returning its byte length (or an error flag + message length); after resume, the cell allocates a buffer and a plain synchronous import copies the payload in — the same shape as `ironpad_sim_read`.

# Assumptions

- Playwright's bundled Chromium is >= 137 (JSPI on by default) so the e2e coverage is real.
- Calling `ironpad_alloc`-style exports is safe during synchronous imports (established by `sim_read`); the design avoids any export call while the instance is suspended anyway.

# Constraints

- Sync cells only: `.await` cells go through the wasm-bindgen async wrapper whose raw ABI is not `cell_main(i32, i32) -> i32`; mixing `.await` and `blocking::*` is unsupported (documented in the module docs).
- Simulation `tick` paths keep the non-promising call; blocking calls in tick are out of scope.
- Fetch is subject to page CSP and target CORS, same as the existing async `http` module.

# References to Code

- `crates/ironpad-cell/src/http.rs` — the async counterpart being contrasted
- `crates/ironpad-cell/src/sim.rs` — import declaration + two-phase read pattern
- `public/executor-core.js` — loadBlob import shims (3 sites), `_executeBindgen` / `_executeRaw`
- `crates/ironpad-app/src/compiler/mod.rs::e2e_tests` — link-check precedent

# Non-Goals (MVP)

- JSPI for async (`.await`) cells, simulation ticks, or rayon cells
- Firefox/Safari support (revisit when they ship; Interop 2026 focus area)
- Blocking APIs beyond sleep + fetch (no blocking WebSocket, IndexedDB, etc.)

# History

- 2026-07-10: Created. Chrome/Edge 137 ship JSPI by default; Firefox behind a flag; Safari implementing after dropping its objection late 2025.
- 2026-07-11: T-001..T-005 done. Protocol proven three ways: Node harness (--experimental-wasm-jspi: 80ms real suspension, fetch body, 404 propagation), Playwright e2e (300ms suspend + same-origin fetch through the production executor), and the live notebook (sleep 401ms; three fetches inside Iterator::map; api.github.com round trip 291ms). CACHE_EPOCH bumped 4 -> 5 for the ironpad-cell change. Pitfall documented in the generator: cell text must never contain the literal ".await" substring or needs_async() flips the cell to the async wrapper the promising path cannot enter.
