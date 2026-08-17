---
id: PRD-0067
title: "Nightly pin collapse: four cell toolchains onto one"
status: draft
owner: "Aaron Roney"
created: 2026-08-17
updated: 2026-08-17

depends_on:
- PRD-0066

principles:
- "A pin outlives its reason silently, and a pin can be RIGHT for a reason its comment gets wrong. Test the requirement, not the stated mechanism."
- "Measure the artifact that ships. Local toolchains are default-profile; the image is --profile minimal, and the gap was 3x on one pin."
- "cargo check cannot see a codegen bug and cargo build cannot see a post-processing bug. Only the real pipeline discriminates, and this cost a wrong conclusion mid-PRD."
- "Enforce the invariant that costs money. CELL_TOOLCHAIN must equal BROWSERPOD_NIGHTLY or the image silently regrows by a whole toolchain."

references:
- name: "PRD-0041 (autodiff cells, the ICE that sets the ceiling)"
  url: https://github.com/twitchax/ironpad/blob/main/.prds/PRD-0041-autodiff-cells.md
- name: "PRD-0066 (Linux cells, which put nightly-2026-05-19 in the image)"
  url: https://github.com/twitchax/ironpad/blob/main/.prds/PRD-0066-browserpod-cells.md
- name: "wasm-bindgen-rayon atomics guard"
  url: https://github.com/RReverser/wasm-bindgen-rayon

acceptance_tests:
- id: uat-001
  name: "Every cell feature combination compiles on the single pin, through the real pipeline"
  command: cargo make test-integration
  uat_status: verified
- id: uat-002
  name: "A rayon cell survives the FULL pipeline, wasm-bindgen threading transform included"
  command: cargo make test-integration
  uat_status: verified
- id: uat-003
  name: "CELL_TOOLCHAIN equals BROWSERPOD_NIGHTLY, and the image installs what the constants name"
  command: cargo make test
  uat_status: verified
- id: uat-004
  name: "The workspace builds and lints clean on the collapsed pin"
  command: cargo make ci
  uat_status: verified
- id: uat-005
  name: "Public notebooks still run in a browser after the compiler rollback"
  command: cargo make playwright
  uat_status: verified

tasks:
- id: T-001
  title: "Point CELL_TOOLCHAIN at nightly-2026-05-19 and document the ceiling and floor"
  priority: 1
  status: done
  notes: "Autodiff sets the ceiling, BrowserPod's ABI-tied rlibs set the floor. One date between them."
- id: T-002
  title: "Delete ATOMICS_TOOLCHAIN and AUTODIFF_TOOLCHAIN; route cell_toolchain on target alone"
  priority: 1
  status: done
  notes: "Features keep their RUSTFLAGS and profile; only the toolchain split collapses."
- id: T-003
  title: "Move rust-toolchain.toml onto CELL_TOOLCHAIN"
  priority: 1
  status: done
  notes: "Its thaw justification died in v0.12.13. Spiked before moving; 961/963 passed."
- id: T-004
  title: "Collapse docker/Dockerfile and .github/workflows/build.yml to one nightly"
  priority: 1
  status: done
  notes: "Warmups and the baked toolchain-versions file move with it."
- id: T-005
  title: "Add browserpod_pin_matches_cell_toolchain"
  priority: 2
  status: done
  notes: "Drift costs a whole toolchain in the image and breaks nothing, so no other signal would fire."
- id: T-006
  title: "Fix the two clippy lints the stale workspace pin was masking"
  priority: 2
  status: done
  notes: "explicit_counter_loop and unnecessary_trailing_comma, both in a PRD-0066 test."
- id: T-007
  title: "Upgrade the wasm-bindgen CLI and crate family 0.2.114 -> 0.2.127"
  priority: 1
  status: done
  notes: "The real blocker for rayon on modern nightlies. Pins live in Makefile.toml, build.yml and the Dockerfile; js-sys/web-sys move with it."
