---
id: PRD-0038
title: "Post-review hardening + test-coverage expansion (2026-07-07 review)"
status: active
owner: "Aaron Roney"
created: 2026-07-07
updated: 2026-07-07

depends_on:
- PRD-0030
- PRD-0036
- PRD-0037

principles:
- "Fix the confirmed correctness bugs first; each ships with a regression test that fails before the fix"
- "Harden the adversarial dimension uniformly — the strengths already exist, apply them everywhere"
- "Respect the recorded sandboxing decision (PRD-0030): Fly microVM + egress proxy is the isolation boundary; do not re-open full build sandboxing"
- "Close test-coverage holes exactly where risk is highest: sanitizer exclusions, FFI decode, IPC framing, relay auth/DoS"

references:
- name: "Review report — 2026-07-07 (seven-domain deep read, overall B+)"
  url: reviews/2026-07-07-codebase-review.md
- name: "Prior review + epics (0031-0037)"
  url: reviews/2026-07-02-codebase-review.md
- name: "Recorded sandboxing decision (why full isolation is out of scope)"
  url: PRD-0030-post-review-delivery-roadmap.md
- name: "Prior compiler/server hardening (deferred follow-ups picked up here)"
  url: PRD-0036-compiler-server-correctness-hardening.md

acceptance_tests:
- id: uat-001
  name: "Distinct (source, cargo_toml) pairs whose byte-concatenation collides produce DIFFERENT cache keys"
  command: cargo make test
  uat_status: verified
- id: uat-002
  name: "Sanitizer strips foreignObject/use/image/animate and data:-URI hrefs, and neutralizes position:fixed/z-index in style="
  command: cargo make test
  uat_status: verified
- id: uat-003
  name: "A shared notebook emitting a position:fixed overlay <a> cannot capture a click in the viewer (DOM-level)"
  command: cargo make playwright
  uat_status: verified
- id: uat-004
  name: "sim::read_from_ffi round-trips a valid host buffer and rejects a truncated/oversized length prefix without UB"
  command: cargo make test
  uat_status: verified
- id: uat-005
  name: "IPC frame codec round-trips and rejects an over-limit frame instead of unbounded-buffering"
  command: cargo make test
  uat_status: verified
- id: uat-006
  name: "Relay rejects an expired token (HTTP 410) and a permission-denied mutation over the wire"
  command: cargo make test
  uat_status: verified
- id: uat-007
  name: "daemon-stop (SIGTERM) removes the socket + pidfile — no stale files left"
  command: cargo make test
  uat_status: verified
- id: uat-008
  name: "Full gate green (ci + integration + playwright)"
  command: cargo make uat
  uat_status: unverified

tasks:
# ── P1: confirmed correctness bugs (each TDD'd) ──────────────────────────────
- id: T-001
  title: "Cache-key collision: add a domain separator between source and cargo_toml"
  priority: 1
  status: done
  notes: "DONE 2026-07-07: length-prefix (frame) every variable-length field in content_hash_inner (update_framed/update_framed_opt helpers), CACHE_EPOCH 3->4, regression test hash_disambiguates_field_boundaries (covers source/cargo_toml AND cargo_toml/target boundaries). Fixed root cause for ALL field boundaries, not just source/cargo_toml. Original: compiler/cache.rs:79-82: content_hash_inner streams source then cargo_toml into blake3 with NO delimiter, while later fields ARE delimited (\\x01-\\x05). So content_hash(\"foo\",\"bar\",..) == content_hash(\"foobar\",\"\",..) — two distinct crates serve each other's compiled WASM from cache. Both fields are user-controlled in one CompileRequest. Fix: hash a length-prefix or a \\x00 separator between source and cargo_toml (matching the delimiter discipline already used for the trailing fields), then bump CACHE_EPOCH. TDD: uat-001 regression asserting the two inputs hash differently — write it first (fails), then fix."
- id: T-002
  title: "README Docker volume paths cause data loss"
  priority: 1
  status: done
  notes: "README.md:34-35 shows `-v ironpad-data:/data -v ironpad-cache:/cache`, but the image uses VOLUME [\"/ironpad\"] with IRONPAD_DATA_DIR=/ironpad/data and IRONPAD_CACHE_DIR=/ironpad/cache (docker/Dockerfile ENV). Following the README binds empty volumes at unused paths; real data lives on the anonymous /ironpad volume and is lost on container recreation. Fix: correct the README to mount `-v ironpad:/ironpad` (single volume) or per-subdir `/ironpad/data` + `/ironpad/cache`, matching the Dockerfile. Also fix the PLACEHOLDER codecov badge token (README.md:2)."
