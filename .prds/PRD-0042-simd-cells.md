---
id: PRD-0042
title: "WASM SIMD cell support (simd128 + portable_simd)"
status: active
owner: "Aaron Roney"
created: 2026-07-10
updated: 2026-07-10

principles:
- "Using the feature is the opt-in: cells that mention std::simd / core::simd / std::arch::wasm32 get the simd128 build, everything else is untouched (mirrors PRD-0041 autodiff detection)."
- "simd128 is baseline WASM SIMD (~95% browser coverage); relaxed-simd stays out until Safari unflags it."
- "No std rebuild and no pinned toolchain: mixing simd128 cell code with the precompiled non-simd std is valid, unlike atomics."

references:
- name: "WebAssembly SIMD browser support"
  url: https://caniuse.com/wasm-simd
- name: "Rust portable SIMD (std::simd)"
  url: https://doc.rust-lang.org/std/simd/index.html

acceptance_tests:
- id: uat-001
  name: "A cell using std::simd (portable SIMD) compiles to WASM successfully with v128 codegen enabled"
  command: cargo make test-integration
  uat_status: unverified
- id: uat-002
  name: "Full gate: ci + integration + Playwright pass with SIMD infra and notebook in place"
  command: cargo make uat
  uat_status: unverified

tasks:
- id: T-001
  title: "Detection + scaffold injection: uses_wasm_simd(), inject #![feature(portable_simd)] at crate root, preamble bump"
  priority: 1
  status: todo
  notes: "Substrings: std::simd, core::simd, std::arch::wasm32. Injection composes with the autodiff feature gate (each bumps preamble_lines by 1). Unit tests for detection and preamble mapping."
- id: T-002
  title: "RUSTFLAGS composition: merge target-features so simd128 cannot clobber atomics"
  priority: 1
  status: todo
  notes: "rustc takes the LAST -C target-feature flag; a naive '-C target-feature=+simd128' push would wipe +atomics,+bulk-memory,+mutable-globals on rayon cells. Split ATOMICS_RUSTFLAGS into target-feature list + link-arg flags and emit ONE merged -C target-feature. Unit test the composed flag string for atomics+simd."
- id: T-003
  title: "Cache key: needs_simd byte in content_hash"
  priority: 1
  status: todo
  notes: "After needs_autodiff byte. Test hash_changes_with_needs_simd."
- id: T-004
  title: "Thread needs_simd through server_fns compile path and the public-notebook check gate"
  priority: 2
  status: todo
  notes: "Compute pre-cache-check in compile_cell (like needs_autodiff); log it; same computation in compiler/mod.rs notebook gate for check parity."
- id: T-005
  title: "Integration test: portable SIMD cell builds end to end"
  priority: 2
  status: todo
  notes: "Follows compile_cell_with_std_autodiff_builds_successfully pattern; assert feature gate on generated line 1 and successful wasm build."
- id: T-006
  title: "Public notebook: scalar vs autovectorized vs explicit portable SIMD, live benchmarks"
  priority: 3
  status: todo
  notes: "Per-cell detection means a scalar baseline cell compiles WITHOUT simd128 while SIMD cells compile WITH it — one notebook shows all tiers honestly. Stopwatch benchmarks; expand_code true; my-voice rules."
---

# Summary

Compile cells that use Rust SIMD (`std::simd` portable SIMD or `std::arch::wasm32` intrinsics) with the `simd128` WASM target feature, and ship a public notebook that demonstrates scalar vs autovectorized vs explicit SIMD with live in-browser benchmarks.

# Problem

WASM SIMD128 is standardized with roughly 95% browser coverage, and Rust has a first-class portable SIMD API on nightly, but ironpad cells compile without the `simd128` target feature: `std::arch::wasm32` intrinsics fail to compile and `std::simd` falls back to scalar codegen. The notebook platform already runs nightly, so the only missing pieces are detection, a crate-root feature gate, and one codegen flag.

# Goals

1. Cells using `std::simd` / `core::simd` / `std::arch::wasm32` compile with `-C target-feature=+simd128` and `#![feature(portable_simd)]` injected at the generated crate root.
2. SIMD composes correctly with the existing atomics (rayon) and autodiff (Enzyme) special cases.
3. Cache correctness: the simd flag participates in the blake3 cache key.
4. A public notebook demonstrates the feature with honest, live benchmarks.

# Technical Approach

Mirror PRD-0041's autodiff plumbing end to end: `uses_wasm_simd(source, shared_source)` substring detection in `scaffold.rs`; crate-root `#![feature(portable_simd)]` injection with a preamble-line bump for diagnostics mapping; a `needs_simd` boolean threaded through `content_hash`, `build_micro_crate` / `check_micro_crate`, `server_fns::compile_cell`, and the public-notebook check gate.

The one structural change: `configure_cargo_cmd` must emit a **single merged** `-C target-feature=` flag because rustc keeps only the last occurrence of the option. `ATOMICS_RUSTFLAGS` splits into a target-feature list and a link-arg flag set; simd contributes `+simd128` to the merged list. Rolling nightly, no `-Zbuild-std`, no new toolchain, no executor JS changes (browsers instantiate simd128 modules natively).

# Assumptions

- The rolling nightly and both pinned toolchains (atomics 2025-12-22, autodiff 2026-06-01) support `portable_simd` and `+simd128` (verified by spike on the rolling nightly).
- Cache invalidation from the new hash input byte is acceptable (same precedent as PRD-0041).

# Constraints

- relaxed-simd is out of scope (behind a flag in Safari).
- Detection is per-cell over (source, shared_source): if shared source mentions SIMD, every cell in the notebook builds with simd128. The notebook design keeps SIMD helpers out of shared source where a scalar baseline is wanted.

# References to Code

- `crates/ironpad-app/src/compiler/scaffold.rs` — detection + injection (autodiff pattern at lines 67, 81-87)
- `crates/ironpad-app/src/compiler/build.rs` — `configure_cargo_cmd`, `ATOMICS_RUSTFLAGS`
- `crates/ironpad-app/src/compiler/cache.rs` — `content_hash`
- `crates/ironpad-app/src/server_fns.rs` — compile path threading
- `crates/ironpad-app/src/compiler/mod.rs` — notebook gate + e2e_tests

# Non-Goals (MVP)

- relaxed-simd support
- Auto-enabling simd128 for all cells (behavioral change for existing cells; old-browser breakage)

# History

- 2026-07-10: Created after spike verified portable_simd + simd128 on the rolling nightly (24 v128 ops in binary, correct execution in V8).
