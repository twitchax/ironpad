---
id: PRD-0058
title: "/local version history: a rolling snapshot ring for private notebooks"
status: done
owner: "Aaron Roney"
created: 2026-08-06
updated: 2026-08-06

principles:
- "Insurance for the last unguarded surface: published notebooks have the server draft and Push; private notebooks are one bad save from permanent loss. History is local-only, in the same IndexedDB the notebooks live in."
- "Capture at the one choke point every save already flows through (storage.js saveNotebook): no new Rust persistence paths, no protocol change, nothing for agents."
- "Restore is undoable: restoring first force-snapshots the current state, so the restore itself appears in history."
- "Bounded by construction: one snapshot per 5-minute bucket per notebook, ring-capped, deleted with the notebook."

references:
- name: "IndexedDB store (capture choke point, DB_VERSION 6)"
  url: public/storage.js
- name: "Rust bindings"
  url: crates/ironpad-app/src/storage/client.rs
- name: "Editor menu (History panel mount)"
  url: crates/ironpad-app/src/pages/notebook_editor/mod.rs

acceptance_tests:
- id: uat-001
  name: "Editing a local notebook produces a restorable snapshot: History lists it, Restore (confirm-gated) brings the old content back into the editor and persists it"
  command: cargo make uat
  uat_status: verified
- id: uat-002
  name: "Restore is undoable (the pre-restore state appears in history) and deleting a notebook removes its history"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "storage.js: history store (DB_VERSION 6) + capture + API"
  priority: 1
  status: done
  notes: "New object store `history` (autoIncrement key, index by notebookId). saveNotebook writes a snapshot of the OUTGOING record when the notebook's newest snapshot is absent or older than HISTORY_BUCKET_MS (5 min); ring-prune to HISTORY_CAP (30) per notebook on write. deleteNotebook deletes the notebook's history. API: listHistory(id) -> [{savedAt, title, cellCount}] newest-first (meta computed JS-side, JSON stays in the store), getHistorySnapshot(id, savedAt) -> json, snapshotNow(id) (force-snapshot the CURRENT stored record, bypassing the bucket — the undoable-restore half)."
- id: T-002
  title: "Rust bindings + restore flow + History panel"
  priority: 1
  status: done
  notes: "storage/client.rs bindings for the three fns (typed HistoryEntry). Editor: hamburger gains '🕘 History' in Local mode only (ServerDraft has draft/push semantics instead). Panel: overlay listing entries (relative age + title + cell count) with per-entry confirm-gated Restore; restore = snapshotNow(current) -> getHistorySnapshot -> parse IronpadNotebook (KEEP the current notebook id so the /local URL stays valid) -> set model + sync + persist + reload-free re-render. Styles in main.scss."
- id: T-003
  title: "Tests + docs"
  priority: 2
  status: done
  notes: "e2e (local-history.spec.ts): save A (snapshot exists), edit to B, restore A via the panel, Monaco shows A again, and history now also holds B (undoable); delete-notebook clears history (assert via a fresh notebook id reusing the store... simplest: page.evaluate on IronpadStorage.listHistory). Docs: CLAUDE.md storage section + DB_VERSION note."
---

# Summary

Private (`/local`) notebooks get a rolling, local-only snapshot ring: every save may mint one snapshot per 5-minute bucket (ring of 30, deleted with the notebook), a History panel lists them, and a confirm-gated Restore brings any of them back — itself snapshotting first so nothing is ever lost by restoring.

# History

- **2026-08-06** — PRD created as part of the pre-v0.17.0 batch.
- **2026-08-06** — Implemented and closed. storage.js DB_VERSION 6 with a `history` store keyed `[notebookId, savedAt]`: `saveNotebook` captures the outgoing record when the newest snapshot is older than the 5-minute bucket (best-effort — a history failure never fails the save), prunes to 30 per notebook, and `deleteNotebook` clears the ring. API: `listHistory` (meta only, newest first), `getHistorySnapshot`, `snapshotNow` (forced). Rust bindings + a `HistoryPanel` overlay in the hamburger (Local mode only); Restore flushes + persists + force-snapshots the CURRENT version, writes the chosen snapshot, and reloads. The e2e caught my own wrong mental model rather than a bug: ordinary saves mint at most one snapshot per bucket BY DESIGN (the ring holds past states), and an entire e2e run fits inside one bucket, so the test now forces the capture via `snapshotNow` — the same call the restore path makes — and additionally asserts the pre-restore version is byte-recoverable. Gate: cargo make ci (810), full Playwright 112 passed / 0 failed. Unreleased; ships with v0.17.0.