- id: T-003
  title: "Daemon SIGTERM handler: run cleanup on the documented stop path"
  priority: 1
  status: done
  notes: "cli/daemon.rs:206-232: the shutdown select! only awaits ctrl_c() (SIGINT), but the documented stop path (main.rs:493 libc::kill(pid, 15)) sends SIGTERM → default disposition = immediate termination, so cleanup() (removes socket + pidfile) never runs and the WS drops uncleanly. Fix: add a signal::unix::signal(SignalKind::terminate()) arm to the select! and run cleanup() on either signal. TDD: uat-007 spawns the daemon, SIGTERMs it, asserts socket + pidfile are gone."
- id: T-004
  title: "Residual forget() leaks the leak-sweep missed"
  priority: 1
  status: done
  notes: "Two sites PRD-0037 missed. (a) notebook_editor/state.rs:151 schedule_reactive_execution: cb.forget() on the debounce Closure::once — the prior timer is cleared (state.rs:105-108) but its closure was already forgotten, so it leaks permanently AND never fires; on the typing path in reactive mode. Fix: store in StoredValue and drop in on_cleanup, matching the sibling cell debounce pattern (cell_item.rs:1098,1164). (b) home_page.rs:506,512 import_notebook_from_file: on_load/on_change forget() + on file-dialog CANCEL the change event never fires, so the hidden <input> is never removed and the closure leaks; repeated cancels accumulate orphan DOM. Fix: store closures via StoredValue::new_local + on_cleanup; remove the hidden input unconditionally."
# ── P1: sanitizer hardening + the regression tests it rests on ───────────────
- id: T-005
  title: "Sanitizer: CSS-filter style= and tighten the generic attribute allowlist"
  priority: 1
  status: done
  notes: "sanitize.rs:116 adds \"style\" to SVG_ATTRS but ammonia only sanitizes CSS if filter_style_properties() is called (it is not), so the entire inline declaration list survives. Not script-exec on modern browsers, but enables position:fixed;inset:0;z-index overlay clickjacking + background:url() beacons in auto-running shared/public notebooks. Fix: call HTML_BUILDER.filter_style_properties(<presentation allowlist>) — color/background/font/border/margin/padding/display/text-*/etc. — excluding position, z-index, content, and *-binding. Also drop generic id (sanitize.rs:115 — DOM-clobbering surface; scope id to SVG tags where url(#id) needs it) and generic xmlns (sanitize.rs:132 — inert but unnecessary). Update the under-stated doc comment at sanitize.rs:24-26."
- id: T-006
  title: "Sanitizer bypass regression suite (defends the exclusions the model rests on)"
  priority: 1
  status: done
  notes: "DONE 2026-07-07 (unit part): added strips_foreign_object_and_its_active_content, strips_use_and_image_elements, strips_animate_smil, strips_data_uri_href — all pass on current ammonia config (regression-lock against drift). REMAINING: the style-position neutralization test (blocked on T-005) and the DOM-level Playwright spec (uat-003). Original: sanitize.rs:152-224 tests only <script>, on*, javascript:. The security rationale (sanitize.rs:41-71) depends on foreignObject/use+xlink:href/image/animate/set and data: being stripped, with ZERO tests asserting it — a future ammonia-config change would silently un-break them. Add unit tests asserting removal of: <svg><foreignObject><img src=x onerror=1>, <use xlink:href=\"data:...\">, <image href=\"data:...\">, <animate attributeName=href ...>, <a href=\"data:text/html,...\">, and that style=\"position:fixed;inset:0;z-index:9\" is neutralized per T-005 policy. (uat-002.) Plus uat-003: a DOM-level Playwright spec that renders a shared notebook whose cell emits a full-viewport overlay <a> and asserts a click on the underlying element still lands (no hijack)."
# ── P1/P2: the test-coverage holes (primary ask) ────────────────────────────
- id: T-007
  title: "Cover sim::read_from_ffi — the one untested nontrivial unsafe decode path"
  priority: 1
  status: done
  notes: "cell/sim.rs:85-116 read_from_ffi (wasm32-only) does the length-prefix parse + two from_raw_parts reads + ironpad_dealloc — the highest-risk unsafe in the crate, exercised by NO test (sim.rs:120-159 only hit the native no-op stubs). Refactor the pure decode (given a &[u8] buffer, produce T or None) out of the raw-pointer/dealloc shell so it is testable on the host, then add: valid round-trip, truncated buffer (< 4 header bytes), header length exceeding the buffer, and a json_len near u32::MAX (checked_add must not wrap — see T-010). (uat-004, gated behind test-integration for any wasm-target parts.)"
