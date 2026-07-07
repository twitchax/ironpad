# Codebase Review — 2026-07-07

End-to-end review of the ironpad workspace (~28k Rust LOC across 7 crates), conducted
as seven parallel domain deep-reads plus independent verification (`cargo clippy
--all-targets --workspace` → clean; 532 tests; 317-commit history). This is the
follow-up to `2026-07-02-codebase-review.md` (which produced epics PRD-0031…0037).

## Overall grade: **B+**

Ambitious, disciplined project executed with real security awareness. Consistent pattern
across every subsystem: the *happy path* is well-built and well-tested; the *adversarial*
dimension (resource exhaustion, negative tests, defense-in-depth) lags. Held back from
A-tier by uniform-hardening and CI-trust gaps, not by architecture.

## Per-domain scorecard

| Domain | Grade | Headline |
| --- | :---: | --- |
| Frontend model + sanitizer + storage | A− | Real HTML5-parsing sanitizer + single OCC mutation path; no panic reachable from hostile input |
| Cell runtime (`unsafe`/FFI) | B+ | Symmetric reclamation + solid untrusted-wire parser; one latent soundness gap + an untested decode path |
| Leptos components + pages | B+ | RAF leak discipline; residual `forget()` leaks + `view_only_notebook` duplication |
| Compiler pipeline | B | Near-exemplary diagnostics/cache tests; a real cache-key collision + cooperative-only sandbox |
| Server / relay / sessions / proxy | B− | CSPRNG tokens done right; unauthenticated host role + uncapped DoS vectors |
| Protocol + CLI daemon/IPC | B− | Clean serde/OCC design; SIGTERM skips cleanup, no versioning, `ipc.rs` untested |
| Infra / CI / Docker / deps / docs | B− | Sophisticated build eng; CI doesn't gate what the docs claim |

## Cross-cutting themes

1. **"Correct happy path, thin adversarial hardening"** — recurred independently in all
   seven reviews. The team knows how to harden (process-group kill on timeout, atomic
   cache writes, `CellInputs::from_raw`); it just isn't applied uniformly.
2. **Untrusted-compile sandbox** — cooperative env-proxy only at the app layer. Per the
   recorded decision (PRD-0030 §"Sandboxing decision", PRD-0036), full sandboxing is out
   of scope: Fly Firecracker microVM + egress proxy bound the blast radius. Residual risk
   accepted; **not** re-opened here.
3. **"Green CI" ≠ working artifact** — the "one true gate" (`uat`) isn't run in CI;
   integration/wasm/playwright run only locally or main-only. Toolchain declared stable
   1.93.0 but `rust-toolchain.toml` silently pins nightly-2025-12-22.
4. **Voluminous tests with holes exactly where risk is highest** — `sim::read_from_ffi`,
   sanitizer exclusions, `ipc.rs` framing, relay auth/DoS paths.

## Confirmed findings → PRD-0038 tasks

Correctness bugs:
- Cache-key collision: `compiler/cache.rs:80` concatenates `source`+`cargo_toml` with no
  domain separator (later fields *are* delimited) → `hash("foo","bar") == hash("foobar","")`.
- README data-loss: `README.md:34` mounts `/data`+`/cache`; image stores under `/ironpad/*`.
- Daemon SIGTERM: `cli/daemon.rs:206` handles only SIGINT; documented stop sends SIGTERM → no cleanup.
- Reactive-schedule leak: `notebook_editor/state.rs:151` forgets a debounce closure that is
  then cancelled but never freed; import cancel-path leak at `home_page.rs:506`.

Security hardening (cheap, in-scope):
- `style` attribute passes through with no CSS filtering (`sanitize.rs:116`) → `position:fixed`
  overlay clickjacking / beacon in auto-running shared notebooks. Generic `id`/`xmlns` too.
- `read` permission is not a confidentiality boundary (`sessions.rs:182` returns true for all
  events; events carry full content).
- Uncapped resources: guest idle-timeout/connection cap (`ws.rs:345`), `wasm-opt` timeout
  (`optimize.rs:58`), aggregate share cap (`server_fns.rs:357`), IPC frame bound (`daemon.rs:411`).

Soundness / robustness:
- `vec_into_raw` uses `shrink_to_fit` (`cell/lib.rs:1233`); only `into_boxed_slice()` guarantees
  `capacity==len` (siblings already use it). Latent mismatched-size dealloc.
- No protocol version / unknown-variant fallback (`common/protocol.rs`); cross-version peers
  degrade to silent drops + 10s stalls.

Deferred-from-0036 follow-ups (design):
- Host credential to claim a `notebook_id` (`ws.rs:58`) — unauthenticated host role.
- Non-root Dockerfile `USER` (needs gosu privilege-drop entrypoint on the Fly volume mount).

Test-coverage gaps (the primary ask):
- Sanitizer bypass regressions (foreignObject/use/animate/data:/style-position).
- `sim::read_from_ffi` FFI decode round-trip + malformed-input.
- `ipc.rs` framing round-trip + oversize rejection.
- Relay auth/expiry/permission-denied + session read-boundary.
- Cache-key collision regression.

## Genuine strengths

Parser-based sanitizer (ammonia/html5ever), markdown sanitized after rendering, KaTeX
`trust:false`; single OCC-guarded mutation path shared by UI + agents; CSPRNG tokens stored
as blake3 hashes; process-group kill on compile timeout; atomic cache writes; RAF Weak/Rc
cycle-break leak discipline; centralized workspace lints; dated-nightly pin with honest
rationale; fail-closed egress proxy.
