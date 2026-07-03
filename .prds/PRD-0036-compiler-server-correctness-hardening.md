---
id: PRD-0036
title: "Compiler/server correctness + pragmatic hardening (B1-B6, S2-S5)"
status: active
owner: "Aaron Roney"
created: 2026-07-02
updated: 2026-07-02

depends_on:
- PRD-0031

principles:
- "A timed-out build must actually die; concurrent builds must not corrupt each other"
- "Pragmatic hardening — rely on the Fly microVM + egress proxy as the isolation boundary"
- "Validate untrusted input at the boundary; never trust cell_id or share size"

references:
- name: "Review report — sections B1-B6 (compiler/server), S2-S5 (hardening)"
  url: reviews/2026-07-02-codebase-review.md
- name: "Sandboxing decision (why full containerization is out of scope)"
  url: PRD-0030-post-review-delivery-roadmap.md

acceptance_tests:
- id: uat-001
  name: "A build that exceeds the timeout leaves no orphaned cargo/rustc processes"
  command: cargo make test-integration
  uat_status: unverified
- id: uat-002
  name: "Two concurrent compiles of different cells don't clobber each other's WASM/optimized output"
  command: cargo make test-integration
  uat_status: unverified
- id: uat-003
  name: "cell_id with path/TOML metacharacters is rejected before any filesystem or Cargo.toml use"
  command: cargo make test
  uat_status: unverified
- id: uat-004
  name: "A shared notebook emitting <img onerror=...> does not execute script when viewed"
  command: cargo make playwright
  uat_status: unverified

tasks:
- id: T-001
  title: "B1: Kill the build process group on timeout"
  priority: 1
  status: done
  notes: "build.rs:109-142: timeout() drops the future but cargo + its rustc/build-script children keep running (no kill_on_drop, no process group) -> a compile-bomb cell burns CPU/RAM after 'timeout'. Fix: .kill_on_drop(true) + .process_group(0), and killpg the group on timeout (killing only cargo is insufficient — it spawns children)."
- id: T-002
  title: "S2: Validate cell_id before any filesystem or Cargo.toml use"
  priority: 1
  status: done
  notes: "scaffold.rs:39,112: unvalidated cell_id (from request.cell_id, server_fns.rs:33) is joined into fs paths (../../.. escapes the workspace -> arbitrary file write within server perms) and interpolated unescaped into name = \"cell-{cell_id}\" (\" + newline injects [package] keys, e.g. build=). Fix: validate ^[A-Za-z0-9_-]{1,64}$ in compile_cell before use; reject otherwise."
- id: T-003
  title: "B2: Isolate workspace + target dir per request (fix cache-poisoning race)"
  priority: 1
  status: todo
  notes: "server_fns.rs:23,27-48 + scaffold.rs:39: session hardcoded 'default', crate keyed only by cell_id, scaffolding happens before cargo's lock -> two requests sharing a cell_id (same notebook in two tabs, or attacker-chosen id) race: A writes SA, B overwrites SB, A builds SB and caches it under hash(SA) -> wrong result. One shared CARGO_TARGET_DIR also serializes ALL builds globally. Fix: per-request workspace + target dir (include content hash / UUID in the path), or a per-cell_id compile mutex."
- id: T-004
  title: "B3: Give wasm-opt a unique per-compile work dir + filenames"
  priority: 2
  status: todo
  notes: "server_fns.rs:100-105 + optimize.rs:40-41: optimize runs on crate_dir.parent() = {cache}/workspaces/default (shared) with fixed pre_opt.wasm/post_opt.wasm names, outside cargo's lock -> concurrent compiles clobber each other's temp files. Fix: unique per-compile dir (use crate_dir itself or a TempDir) + unique filenames."
- id: T-005
  title: "B4: Atomic cache blob writes"
  priority: 2
  status: done
  notes: "cache.rs:132: store_blob does std::fs::write (truncate+write); a concurrent try_cache_hit (cache.rs:88 std::fs::read) can read a partial file and return truncated WASM as a 'hit'. Fix: write to {path}.tmp.{uuid} then std::fs::rename (atomic same-fs); same for the .js glue."
- id: T-006
  title: "S3: Sanitize cell HTML/SVG output (stored XSS in shared/public notebooks)"
  priority: 1
  status: done
  notes: "cell_output.rs:231 (<div inner_html=html>) and :241 (SVG) inject cell output raw; export.rs:191-194 inlines the same. View mode auto-runs all cells (mod.rs:447-460) and /shared/{hash} notebooks are viewed by OTHERS -> a cell emitting Html('<img src=x onerror=...>') runs script in the viewer's origin. Fix: sanitize HTML/SVG panels (allowlist) or render in a sandboxed iframe."