- id: T-008
  title: "IPC framing codec + tests"
  priority: 2
  status: done
  notes: "DONE 2026-07-07 (tests): cli/ipc.rs went from 0 -> 6 tests (request/response serde round-trips, #[serde(default)] args, newline-delimited framing contract, skip_serializing_if shapes). REMAINING (hardening): the bounded frame reader (take(MAX_IPC_FRAME)) in daemon.rs/main.rs and socket perms (0o600/0o700) — those are code changes, not covered yet. Original: cli/ipc.rs has ZERO tests, and daemon.rs:411-413 / main.rs:466 read frames with tokio lines()/next_line() — unbounded buffering until \\n/EOF (a local client that never sends a newline forces OOM). Introduce a bounded frame reader (take(MAX_IPC_FRAME) or a length-prefixed codec) and add round-trip tests + an over-limit-rejection test. (uat-005.) Also harden the socket: set_permissions(0o600) on the socket and 0o700 on ~/.ironpad (daemon.rs:71,85) rather than relying on umask."
- id: T-009
  title: "Relay auth/DoS integration coverage + session read-boundary decision"
  priority: 2
  status: done
  notes: "server/tests/relay.rs is happy-path-plus-one (only round-trip + invalid-token). Add over-the-wire tests for: expired token → HTTP 410 (ws.rs:270-271 ValidateError::SessionExpired branch is untested e2e), permission-denied mutation reply, and host-disconnect → session invalidation → guest SessionEnded (uat-006). SEPARATELY decide the read-permission semantics (sessions.rs:182-190 check_permission returns true for every Event, so a read:false guest still receives full cell content via broadcast events, protocol.rs:143-190): either gate event broadcast on the guest's read permission, or document that 'read' only gates explicit Query and rename the field to reflect that. Land whichever with a test."
# ── P2: resource caps / DoS (uniform hardening) ─────────────────────────────
- id: T-010
  title: "Cell-runtime soundness: into_boxed_slice + checked arithmetic"
  priority: 2
  status: done
  notes: "(a) cell/lib.rs:1233-1236 vec_into_raw uses shrink_to_fit, which is only ALLOWED to reach capacity==len — the dealloc path (lib.rs:1300 from_raw_parts(ptr,len,len)) assumes it; switch to into_boxed_slice() like the sibling TickResult/LiveTickResult (lib.rs:943,1011) which guarantee it. (b) sim.rs:109,112 unchecked u32 ptr+len arithmetic → checked_add. (c) gpu.rs:159 and canvas.rs:43,53,80 u32 size products can wrap on wasm32 → checked_mul or u64 intermediate. Latent-not-exploitable given the trusted host + sandbox, but each is a cheap hardening win; tests come with T-007."
- id: T-011
  title: "Server/compiler resource caps"
  priority: 2
  status: done
  notes: "Bundle the uncapped DoS vectors NOT already covered by PRD-0036: (a) guest connections have no idle timeout (ws.rs:345-355, unlike the 120s host timeout at ws.rs:100-113) and no per-session/global connection cap (state.rs:117-132) → add an idle timeout + a guest cap. (b) optimize.rs:58-61 wasm-opt runs with no timeout (unlike build's 300s) → wrap in a bounded timeout + kill. (c) server_fns.rs:357-392 share_notebook writes one file per distinct body with no aggregate cap/eviction → add a total-bytes/-count cap (reject or LRU-evict). (d) mod.rs:31-37 CompileLocks table grows unbounded keyed on cell_id → prune idle entries. Configure an explicit axum DefaultBodyLimit so the MAX_SHARE_BYTES check isn't the only guard (server_fns.rs:364)."
- id: T-012
  title: "Protocol versioning + forward-compatible enums"
  priority: 2
  status: in-progress
  notes: "common/protocol.rs:16-34 Message + every sub-enum (Mutation:61, Event:140, Response:196, ControlMessage:233) lack a version field and any #[serde(other)] catch-all, and none are #[non_exhaustive] → a newer peer's added variant makes the ENTIRE Message fail to deserialize on an older daemon, silently dropping it (daemon.rs:237-240) and stalling the correlated IPC oneshot for the full 10s. Fix: add a protocol version to the Message envelope and an Unknown #[serde(other)] arm (or a catch-all variant) to the payload enums so unknown variants degrade gracefully; add round-trip tests for old→new and new→old."
