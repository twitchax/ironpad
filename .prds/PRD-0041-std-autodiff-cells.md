---
id: PRD-0041
title: "std::autodiff in cells: Enzyme-powered derivatives on the wasm pipeline"
status: active
owner: "Aaron Roney"
created: 2026-07-10
updated: 2026-07-10

principles:
- "Opt-in is automatic and invisible: using std::autodiff in a cell is the opt-in, exactly like rayon deps opting into the atomics build"
- "The scaffold owns the generated crate root, so crate-root requirements (feature gate, profile) are ironpad's job, never the notebook author's"
- "Every pipeline input that changes codegen must be in the cache key"

references:
- name: "std::autodiff nightly docs"
  url: https://doc.rust-lang.org/nightly/std/autodiff/index.html
- name: "-Zautodiff unstable-book entry"
  url: https://doc.rust-lang.org/nightly/unstable-book/compiler-flags/autodiff.html
- name: "Feasibility spike (session research + probes)"
  url: https://github.com/rust-lang/rust/issues/124509

acceptance_tests:
- id: uat-001
  name: "A cell using #[autodiff_reverse] through a data-dependent loop compiles to wasm via the real pipeline"
  command: cargo make test-integration
  uat_status: verified
- id: uat-002
  name: "Full gate stays green"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "Enzyme allocator shims in ironpad-cell (wasm32-only)"
  priority: 1
  status: done
  notes: "malloc(usize)/free(ptr)/realloc(ptr, u64 — Enzyme declares a 64-bit size)/calloc backed by the Rust global allocator with a 16-byte size header. Verified signatures from the spike; module compiled unconditionally on wasm32 (inert unless Enzyme calls them; nothing else defines these symbols on wasm32-unknown-unknown)."
- id: T-002
  title: "needs_autodiff detection + scaffold crate-root support"
  priority: 1
  status: done
  notes: "Pure fn on (source, shared_source): contains autodiff_forward | autodiff_reverse | std::autodiff. When true: generate_lib_rs prepends #![feature(autodiff)] (preamble_lines += 1 — diagnostics mapping), and generate_cargo_toml enforces lto=\"fat\" + codegen-units=1 in [profile.release], overriding user keys while preserving the rest."
- id: T-003
  title: "Build invocation: +nightly toolchain + RUSTFLAGS"
  priority: 1
  status: done
  notes: "configure_cargo_cmd gains needs_autodiff: select the nightly toolchain (runtime image default is stable — mirror the atomics path) and set RUSTFLAGS=-Zautodiff=Enable, composing with ATOMICS_RUSTFLAGS if both apply. check_micro_crate inherits, so the notebook gate compiles autodiff cells identically."
- id: T-004
  title: "Cache key includes needs_autodiff"
  priority: 1
  status: done
  notes: "content_hash gains the bool; server_fns computes it pre-cache-check next to needs_atomics."
- id: T-005
  title: "Docker runtime: enzyme component"
  priority: 2
  status: done
  notes: "rustup component add enzyme --toolchain nightly in the runtime stage (component distributes libEnzyme-22.so matched to the nightly's LLVM)."
- id: T-006
  title: "Tests: detection/scaffold/cache units + pipeline integration"
  priority: 2
  status: done
  notes: "Unit: detection strings, feature-line + preamble count, profile enforcement, hash divergence. Integration (#[ignore]): compile a reverse-mode-through-a-loop cell end to end (mirrors compile_cell_with_host_imports_links_successfully)."
- id: T-007
  title: "Cannon notebook: 'Teaching the compiler to aim a cannon'"
  priority: 3
  status: todo
  notes: "Drag ballistics sim in shared_source with #[autodiff_reverse]; cells: trajectory plot, gradient vs finite-difference table, gradient-descent aiming with animated convergence; expand_code: true. Ships after infra."

---

# Summary

Cells can use the real `std::autodiff` (`#[autodiff_forward]` / `#[autodiff_reverse]`): the pipeline detects usage, emits the crate-root feature gate and fat-LTO profile in the generated micro-crate, builds on nightly with `-Zautodiff=Enable`, and ironpad-cell supplies the C allocator symbols Enzyme's tape needs on wasm32.

# Problem

`std::autodiff` is the flagship "compiler learns calculus" feature and the existing autodiff notebook can only hand-roll the algorithms: the feature needs a crate-root `#![feature(autodiff)]` (impossible to type in a cell), a fat-LTO profile, a `-Z` flag, an Enzyme artifact, and — discovered in the spike — C allocator symbols that wasm32-unknown-unknown doesn't have. Every one of those is pipeline-owned, so the pipeline can make it *just work*.

# Technical Approach

Feasibility fully verified on this machine (2026-07-10): `rustup component add enzyme` on nightly 2026-06-01; a reverse-mode gradient **through a data-dependent drag-simulation loop**, compiled for wasm32-unknown-unknown and executed under wasmtime, matches central finite differences to 4 decimal places (−110.6514). Two non-obvious findings baked into the design: Enzyme's tape calls C `malloc`/`free`/`realloc` (with a **64-bit** `realloc` size) which must be shimmed on wasm32, and the fat-LTO requirement is enforced by rustc per-crate, so the micro-crate profile must be rewritten, not merely appended.

The opt-in shape mirrors rayon/atomics exactly: detect (source substring) → hash → scaffold (crate root + profile) → build (toolchain + RUSTFLAGS). Compile cost is negligible for micro-crates (0.3s incremental in the spike despite fat LTO + codegen-units=1).

# Constraints

- Nightly-only (the runtime image's default cell toolchain is stable → per-build `+nightly`, as atomics already does).
- Enzyme component must track the installed nightly (re-add after toolchain updates; Docker installs both together at image build).
- `#[autodiff_*]` functions belong in shared source (module scope); fn-body placement is untested and not promised.

# Non-Goals (MVP)

- `std::offload` / GPU, batching attributes.
- Autodiff on the editor's stable-toolchain fast path beyond the automatic nightly switch.
- Surfacing Enzyme's own diagnostics beyond normal rustc JSON output.

# History

- 2026-07-10: Created after the feasibility spike (rustup enzyme component; wasm32 verified end to end including the allocator-shim discovery).