- id: T-008
  title: "Add compile_rayon_cell_links_a_shared_memory_module"
  priority: 1
  status: done
  notes: "Nothing built a rayon cell before, which is why the pin's real constraint went unmeasured for so long."
- id: T-009
  title: "Update CLAUDE.md's toolchain section"
  priority: 3
  status: done
  notes: "The three-pin table and the CACHE_EPOCH trap it documents are both obsolete."
---

# Summary

Cell compilation ran on four pinned toolchains. It now runs on one,
`nightly-2026-05-19`, which the deploy image was already carrying for
BrowserPod. Toolchains in the runtime image drop from 2,938,996,672 bytes to
795,512,039: **2.14 GB saved, 73% of the toolchain bytes**, measured by
installing each set into a `rust:1.93.0-slim` container exactly as the
Dockerfile does.

# Problem

Four nightlies, each held where some feature was known-good:

| pin | held for | still true? |
| --- | --- | --- |
| `nightly-2026-07-14` | normal + SIMD, tracked fresh | yes |
| `nightly-2026-06-01` | `std::autodiff`, has `enzyme` | yes |
| `nightly-2025-12-22` | rayon, wasm-bindgen-rayon's atomics guard | **wrong mechanism, real constraint** |
| `nightly-2026-05-19` | BrowserPod's ABI-tied prebuilt rlibs | yes, immovable |

Plus `rust-toolchain.toml`, pinned to `nightly-2025-12-22` and justified in a
comment by the `thaw` UI dependency, which was deleted in v0.12.13 and is absent
from `Cargo.toml` and `Cargo.lock`.

The `thaw` justification was simply dead. The atomics one was more interesting
and nearly caused a bad ship: the stated mechanism was false, so testing the
stated mechanism said "stale", while the pin itself was load-bearing for a
reason nobody had written down.

Neither could fail on its own. A pin held for a reason that stopped being true
keeps working, and the only way to notice is to re-test it deliberately.

The cost was not only bytes. `AUTODIFF_TOOLCHAIN` and `ATOMICS_TOOLCHAIN` were
not part of the compile-cache fingerprint, which tracks `CELL_TOOLCHAIN` alone,
so bumping either one required a manual `CACHE_EPOCH` bump to invalidate stale
blobs. That is a rule living in a comment, which is where rules go to be
forgotten.

# Goals

1. One nightly for every cell build.
2. Keep every feature working: SIMD, gen blocks, coroutines, blocking/JSPI,
   rayon, autodiff, Linux.
3. Make the remaining coupling enforced rather than documented.
4. Remove the manual `CACHE_EPOCH` obligation by construction.

# Technical Approach

## The date is forced

`nightly-2026-05-19` is not a preference. It is the only date that works.

**Autodiff is the ceiling.** Nightlies from July 2026 fail with
`error: internal compiler error: incorrect autodiff typetree handling for slice`.
Because `-Zautodiff=Enable` is a RUSTFLAG applied to the whole crate graph, the
ICE fires while compiling a transitive dependency (`icu_normalizer`), not user
code, so no cell-side workaround exists.

**BrowserPod is the floor.** The pack ships 22 prebuilt rlibs ABI-tied to this
rustc, and its `libexec/rustc` and `libexec/cargo` are symlinks into
`nightly-2026-05-19`, which its installer pulls with `--profile minimal`. The
toolchain is in the image whether or not anything else uses it.

Between a hard ceiling and an immovable floor sits one usable date, and the
image had already paid for it.

## What was measured, including the part that was measured wrong

The autodiff ceiling held up under every check:

| workload | 2025-12-22 | 2026-05-19 | 2026-06-01 | 2026-07-14 |
| --- | --- | --- | --- | --- |
| normal / SIMD / gen blocks | n/a | 16/16 pipeline | n/a | previous pin |
| autodiff (production shape) | n/a | real pipeline | previous pin | **ICE** |