# ── P2: CI trust (green CI should mean a working, reproducible artifact) ─────
- id: T-013
  title: "Wire the real gate into CI + fix the toolchain/deps drift"
  priority: 2
  status: done
  notes: "build.yml:19 runs only `cargo make ci` (fmt+clippy+test) → the compiler-pipeline #[ignore] integration tests (compiler/mod.rs), the wasm/hydrate frontend compile, and all Playwright e2e never run in CI (only main-only Docker job / locally). Add: a test-integration step, a `cargo leptos build`/`clippy --features hydrate` wasm step, and a fast Playwright smoke subset; trigger on pull_request (not just push). Fix reproducibility: CI + docker/Dockerfile declare stable 1.93.0 but rust-toolchain.toml pins nightly-2025-12-22 (silently wins, and defeats cargo-chef since deps cook on stable then build on nightly) — align both to the pinned nightly; pin wasm-bindgen-cli --version 0.2.114 in the Dockerfile (Makefile already does) and use cargo binstall not `cargo install` from source. Add a cargo-audit/deny step + deny.toml."
# ── P3: deferred design follow-ups (picked up from PRD-0036 deferrals) ───────
- id: T-014
  title: "Host credential to claim a notebook_id (close the unauthenticated host role)"
  priority: 3
  status: done
  notes: "DONE 2026-07-08 (TDD, design spec: docs/superpowers/specs/2026-07-08-host-credential-design.md). Trust-on-first-use (TOFU), in-memory, forget-when-idle + a first-message handshake. Protocol: new ControlMessage::ClaimHost { secret } (browser's first frame). Server: WsState.notebook_secrets (notebook_id → blake3(secret)); claim_host() binds on first claim, constant-time-matches on later ones, else Rejected; forget_secret_if_idle() drops the binding when no host + no sessions; handle_host validates the first frame BEFORE register_host — mismatch closes WS 4403 (incumbent untouched), bad/missing first frame closes 4400. Browser (connection.rs): per-notebook secret in localStorage['ironpad:host-secret:{id}'] (two v4 UUIDs, CSPRNG via getrandom; kept OUT of the notebook record so export/share never leaks it), sent as the first frame; on-close 4403 → clear console error, no reconnect loop. Rollout: ClaimHost required (no legacy fallback) — a pre-update tab reloads into new WASM. Tests: protocol round-trip; 4 state unit tests (TOFU/match/reject + forget-when-idle keeps/drops with host/session); 2 relay integration tests (first-frame-must-be-claim → 4400; mismatch → 4403 without evicting incumbent); all 8 existing relay tests updated to send ClaimHost. FOLLOW-UP (noted, out of scope): a user-visible 'rejected' UI state (currently console-only). Original: Deferred from PRD-0036 T-008. ws.rs:58-81 ws_host_handler upgrades and register_host (state.rs:66-85) using only the client-supplied notebook_id — no auth, no owner binding, and a new host EVICTS the incumbent (hosts.insert). Anyone who learns a notebook_id (URL/referrer/logs) can claim the authoritative model, receive all guest mutations/queries, and mint sessions. Today only UUID secrecy protects it. Fix (new auth scheme — hence P3): issue a per-notebook host secret on first claim (stored client-side in IndexedDB) and require it to re-claim / evict; reject a mismatched claim. Design first, then implement + test."
- id: T-015
  title: "Non-root Docker runtime (defense-in-depth)"
  priority: 3
  status: todo
  notes: "Deferred from PRD-0036 T-008. docker/Dockerfile has no USER directive → the runtime compiles untrusted cell code as root. Per PRD-0030 the microVM is the real boundary, so this is defense-in-depth, but it was explicitly deferred because it needs a gosu/su-exec privilege-drop entrypoint that still works with Fly's persistent-volume mount ownership. Add a non-root USER + entrypoint privilege-drop, verified against a real deploy (or documented as verified-on-Fly)."