- id: T-007
  title: "S4: Cap shared-notebook upload size"
  priority: 2
  status: done
  notes: "server_fns.rs:294-332: share_notebook_core accepts arbitrary-length notebook_json, serde_json::from_str over all of it, writes to disk — no size cap/rate limit -> distinct large uploads fill disk and the parse CPU-blocks the runtime. Fix: enforce a max body size (Axum DefaultBodyLimit / explicit length check) before parse."
- id: T-008
  title: "S5 + B5/B6: server hardening, diagnostics cleanup, and misc correctness"
  priority: 2
  status: todo
  notes: "Bundle the smaller items: (S5) bound the WS relay channels + cap frame size on upgrade (ws.rs:57,264, state.rs:38-42) and add a host credential to claim a notebook_id (ws.rs:47-53, currently last-writer-wins by UUID); add a USER (non-root) directive to docker/Dockerfile as defense-in-depth. (B5) parse rustc children for help/note text and accept src/shared.rs spans (diagnostics.rs:26-31,96); cache diagnostics alongside the blob so warnings survive a cache hit (server_fns.rs:56-62); trim raw anyhow backtraces + server fs paths out of user-facing panels (confirmed live). (B6) use tokio::fs/spawn_blocking for the sync std::fs on the compile hot path; enforce the .ironpad extension in get_public_notebook (server_fns.rs:251-277); bounds-check CellInputs::from_raw (ironpad-cell/src/lib.rs:161-177); verify /proc/<pid>/cmdline before the daemon SIGTERMs a pidfile PID (ironpad-cli/src/main.rs:180-205)."
---

# Summary

The compile pipeline and server have correctness bugs (timeouts that don't kill the build, concurrent compiles that poison each other's cache/temp files, torn cache blobs) and untrusted-input gaps (path-traversal via `cell_id`, unbounded share uploads, stored XSS in shared notebooks). This epic fixes the correctness bugs and applies *pragmatic* hardening — the cheap, high-value items — while explicitly declining full build sandboxing.

# Problem

`compile_cell` runs `cargo build` on untrusted input. The correctness bugs (B1-B6) matter for reliability regardless of deployment. The security items (S2-S5) matter because ironpad is reachable on Fly.io. Per the roadmap's recorded decision, the Fly Firecracker microVM + the existing egress proxy already bound the RCE blast radius to one ephemeral VM, so full sandboxing is out of scope; the remaining worthwhile hardening — validate `cell_id`, cap uploads, kill runaway builds, sanitize output HTML, run non-root — is cheap and mostly overlaps DoS/correctness fixes.

# Goals

1. Timed-out builds die completely; concurrent compiles are isolated.
2. Cache reads never return torn/wrong blobs; warnings survive cache hits.
3. Untrusted `cell_id` and share size are validated at the boundary.
4. Shared/public notebook output can't run script in a viewer's browser.
5. Diagnostics show clean messages, not backtraces and server paths.

# Technical Approach

Compiler fixes in `compiler/{build,optimize,cache,diagnostics}.rs` and `server_fns.rs`; hardening spans `server_fns.rs`, `ws.rs`, `state.rs`, `docker/Dockerfile`, and the XSS fix in `cell_output.rs`/`export.rs`. **Sequence B2/B4/B5 cache items after PRD-0031** (which reworks the cache key). T-001, T-002, T-003, T-006 are the highest-value (runaway builds, path traversal, cache poisoning, XSS).

## Explicitly out of scope (recorded)

Full build sandboxing (gVisor / rootless nested containers / seccomp profiles) is **not** a task here. The residual risk — a malicious build can read that one Fly VM's env/volume and use its (proxy-filtered) network — is accepted for a single-author fun project. Revisit only if ironpad becomes a serious multi-tenant service.

# Assumptions

- Deployment remains Fly.io (Firecracker microVM per instance) with the compilation proxy in front (`docker/entrypoint.sh` starts `ironpad-proxy`).
- No real secrets that matter live in the server's runtime env/volume (worth confirming when doing T-008's non-root work).

# Constraints

- `compile_cell` stays a `#[server]` fn; hardening is additive (validation, limits), not a rewrite.
- Per-request isolation must not defeat the compile cache's legitimate hits (key on content, not path).

# References to Code

- `crates/ironpad-app/src/compiler/{build.rs,optimize.rs,cache.rs,diagnostics.rs}`
- `crates/ironpad-app/src/server_fns.rs`
- `crates/ironpad-app/src/pages/notebook_editor/{cell_output.rs,export.rs}`
- `crates/ironpad-server/src/{ws.rs,state.rs}`, `crates/ironpad-cli/src/main.rs`
- `crates/ironpad-cell/src/lib.rs`, `docker/Dockerfile`

# Non-Goals (MVP)

- Full build sandboxing / seccomp / rootless nested containers (documented residual risk).
- Rate limiting / auth on the public endpoints beyond the specific caps in this epic.

# History

(Entries appended during implementation go below this line.)
