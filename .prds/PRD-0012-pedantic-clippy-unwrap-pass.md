---
id: PRD-0012
title: "Pedantic Clippy Lints and Unwrap Elimination"
status: done
owner: "Aaron Roney"
created: 2026-03-13
updated: 2026-03-14

principles:
- "Enable clippy::pedantic workspace-wide — raise the quality bar permanently"
- "Replace unwrap() with proper error handling in non-test library code only"
- "Tests and examples may keep unwrap()/expect() for readability"
- "Allow specific pedantic lints only when justified with a comment"
- "Audit Box/Arc usage for unnecessary indirection, but don't over-optimize"

references:
- name: "Workspace Cargo.toml"
  url: Cargo.toml
- name: "Clippy lint list"
  url: https://rust-lang.github.io/rust-clippy/master/
- name: "Makefile.toml clippy task"
  url: Makefile.toml

acceptance_tests:
- id: uat-001
  name: "cargo make clippy passes with clippy::pedantic enabled workspace-wide"
  command: cargo make clippy
  uat_status: verified
- id: uat-002
  name: "No unwrap() calls in non-test library code (excluding justified expect() with messages)"
  command: cargo make clippy
  uat_status: verified
- id: uat-003
  name: "All existing tests continue to pass after refactoring"
  command: cargo make test
  uat_status: verified
- id: uat-004
  name: "Full CI pipeline passes"
  command: cargo make ci
  uat_status: verified

tasks:
- id: T-001
  title: "Enable clippy::pedantic workspace-wide in Cargo.toml"
  priority: 1
  status: done
  notes: "Add workspace-level clippy configuration in Cargo.toml [workspace.lints.clippy] section. Enable pedantic group. Add targeted #[allow(...)] for specific lints that are too noisy or inapplicable (e.g., module_name_repetitions, missing_errors_doc). Document each allow with a brief justification. Run cargo clippy to get the initial list of warnings."

- id: T-002
  title: "Fix pedantic clippy warnings in ironpad-common"
  priority: 1
  status: done
  notes: "Fix all pedantic clippy warnings in crates/ironpad-common/. This crate has ~5 unwrap() calls and defines shared types (IronpadNotebook, protocol, etc.). Common fixes: missing docs, needless pass by value, manual implementations that could use derive. Replace unwrap() with proper error handling."

- id: T-003
  title: "Fix pedantic clippy warnings in ironpad-cell"
  priority: 1
  status: done
  notes: "Fix all pedantic clippy warnings in crates/ironpad-cell/. This crate has ~56 unwrap() calls, mostly in prelude and UI code. Many are in WASM FFI boundaries where panics are acceptable — convert these to expect() with descriptive messages. For non-FFI code, use proper error handling with anyhow or Results."

- id: T-004
  title: "Fix pedantic clippy warnings in ironpad-app"
  priority: 1
  status: done
  notes: "Fix all pedantic clippy warnings in crates/ironpad-app/. This is the largest crate with ~77 unwrap() calls spread across compiler/ (scaffold, cache, mod, diagnostics), pages/, storage/, model, session. The compiler modules should use anyhow::Result. UI components may use expect() with messages for Leptos signal access patterns where None is a logic error."

- id: T-005
  title: "Fix pedantic clippy warnings in ironpad-server"
  priority: 2
  status: done
  notes: "Fix all pedantic clippy warnings in crates/ironpad-server/. Only ~3 unwrap() calls. Focus on session management, WebSocket relay, and state modules. Should be straightforward — mostly type annotation and documentation fixes."

- id: T-006
  title: "Fix pedantic clippy warnings in ironpad-cli"
  priority: 2
  status: done
  notes: "Fix all pedantic clippy warnings in crates/ironpad-cli/. ~7 unwrap() calls in daemon.rs and main.rs. Replace with anyhow error handling. CLI binary code can use anyhow for top-level error handling."

- id: T-007
  title: "Fix pedantic clippy warnings in ironpad-frontend"
  priority: 2
  status: done
  notes: "Fix pedantic clippy warnings in crates/ironpad-frontend/. This is the minimal WASM hydration crate — likely very few issues. Should be a quick pass."

- id: T-008
  title: "Audit and optimize Box/Arc usage across workspace"
  priority: 3
  status: done
  notes: "Review all Box and Arc usage for unnecessary indirection. Key areas: ironpad-server state (Arc<RwLock<...>>), ironpad-cli daemon state (Arc<DaemonState>), ironpad-app compiler scaffold (Box for WASM FFI). The FFI Box usage is justified. Look for cases where owned values or references would suffice instead of Arc, or where Box<dyn Trait> could be replaced with enum dispatch or generics."

- id: T-009
  title: "Verify full CI and test suite passes"
  priority: 1
  status: done
  notes: "Run cargo make ci to verify fmt-check + clippy + test all pass. Run cargo make test-integration for the full integration test suite. Fix any regressions introduced by the refactoring. This is the final gate before marking the PRD as done."
---

# Summary

Enable `clippy::pedantic` lints workspace-wide, eliminate `unwrap()` calls from non-test library code, and audit `Box`/`Arc` usage for unnecessary indirection. This raises the code quality bar permanently and prevents future regressions.

---

# Problem