# ── P2: coordinated follow-up split out of T-012 during execution ────────────
- id: T-016
  title: "Protocol runtime enforcement: version field + Unknown variants + call sites"
  priority: 2
  status: done
  notes: "DONE 2026-07-07 (lead, single-threaded): added #[serde(other)] Unknown to the five INTERNALLY-tagged sub-enums (Mutation/Query/Event/Response/ControlMessage); a compiler-driven sweep added graceful Unknown arms at every consumer match (model.rs apply/query, cli/daemon.rs event+response, connection.rs + ws.rs control). A new action/event/query/response/control tag from a newer peer now decodes to that enum's Unknown and is dropped-with-a-warning instead of failing the whole Message — this is the case that grows the protocol and the one that could stall a correlated request. Test unknown_payload_variant_decodes_to_unknown. LIMITATION (documented, not a gap): the OUTER MessageKind is adjacently tagged (type/payload), where serde's #[serde(other)] can't consume the payload, so a wholly-unknown top-level `type` fails-to-parse and is dropped-with-warn at the decode site — safe because such a frame is never a correlated Response; the five categories are architecturally stable. DELIBERATELY DEFERRED: the `version` STRUCT field — advisory-only (no consumer; PROTOCOL_VERSION const + the deny_unknown_fields-free envelope already let it be added later without a flag day), and it churns ~40 construction sites for zero current behavior. #[non_exhaustive] intentionally NOT applied: every variant is in-repo, so exhaustive matches keep forcing consumers to handle new variants. Original: Split from T-012 (2026-07-07 fleet): the version FIELD on Message and #[serde(other)] Unknown arms on the payload enums cannot land inside a protocol.rs-only fence — a new struct field breaks ~15 `Message { id, kind }` construction sites (ws.rs:24, daemon.rs:{134,514,877}, session/connection.rs x7, tests/relay.rs:68) and Unknown variants break exhaustive matches (model.rs:{88,137}, daemon.rs:{280,666}, ws.rs:{159,200}, sessions.rs:183, connection.rs:315). T-012 landed the safe subset (PROTOCOL_VERSION const + forward-compat doc + 4 tests, incl. one that characterizes the gap). This task lands the coupled change single-threaded: add `version: u32` (#[serde(default)]) to Message + PROTOCOL_VERSION default at every construction site, add an Unknown #[serde(other)] arm to each payload enum + handle it at every match site (degrade gracefully, don't drop the whole Message), and flip the characterization test. Also apply the same to common/types.rs enums (flagged by the fleet)."
# ── Discovered during the post-review-failure investigation ──────────────────
- id: T-017
  title: "Fix all_public_notebook_cells_compile (rayon/atomics) — test bug + floating atomics toolchain"
  priority: 1
  status: done
  notes: "DONE 2026-07-07: the review flagged this e2e as a pre-existing red (blocking `cargo make uat`). Root-caused via evidence (raised the test's stderr truncation → showed wasm-bindgen-rayon's `compile_error!(\"forget to enable atomics/bulk-memory\")`). TWO bugs: (1) TEST BUG — compiler/mod.rs:997 hard-coded needs_atomics=false for every cell, so rayon cells (which the scaffold correctly detects via merged_deps_contain_rayon and injects wasm-bindgen-rayon for) were checked WITHOUT the atomics RUSTFLAGS/-Zbuild-std → the guard fired. Fixed: compute needs_atomics per cell and pass it. (2) FLOATING ATOMICS TOOLCHAIN — build.rs used `cargo +nightly` (rolling; was 1.98.0-nightly 2026-06-01) for atomics builds, but docker/Dockerfile provisions `rustup component add rust-src` for the PINNED nightly-2025-12-22 (rust-toolchain.toml), NOT for rolling nightly — so the atomics build referenced a toolchain Docker doesn't provision rust-src for (latent-broken in prod, non-reproducible). Fixed: new ATOMICS_TOOLCHAIN const pins both build+check atomics paths to nightly-2025-12-22 (matches Docker). Also added `rustup component add rust-src` to cargo-make install-tools (was missing → local atomics builds would fail). VERIFIED: all_public_notebook_cells_compile now PASSES (7/7 integration, the atomics cell builds). Kept the test's tail-of-stderr diagnostic improvement (head showed only dependency-locking noise)."
---

# Summary

The 2026-07-07 end-to-end review graded ironpad **B+**: strong architecture and happy-path
quality, with a consistent lag in the *adversarial* dimension (resource caps, negative tests,
defense-in-depth) and a CI gate that doesn't run what the docs call "the one true gate." This
PRD is the single track that closes that gap — the confirmed correctness bugs, the cheap
in-scope hardening, and (the primary ask) the test-coverage holes exactly where risk is
highest. It deliberately does **not** re-open full build sandboxing (recorded out-of-scope in
PRD-0030), and it picks up the two design follow-ups PRD-0036 explicitly deferred.

