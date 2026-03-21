---
id: PRD-0016
title: "Per-Cell Cargo.toml Fix & Shared Deps Migration"
status: done
owner: "Aaron Roney"
created: 2026-03-14
updated: 2026-03-15

principles:
- "Per-cell Cargo.toml edits must take effect on the next compile"
- "Shared dependencies should be the default — per-cell overrides are the exception"
- "Public demo notebooks should model best practices for new users"

references:
- name: "Cargo.toml merge logic"
  url: crates/ironpad-app/src/compiler/scaffold.rs

acceptance_tests:
- id: uat-001
  name: "Editing a cell's Cargo.toml and recompiling picks up the new dependency"
  command: cargo make playwright
  uat_status: unverified
- id: uat-002
  name: "Cache key includes per-cell Cargo.toml content (changing deps invalidates cache)"
  command: cargo make test
  uat_status: unverified
- id: uat-003
  name: "All public notebooks compile and execute with shared deps"
  command: cargo make uat
  uat_status: unverified
- id: uat-004
  name: "New notebooks default to shared Cargo.toml with standard deps"
  command: cargo make playwright
  uat_status: unverified
- id: uat-005
  name: "CI passes"
  command: cargo make ci
  uat_status: verified

tasks:
- id: T-001
  title: "Investigate per-cell Cargo.toml bug"
  priority: 1
  status: done
  notes: "Investigation found no bug. Pipeline correctly passes cargo_toml through cache hash, scaffold, and server fn. Issue is likely UX confusion with empty cargo_toml defaults."
- id: T-002
  title: "Fix per-cell Cargo.toml (based on T-001 findings)"
  priority: 1
  status: done
  notes: "No fix needed — pipeline is correctly implemented. Per-cell cargo_toml is included in cache hash, passed to scaffold, and merged with shared deps."
- id: T-003
  title: "Migrate public notebooks to maximize shared deps"
  priority: 2
  status: done
  notes: "For all 21 public notebooks: move as many dependencies as possible from per-cell cargo_toml to the notebook's shared_cargo_toml. Per-cell cargo_toml should only contain the minimal [package]/[lib]/[dependencies] ironpad-cell skeleton (or be empty if the scaffold provides defaults). Keep per-cell overrides only when a cell genuinely needs a unique dependency not shared by other cells. The common deps to share: plotters, serde, serde_json, reqwest, rand, image, etc."
- id: T-004
  title: "Default new notebooks to shared Cargo.toml with standard deps"
  priority: 2
  status: done
  notes: "When creating a new notebook, pre-populate shared_cargo_toml with the standard dependency block (ironpad-cell, and common crates like serde). Per-cell cargo_toml should be minimal or empty by default. Update the notebook creation logic in model.rs or wherever the default notebook template is defined. Also update the default per-cell Cargo.toml template to be minimal (just [package] and ironpad-cell)."

---

# Summary

Fix the per-cell Cargo.toml not taking effect, then migrate all public notebooks to use shared dependencies as the primary pattern and set this as the default for new notebooks.

---

# Problem

1. **Per-cell Cargo.toml appears broken**: Editing a cell's individual Cargo.toml doesn't seem to affect compilation. This could be a cache invalidation issue (hash doesn't include cell cargo_toml), a storage issue (edits not saved), or a scaffolding issue (cell cargo_toml not passed through).

2. **Inconsistent dependency management**: Public notebooks have full Cargo.toml blocks in every cell, duplicating deps. The shared_cargo_toml feature exists but isn't used consistently. New notebooks don't default to shared deps.

---

# Goals

1. Per-cell Cargo.toml edits take effect on the next compile.
2. Cache is properly invalidated when per-cell Cargo.toml changes.
3. All public notebooks use shared_cargo_toml as the primary dependency source.
4. New notebooks default to shared deps with a minimal per-cell skeleton.

---

# Technical Approach

### Bug Investigation (T-001)

Trace the per-cell Cargo.toml through the full pipeline:

```
UI (cell settings panel) → model.rs mutation → storage write
                                                    ↓
compile_cell server fn → scaffold_micro_crate(cargo_toml, shared_cargo_toml)
                                                    ↓
                              cache.rs: compute_hash(source, cargo_toml?, shared_cargo_toml?)
                                                    ↓
                              scaffold.rs: generate_cargo_toml() → merge_dependencies()
```

Key checkpoints:
1. Is `cargo_toml` passed to `compile_cell()`?
2. Is `cargo_toml` included in `compute_hash()`?
3. Is the merged output correct in the workspace directory?

### Shared Deps Migration (T-003)

For each public notebook:
1. Collect all unique deps across cells
2. Move them to `shared_cargo_toml`
3. Strip per-cell `cargo_toml` to minimal skeleton
4. Verify notebook still compiles

### Default Template (T-004)

Update new notebook creation to pre-populate:
```toml
# shared_cargo_toml
[dependencies]
ironpad-cell = "0.1"
```

And per-cell default to just:
```toml
[package]
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]
```

---

# Assumptions

- The `scaffold_micro_crate` merge logic is correct — the bug is likely upstream (cache key or data threading).
- Public notebooks can share most dependencies without conflicts.
- Users expect per-cell Cargo.toml edits to work the same as editing a real Cargo.toml.

---

# Constraints

- Changing the cache key formula invalidates all existing caches. This is acceptable.
- Some notebooks may need per-cell deps if cells use conflicting versions (unlikely but possible).
- The default shared Cargo.toml shouldn't include too many deps — just ironpad-cell and perhaps serde.

---

# References to Code

| File | Role | Key Lines/Functions |
|---|---|---|
| `crates/ironpad-app/src/compiler/scaffold.rs` | Cargo.toml generation + merge | scaffold_micro_crate (27-63), merge_dependencies (158-193), generate_cargo_toml |
| `crates/ironpad-app/src/compiler/cache.rs` | Cache hash computation | compute_hash — check if cell cargo_toml is included |
| `crates/ironpad-app/src/server_fns.rs` | compile_cell server function | Check what params are passed to scaffold |
| `crates/ironpad-app/src/model.rs` | Notebook model mutations | Cell cargo_toml storage |
| `crates/ironpad-common/src/types.rs` | IronpadNotebook, Cell structs | cargo_toml and shared_cargo_toml fields |
| `public/notebooks/*.ironpad` | All 21 demo notebooks | Per-cell cargo_toml and shared_cargo_toml usage |

---

# Non-Goals (MVP)

- Cargo.toml syntax validation or autocompletion in the editor
- Dependency resolution UI (showing resolved versions)
- Automatic dependency detection from cell source code
- Lock file support for reproducible builds
- Per-cell rustflags or build profiles beyond what Cargo.toml supports

---

# History

(Entries appended during implementation go below this line.)

2026-03-15: All 4 tasks completed. Investigation found no bug in per-cell Cargo.toml pipeline. Migrated 12 public notebooks to use null per-cell cargo_toml (deps in shared). New notebooks now default to shared Cargo.toml with ironpad-cell. CI passes (307 tests).

---