The codebase currently runs clippy with only `-D warnings` (default lints). There are ~148 `unwrap()` calls in non-test code, many of which could panic at runtime instead of returning proper errors. No pedantic lints are enforced, meaning common code quality issues (missing docs, needless clones, suboptimal patterns) go undetected. This is a liability for a project approaching release readiness.

---

# Goals

1. Enable `clippy::pedantic` workspace-wide so all future code is held to a higher standard.
2. Eliminate `unwrap()` from non-test library code — replace with `?`, `.context()`, or `expect("descriptive message")`.
3. Audit `Box`/`Arc` for unnecessary heap allocation or reference counting.
4. All existing tests continue to pass after the refactoring.

---

# Technical Approach

## Phase 1: Enable Lints

Add workspace-level clippy configuration in `Cargo.toml`:

```toml
[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
# Targeted allows for noisy/inapplicable lints:
module_name_repetitions = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"
# ... (determine full list from initial clippy run)
```

Each crate's `Cargo.toml` inherits via:

```toml
[lints]
workspace = true
```

## Phase 2: Fix Per-Crate

Work through each crate, fixing warnings. Common patterns:
- **`unwrap()` → `?` or `.context("message")`**: For fallible operations in functions returning `Result`.
- **`unwrap()` → `expect("reason")`**: For cases where `None`/`Err` is a logic bug (e.g., Leptos signal access).
- **Needless clone/borrow**: Remove unnecessary `.clone()` or `&*` patterns.
- **Missing `#[must_use]`**: Add to public functions that return values.
- **Type complexity**: Simplify complex type signatures with type aliases.

## Phase 3: Box/Arc Audit

Review heap allocation patterns:
- `Arc<RwLock<...>>` in server state — likely justified for concurrent access.
- `Box::into_raw` in compiler scaffold — justified for WASM FFI.
- Look for `Box<dyn Trait>` that could be enum dispatch.
- Look for `Arc` where `Rc` would suffice (single-threaded contexts).

---

# Assumptions

- `clippy::pedantic` can be enabled workspace-wide via `[workspace.lints.clippy]` (stable Rust 1.74+).
- Some pedantic lints will need `#[allow(...)]` at the item level where they're genuinely inapplicable.
- The number of fixes is manageable (~148 unwrap calls + pedantic warnings).

---

# Constraints

- Tests and examples are exempt from `unwrap()` elimination — readability is prioritized there.
- Do not change public API signatures unless strictly necessary for error handling.
- `expect()` with a descriptive message is acceptable where `None`/`Err` indicates a logic bug.
- Do not remove `unsafe` blocks — they are necessary for WASM FFI and are out of scope.

---

# References to Code

- `Cargo.toml` — Workspace configuration (lints will be added here).
- `Makefile.toml` — Clippy task currently runs `cargo clippy --all-targets -- -D warnings`.
- `crates/ironpad-common/src/` — Shared types, protocol (~5 unwraps).
- `crates/ironpad-cell/src/` — Cell runtime, prelude, UI code (~56 unwraps).
- `crates/ironpad-app/src/` — Core app: compiler, pages, storage, model (~77 unwraps).
- `crates/ironpad-server/src/` — HTTP server, WebSocket relay, sessions (~3 unwraps).
- `crates/ironpad-cli/src/` — CLI daemon and commands (~7 unwraps).
- `crates/ironpad-frontend/src/` — WASM hydration entry point (~0 unwraps).

---

# Non-Goals (MVP)

- Enabling `clippy::nursery` or `clippy::restriction` lint groups.
- Rewriting `unsafe` blocks or WASM FFI patterns.
- Adding comprehensive documentation to all public items (just enough to satisfy pedantic).
- Refactoring architecture to reduce Arc/Box usage (audit and document only if changes are non-trivial).

---

# History

## 2026-03-14 — Batch Execution (T-001 through T-009)
- **Tasks completed**: T-001, T-002, T-003, T-004, T-005, T-006, T-007, T-008, T-009
- **Changes**:
  - T-001: Added `[workspace.lints.clippy]` with pedantic + 8 targeted allows. All 6 crates inherit via `[lints] workspace = true`.
  - T-002: Fixed 1 warning in ironpad-common (doc_markdown in types.rs)
  - T-003: Fixed 13 warnings in ironpad-cell (unwrap→expect, format_push_string→write!, cast allows, assigning_clones)
  - T-004: Fixed 159 warnings in ironpad-app (~25 files: if_not_else, format_push_string, redundant_closure, cast lints, doc_markdown, manual_let_else, needless_pass_by_value, unnecessary_wraps, etc.)
  - T-005: Fixed warnings in ironpad-server (merged match arms, doc backticks, unwrap→expect)
  - T-006: Fixed 17 warnings in ironpad-cli (map_unwrap_or, clone_from, unwrap→expect, etc.)
  - T-007: Fixed 1 warning in ironpad-frontend
  - T-008: Audited all 7 Box/Arc usages — all justified (Arc for Axum shared state, Arc for tokio tasks, Box::into_raw for WASM FFI). No changes needed.
  - T-009: Verified CI: 295 tests pass, 3 integration tests pass, 0 clippy warnings
- **Test results**: `cargo make ci` passes (295 tests, 0 warnings)
- **UATs verified**: uat-001, uat-002, uat-003, uat-004
- **Constitution compliance**: No violations

---