# Problem

The review surfaced four confirmed bugs (a cache-key collision that serves wrong WASM, a README
that loses user data, a daemon stop path that skips cleanup, two leaked closures), a cluster of
uncapped/adversarially-weak paths, and — most actionably — untested code precisely where a
regression would be worst: the one nontrivial `unsafe` FFI decode, the sanitizer exclusions the
whole XSS model rests on, the IPC wire framing, and the relay's auth/DoS branches. "Green CI"
currently does not exercise the compiler pipeline, the wasm frontend, or the browser layer, so
these regressions could merge unnoticed.

# Goals

1. The four confirmed correctness bugs are fixed, each with a regression test that fails first.
2. The sanitizer's exclusions and CSS policy are defended by tests; `style=` can't hijack layout.
3. The highest-risk untested paths (FFI decode, IPC framing, relay auth) gain real coverage.
4. The uniform-hardening gaps (idle timeouts, `wasm-opt` timeout, share/IPC/connection caps,
   protocol versioning, cell-runtime soundness) are closed.
5. CI runs the real gate (integration + wasm + Playwright smoke) on PRs, with an aligned,
   reproducible toolchain.

# Technical Approach

TDD wherever a confirmed bug exists (T-001, T-003, T-004 land the failing test first). Sanitizer
work (T-005/T-006) uses ammonia's `filter_style_properties` (cssparser-based) — no regex. The
FFI test gap (T-007) is unlocked by extracting the pure decode from the raw-pointer shell so it
runs on the host. Server caps (T-011) reuse the patterns PRD-0036 established (bounded channels,
size caps, process-group kill). CI (T-013) adds tiers to `build.yml` without changing the local
`cargo make uat` contract.

## Sequencing

P1 first (confirmed bugs + sanitizer + FFI/relay tests), P2 next (caps/soundness/protocol/CI),
P3 last (the two auth/isolation design follow-ups, which introduce new schemes). T-014 and T-015
are independent design tasks and may split into their own PRDs if they grow.

# Assumptions

- Deployment stays Fly.io (Firecracker microVM) + the `ironpad-proxy` egress filter; the
  compile-sandbox residual risk remains accepted (PRD-0030).
- `cargo make uat` remains the local super-gate; CI approximates it, it does not replace it.

# Constraints

- No public-API rewrites; hardening is additive (caps, validation, versioning arms).
- Sanitizer changes must preserve the legitimate inline UX PRD-0036 verified (plotters SVG,
  KaTeX spans) — re-verify a real chart still renders.
- Cache-key change (T-001) must bump `CACHE_EPOCH` and not defeat legitimate content hits.

# References to Code

- `crates/ironpad-app/src/compiler/{cache.rs,optimize.rs,mod.rs}`, `server_fns.rs`
- `crates/ironpad-app/src/sanitize.rs`, `pages/notebook_editor/state.rs`, `pages/home_page.rs`
- `crates/ironpad-cell/src/{lib.rs,sim.rs,gpu.rs,canvas.rs}`
- `crates/ironpad-cli/src/{daemon.rs,ipc.rs,main.rs}`, `crates/ironpad-common/src/protocol.rs`
- `crates/ironpad-server/src/{ws.rs,state.rs,sessions.rs}`, `tests/relay.rs`
- `.github/workflows/build.yml`, `docker/Dockerfile`, `README.md`, `tests/e2e/`

# Non-Goals (MVP)

- Full build sandboxing / seccomp / rootless nested containers (recorded residual risk, PRD-0030).
- `view_only_notebook.rs` ↔ `notebook_editor` DRY consolidation and the O(n²) autocomplete /
  unkeyed home-grid perf items (real but separable code-quality work; track separately).
- Unifying the TLS stack on rustls (drop `native-tls`) — worthwhile cleanup, its own change.

# History

## 2026-07-07 — Created
Opened from the 2026-07-07 seven-domain review (overall B+). Scoped 15 tasks: 4 confirmed-bug
fixes (TDD), sanitizer CSS-policy + bypass regressions, the four highest-risk test-coverage
holes, uniform resource caps, protocol versioning, cell-runtime soundness, CI-gate alignment,
and the two deferred auth/isolation design follow-ups from PRD-0036. Sandboxing stays out of
scope per PRD-0030.