Rayon did not, and the way it failed is the useful part of this PRD.

**First pass, and it was wrong.** A rayon cell was built by hand on all three
nightlies. All three produced ~1.02 MB blobs exporting `__wasm_init_tls`,
`__tls_size`, `__tls_align` and `__tls_base` with an imported memory, and
`libwasm_bindgen_rayon.rlib` was present in the graph, proving the
`compile_error!` guard had been evaluated and had not fired. Conclusion drawn:
the atomics pin is stale. That conclusion was reported and it was false.

**What it missed.** `build_micro_crate` does not stop at `cargo build`. It runs
`wasm-bindgen` afterward, and that step fails on every nightly newer than the
pin:

```
error: failed to prepare module for threading
Caused by: failed to find `__heap_base` for injecting thread id
```

This is the same class of error as testing with `cargo check` when Enzyme runs
during LTO, one level further out: `cargo build` cannot see a post-processing
failure. The pin's comment named wasm-bindgen-rayon's `compile_error!` guard,
that mechanism was disproved, and the pin was declared stale on the strength of
disproving the wrong thing.

**The actual root cause is the wasm-bindgen CLI, not rustc.** Isolated by
running two CLI versions over the *same* `nightly-2026-05-19` blob:

| wasm-bindgen CLI | result on an identical blob |
| --- | --- |
| 0.2.114 (pinned) | `failed to find __heap_base for injecting thread id` |
| 0.2.127 | succeeds |

So `ATOMICS_TOOLCHAIN` was holding rustc two years back to accommodate a CLI
thirteen patch versions stale. Upgrading the CLI fixes the cause; pinning the
compiler was treating the symptom. The full pipeline test now covers this, which
nothing did before: the only rayon coverage was
`all_public_notebook_cells_compile`, and it calls `check_micro_crate`.

## Sizes

The first numbers taken were wrong by 3x on one pin, because local toolchains
are default-profile and the image installs `--profile minimal`. 758 MB of the
2.1 GB `nightly-2025-12-22` is documentation the image never installs. Measured
inside a real image instead:

| toolchain | before | after |
| --- | --- | --- |
| `nightly-2026-05-19` (BrowserPod's, minimal) | 591 MB | — |
| `nightly-2025-12-22` (atomics) | 717 MB | — |
| `nightly-2026-07-14` (cells) | 756 MB | — |
| `nightly-2026-06-01` (autodiff, + `enzyme`) | 774 MB | — |
| `nightly-2026-05-19` (everything, + `enzyme`) | — | 772 MB |
| **total** | **2,938,996,672 B** | **795,512,039 B** |

2.14 GB saved, 73% of the toolchain bytes. The BrowserPod pack itself is 126 MB
and sits on top of both columns unchanged. Note the collapsed toolchain is
772 MB against the 591 MB the pack was already pulling, so the entire cost of
serving every other cell from it is 181 MB of components.

## What did not change

The toolchain split collapsed. The feature handling did not. Atomics cells still
get `+atomics,+bulk-memory,+mutable-globals`, the shared-memory link args and
`-Zbuild-std`; autodiff cells still get `-Zautodiff=Enable` and a forced fat-LTO
profile. Cargo fingerprints those builds apart by RUSTFLAGS and profile exactly
as it previously did by rustc version, so they stay warm side by side in
`targets/default` without churn. The atomics warmup survives as its own image
layer for the same reason: a `-Zbuild-std` std rebuilt with `+atomics` is its
own sysroot, and sharing a compiler with the plain warmup does nothing to merge
the two.

## The one coupling worth a test

`CELL_TOOLCHAIN` must equal `BROWSERPOD_NIGHTLY` in `docker/browserpod.env`.
Let them drift and nothing breaks; the image just quietly carries two full
toolchains again. `browserpod_pin_matches_cell_toolchain` fails the build
instead, so a BrowserPod bump surfaces the decision.

This is the accepted cost of the collapse: a BrowserPod pack upgrade now moves
the compiler for every cell in the product, and re-verifying autodiff and rayon
on the new nightly becomes part of taking that upgrade.

# Assumptions

- Cells lose nothing by moving from 2026-07-14 back to 2026-05-19. Verified
  against the whole integration suite, including gen blocks, coroutines, SIMD,
  blocking/JSPI and edition 2024.
- BrowserPod 3.0.0 stays pinned to `nightly-2026-05-19`. A pack bump invalidates
  this and the test above is what says so.

# Constraints

- The compile cache invalidates on deploy. `CELL_TOOLCHAIN` feeds the
  fingerprint, so the rustc version string changes and every cached blob is
  cold. This needs no `CACHE_EPOCH` bump, and after this PRD there is only one
  pin and it is inside the fingerprint, so the manual-bump trap is gone.
- Prod needs re-warming after deploy (`cargo make warm-prod`).
- The wasm-bindgen upgrade moves `js-sys` and `web-sys` (0.3.91 to 0.3.104) with
  it. Leptos 0.8 needed no change, and the CLI pin is duplicated in
  `Makefile.toml`, `.github/workflows/build.yml` and `docker/Dockerfile`, all
  three of which must move together or cells get a crate version their
  post-processor cannot read.

# References to Code

- `crates/ironpad-app/src/lib.rs` (`CELL_TOOLCHAIN`)
- `crates/ironpad-app/src/compiler/build.rs` (`cell_toolchain`, `BROWSERPOD_TOOLCHAIN`, the sync tests)
- `rust-toolchain.toml`
- `docker/Dockerfile`, `docker/browserpod.env`
- `.github/workflows/build.yml`

# Non-Goals (MVP)

- Bumping BrowserPod. The pack version is untouched.
- Removing the atomics warmup layer or the autodiff RUSTFLAGS path.
- Chasing the newest wasm-bindgen for its own sake. 0.2.127 is the version that
  makes rayon work on a modern nightly; that is the whole reason it moved.
- Installing clang in the image, which is tracked separately.

# History

## 2026-08-17

Shipped. Four nightlies to one, measured at 2,938,996,672 bytes down to
795,512,039 (2.14 GB, 73%).

Gates: `cargo make ci` 963 tests, `cargo make test-integration` 17/17,
`cargo make playwright` 166 passed, `cargo make docker-build` clean.

Confirmed in the built image rather than only in the container probes: two
toolchains, `browserpod-3.0.0` at 126 MB and `nightly-2026-05-19` at 772 MB,
898 MB total against four nightlies before. The baked fingerprint reads
`rustc 1.97.0-nightly (9eb3be26b 2026-05-18)` and `wasm-bindgen 0.2.127`, which
is the pair the cache key is built from.

Two corrections worth recording, because both were reported before they were
true.

**The atomics pin was not stale.** Its comment blamed wasm-bindgen-rayon's
`compile_error!` guard; that mechanism was disproved (rayon compiles clean five
months past the pin, the guard never fires, the blob carries every TLS export)
and the pin was declared stale on the strength of it. `build_micro_crate` does
not stop at `cargo build`. It runs `wasm-bindgen`, and that step failed on every
newer nightly. Running two CLI versions over one identical blob put the cause
where it belonged: 0.2.114 could not read a modern nightly's output and 0.2.127
can, so the pin had been holding rustc back to accommodate a stale CLI.
Upgrading the CLI dissolved it.

**The first e2e run failed 11 specs and that was cold cache, not breakage.**
Changing `CELL_TOOLCHAIN` changes the fingerprint, so every cell recompiled;
cells sat in `compiling` past a 120s timeout whose spec comment already says it
assumes a warm cache. A second run passed 166 of 166 in half the wall time.

The method note: `cargo check` cannot see a codegen bug and `cargo build` cannot
see a post-processing bug. The second error was made in the same session that
warned about the first.