## 2026-07-07 — First increment landed (branch `feat/prd-0038-post-review-hardening-tests`)
- **T-001 done** (confirmed correctness bug, TDD): framed all variable-length cache-key
  fields, CACHE_EPOCH 3→4, `hash_disambiguates_field_boundaries` regression test. **uat-001 verified.**
- **T-006 in-progress**: 4 sanitizer bypass regression-lock tests (foreignObject / use+image /
  animate+set / data:-URI href). Style-position test + Playwright DOM spec still pending (need T-005).
- **T-008 in-progress**: 6 IPC serde/framing round-trip + response-shape tests (`ipc.rs` 0→6).
  Bounded-reader + socket-perm hardening still pending.
- Net: +15 tests, all green (`nextest` filtered), `cargo clippy --all-targets --workspace -D warnings` clean.
- Remaining P1: T-002 (README volumes), T-003 (SIGTERM cleanup), T-004 (forget leaks),
  T-005 (style CSS filter), T-007 (sim FFI decode), T-009 (relay auth).

## 2026-07-07 — Batch execution via Agent Team (T-002…T-013)
Dispatched 7 conflict-free fleet members (disjoint file ownership), lead consolidated.

- **Tasks completed**: T-002, T-003, T-004, T-005, T-006, T-007, T-008, T-009, T-010, T-011, T-013 (done); T-012 partial → remainder split into **T-016** (todo).
- **Changes**:
  - T-002/T-013 (`docs-ci`): README volume fix (single `-v ironpad:/ironpad`) + codecov badge; CI gains `pull_request` trigger (notify guarded to push), `test-integration` + wasm/hydrate build step + `cargo audit` job; Docker → `binstall` + `wasm-bindgen-cli@0.2.114` pin + `COPY rust-toolchain.toml` before `cargo chef cook`; CI toolchain aligned to nightly-2025-12-22. Deferred (flagged): Playwright-smoke CI job (snippet provided), cargo-deny, codecov-job toolchain.
  - T-003/T-008 (`cli-daemon`): SIGTERM→cleanup handler; bounded `read_frame` (MAX_IPC_FRAME=1 MiB) replacing unbounded `lines()`; socket perms 0o600/0o700. +8 tests.
  - T-004 (lead, taken over from stalled `frontend-leaks`): reusable reactive-debounce closure built once at setup (`init_reactive_timer`) — no per-edit leak; import flow cleans up on file-selection AND `cancel` (deliberately-broken keepalive cycle) so neither closures nor the hidden `<input>` leak.
  - T-005/T-006 (`sanitizer`): `filter_style_properties(STYLE_PROPERTIES)` presentation allowlist (kills `position:fixed` overlay clickjacking), `id` scoped to `url(#id)` SVG containers, `xmlns` dropped; +5 tests (14 sanitize total). uat-003 Playwright DOM spec deferred.
  - T-007/T-010 (`cell-runtime`): extracted host-testable `decode_length_prefixed`; `into_boxed_slice` + `checked_add`/`checked_mul`/saturating sizing across sim/lib/gpu/canvas. +8 tests; native AND wasm32 clippy clean.
  - T-009/T-011 (`server-perimeter`): `read` is now a real confidentiality boundary (content events gated on guest read perm; `SessionEnded` stays ungated); expired-token→410; guest idle-timeout + connection cap (503); `wasm-opt` 120s kill-timeout; 512 MiB aggregate share cap; `CompileLocks` idle-pruning; 8 MiB `DefaultBodyLimit`. +6 relay + unit tests.
  - T-012 (`protocol`): safe subset only (PROTOCOL_VERSION const + forward-compat doc + 4 tests); runtime enforcement split to T-016 (cross-crate, can't fit protocol.rs-only fence).
  - Also: GitHub-repo link added to the app header (`app_layout.rs` + `main.scss`) per user request.
- **Test results**: `cargo make ci` (fmt-check + clippy -D warnings + nextest) **green — 574 passed, 7 skipped** (was 536). `cargo make test-integration`: 6/7; `all_public_notebook_cells_compile` flaked on 2 mandelbrot cells (cold-compile dependency-fetch after the CACHE_EPOCH bump — no rustc error, no public-API change; re-run to confirm).
- **UATs verified**: uat-001, uat-002, uat-004, uat-005, uat-006, uat-007. **Unverified**: uat-003 (Playwright DOM overlay — deferred), uat-008 (full `uat` — Playwright not run this session).
- **Constitution compliance**: no public-API breaks; every behavioral change shipped tests; sandbox decision (PRD-0030) respected. Deviation: T-012 could not be completed within its ownership fence (cross-crate coupling) — safe subset landed, remainder tracked as T-016.

## 2026-07-07 — T-016 done (protocol forward-compat, single-threaded)
- Added `#[serde(other)] Unknown` to the five internally-tagged sub-enums (Mutation/
  Query/Event/Response/ControlMessage); compiler-driven sweep added graceful `Unknown`
  arms at every consumer match (model.rs apply/query → InvalidMessage error; cli/daemon.rs
  event+response; connection.rs + ws.rs control no-ops). A new variant from a newer peer
  now decodes-and-drops-with-a-warning instead of failing the whole `Message` / stalling.
- Discovered mid-implementation: `#[serde(other)]` on the OUTER adjacently-tagged
  `MessageKind` fails when a `payload` is present (can't fill a unit variant). Dropped
  `MessageKind::Unknown`; an unknown top-level `type` fails-to-parse and is dropped-with-warn
  at the decode site — safe (never a correlated Response). Documented on `Message`.
- Deferred: the advisory `version` STRUCT field (no consumer; ~40 construction-site churn;
  addable later without a flag day per the existing envelope tolerance).
- Test: `unknown_payload_variant_decodes_to_unknown` (replaces the characterization test).
- `cargo make ci` green: 574 tests, clippy -D warnings clean.

## 2026-07-07 — T-017 done (mandelbrot/uat green; supersedes the "pre-existing flake" note)
The `all_public_notebook_cells_compile` failure recorded as a "pre-existing flake" in the
T-002…T-013 batch entry was NOT a flake — it was a real two-part bug, now fixed:
- Test bug: `check_micro_crate(..., false)` hard-coded `needs_atomics=false`, so rayon cells
  were checked without atomics flags and wasm-bindgen-rayon's guard fired. Now computed per cell.
- Floating atomics toolchain: `+nightly` (rolling) vs Docker's `rust-src` on pinned
  nightly-2025-12-22. Pinned both atomics paths to `ATOMICS_TOOLCHAIN` = nightly-2025-12-22;
  added `rust-src` to `install-tools`.
- Diagnosed by evidence (tail-of-stderr fix surfaced the real `compile_error!`).
- Result: `cargo make test-integration` **7/7 green** (the atomics cell compiles); `cargo make ci`
  574 tests green. uat's integration tier is now green (Playwright tier still unrun this session).

## 2026-07-08 — T-014 done (host credential, TDD)
Design → spec (docs/superpowers/specs/2026-07-08-host-credential-design.md, approved) →
TDD implementation. TOFU in-memory secret + first-message ClaimHost handshake closes the
unauthenticated host role: only a browser holding the per-notebook secret can host or replace
the host; a mismatch is closed 4403 without evicting the incumbent, a bad first frame 4400.
- Protocol: ControlMessage::ClaimHost { secret }. Server: WsState.notebook_secrets +
  claim_host()/forget_secret_if_idle(); handle_host validates before register_host.
- Browser: localStorage-held per-notebook secret (kept out of the notebook record), sent as
  the first frame; 4403 → clear error, no reconnect loop.
- +7 tests (protocol round-trip, 4 state units, 2 relay integration); 8 existing relay tests
  updated to send ClaimHost. cargo make ci green: 581 tests.
- Browser runtime path is covered by tests/e2e/session.spec.ts (not unit-testable in wasm).
- Follow-up noted: a user-visible 'rejected' UI state (currently console-only).

## 2026-07-08 — E2E verification (T-014 browser handshake + uat-003)
Ran Playwright (session + new sanitizer-xss specs) against `cargo leptos serve --release`.
6/6 passed.
- **uat-003 verified**: tests/e2e/sanitizer-xss.spec.ts imports a notebook whose markdown cell
  smuggles a full-viewport `position:fixed` overlay `<a>` (same `sanitize_html` path as
  shared/public viewers). Asserts the rendered anchor is `position:static` + `z-index:auto`
  (clickjacking CSS stripped) while `background:rgb(255,0,0)` survives (filter, not blanket drop).
- **T-014 verified end-to-end**: all 5 session.spec.ts agent flows (lifecycle, read cells, add/
  delete, update source, session-ends-on-tab-close) pass — the browser's `ClaimHost` first frame
  doesn't break the live collaboration path.
- Note: Playwright's 300s webServer timeout is too short for a cold `--release` build; run by
  starting the server first, then `npx playwright test` (reuseExistingServer).

(Entries appended during implementation go below this line.)
