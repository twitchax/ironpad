---
id: PRD-0056
title: "Persisted cell outputs: view-only pages render the author's last run before any compile"
status: done
owner: "Aaron Roney"
created: 2026-08-06
updated: 2026-08-06

depends_on:
- PRD-0055

principles:
- "The author's browser is the sandbox: capture happens at the editorial moments (Share, Push, Download) from the run the author is looking at. No server-side execution, no headless-browser screenshots, ever."
- "Display-only: piping bytes are never persisted, so run-cascade correctness is untouched. Running a cell always replaces its saved output with the live result."
- "Readers must never mistake a snapshot for a live run: every saved output carries a visible badge saying it is serialized from the author's last run AND that Run executes it live (the explicit product requirement)."
- "Saved panels are attacker-controlled on /shared and /mutable: they render through the SAME sanitizing panel renderer as live output, never a separate path."
- "The model stays lean: saved_output is written at serialize time (share/push-durable/download), never into the editor's model or the debounced autosaves."

references:
- name: "Panel taxonomy + sanitizing renderer"
  url: crates/ironpad-app/src/components/output_render.rs
- name: "Sharing workflows (capture sites)"
  url: crates/ironpad-app/src/pages/notebook_editor/sharing.rs
- name: "Durable draft save (Push capture site)"
  url: crates/ironpad-app/src/pages/notebook_editor/state.rs
- name: "View-only renderer (preview mount point)"
  url: crates/ironpad-app/src/components/view_only_notebook.rs

acceptance_tests:
- id: uat-001
  name: "A shared notebook with executed cells shows their outputs and the serialized-output badge in a fresh anonymous context with zero compiles; pressing Run replaces the snapshot with a live result"
  command: cargo make uat
  uat_status: verified
- id: uat-002
  name: "Oversized outputs degrade to a placeholder, and the enriched notebook stays within the existing share/draft size caps"
  command: cargo make uat
  uat_status: verified
- id: uat-003
  name: "Push publishes the outputs captured at push time (readers of /mutable see them); debounced autosaves stay lean"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "Schema: IronpadCell.saved_output + capture fn in ironpad-common"
  priority: 1
  status: done
  notes: "saved_output: Option<String> (panels JSON), serde default + skip_serializing_if so every existing notebook parses unchanged and lean notebooks serialize identically. IronpadNotebook::embed_saved_outputs(&mut self, display_texts, per_cell_budget) as a pure method with unit tests: sets panels for cells with outputs, leaves None for unrun cells, replaces over-budget payloads with a placeholder Text panel ('output too large to embed; run the cell to regenerate'). SAVED_OUTPUT_BUDGET_BYTES = 256 KiB. PROTOCOL_VERSION 4 -> 5 (advisory: CellAdded/cells.get now carry the field; agents never set it — no CellPatch/NewCell change)."
- id: T-002
  title: "Capture at the editorial moments"
  priority: 1
  status: done
  notes: "Enrich the SERIALIZED JSON, never the model: (1) sharing.rs flush_serialize_tags embeds before serializing (covers Share Immutable + Share Mutable create); (2) state.rs save_draft_now gains an enrich_outputs flag — true only from persist_notebook_durable (the pre-Push write), so the promote publishes outputs while the 1.5s debounce autosaves stay lean; (3) download_current_notebook embeds (this is how public blog notebooks get committed outputs). Push's own flush_serialize_tags call discards the json (tags only) — the durable save is its capture."
- id: T-003
  title: "Viewer: snapshot rendering + the badge"
  priority: 1
  status: done
  notes: "render_display_panel gains PanelMode { Live, Snapshot }: identical for Text/Html/Svg/Markdown/Table/Interactive/BlobImage/Animation (Animation embeds its frames and replays with no WASM); Snapshot renders Simulation as its first frame drawn once (no tick loop, existing draw helper) and LiveView as its initial content through the same sanitizers (no tick loop). ViewOnlyCodeCell renders the saved panels + a '.view-only-saved-badge' ('Saved output from the author's last run — press ▶ Run to execute live in your browser') whenever saved_output is Some and no live result/error exists yet; the first live result (manual Run or autorun) replaces it. Editor ignores saved_output entirely."
- id: T-004
  title: "Tests, docs, PRD close"
  priority: 2
  status: done
  notes: "Unit: embed_saved_outputs (set/skip/budget), schema round-trip, PROTOCOL_VERSION test bump. e2e (persisted-outputs.spec.ts): run a cell, Share Immutable, open /shared in a fresh context, assert output text + badge visible with no Run click, then Run and assert the badge clears and the live result renders; a mutable push variant for uat-003. Docs: CLAUDE.md storage/sharing sections + DEVELOPMENT.md. No deploy in this PRD."
---

# Summary

View-only pages (public, shared, mutable readers) render each cell's **saved output** — the panels JSON captured from the author's last run at Share/Push/Download time — as the initial output state, clearly badged as serialized with Run as the live upgrade. A first-time reader sees a finished document instead of a wall of unrun cells; nothing about execution, piping, or caching changes.

# Problem

Every first-time reader of a shared or published notebook currently sees empty output panels until cells compile and run (shared pages never autorun at all). The showcase notebooks are exactly the ones that suffer: plots, tables, and animations exist only after a compile round trip the reader has not asked for yet.

# Goals

1. Zero-compile first paint of outputs on all view-only surfaces.
2. Capture with no new execution surface: the author's session is the source of truth.
3. Unmistakable snapshot/live distinction, with Run always one click away.
4. Animated content degrades sensibly: embedded-frame animations replay as-is; simulations and live views show their first frame/initial content statically.

# Technical Approach

See tasks. The design leans on three existing facts: outputs are already serialized per cell as panels JSON (`cell_display_texts`, what Export HTML consumes); `Animation` panels embed their frames and need no WASM to replay; and the sharing workflows all funnel through the serialize seams PRD-0055 just consolidated, so capture is a pure enrichment of the outgoing JSON.

# Assumptions

- 256 KiB per cell of saved output, inside the existing 4 MiB notebook cap, is enough for the real notebooks; oversized outputs degrade to a placeholder rather than failing the share.
- The badge is sufficient honesty for a stale-at-share output; shares are not blocked on freshness (the author sees what they are sharing).

# Constraints

- No piping-byte persistence; `cell_outputs` (the typed bytes) remain session-only.
- Saved panels render only through the sanitizing renderer (`sanitize_html`/`sanitize_svg` paths).
- Wire-compatible: serde-defaulted field; PROTOCOL_VERSION bump is advisory per its policy.

# References to Code

See frontmatter.

# Non-Goals (MVP)

- Server-side or headless-browser output generation of any kind (considered and dropped: the executor is browser-coupled — WebGPU, rayon, JSPI — and running reader-triggerable user code server-side is a new attack surface for marginal gain).
- Canvas screenshots of mid-interaction sim/liveview state (the first-frame/initial-content static render covers the value; DOM capture was the flakiest part of the alternative design).
- Editor-side rendering of saved outputs, agent-settable saved outputs, or committing outputs to the existing public notebooks (follow-up content work once the mechanism ships).

# History

- **2026-08-06** — PRD created; design approved (author-side capture, panel-verbatim persistence with static Simulation/LiveView preview arms, explicit serialized-output badge with a live-run affordance).
- **2026-08-06** — Implemented and closed in one pass. T-001: `saved_output` field (lean when absent) + `embed_saved_outputs` in ironpad-common with `SAVED_OUTPUT_BUDGET_BYTES`/placeholder; PROTOCOL_VERSION 5. T-002: capture at Share Immutable/Mutable (`flush_serialize_tags`), the pre-Push durable save (`save_draft_now(_, enrich_outputs)`), and Download. T-003: `PanelMode::Snapshot` (static Simulation first frame via a one-frame `AnimationCanvas`; LiveView captured content through `render_live_content`'s sanitizers), the `.view-only-saved-badge` with the explicit serialized-plus-Run copy, dashed-border saved block; saved widgets are inert (the badge's Run affordance is the interaction story). The push-path e2e caught a real bug on its first run: enrichment used to ASSIGN `None` for cells with no session capture, stripping the published snapshot on the first Push from the /mutable editor (whose session never ran the cells) — `embed_saved_outputs` now preserves prior snapshots when there is no fresh capture, with a unit regression. Gate: cargo make ci (808), full Playwright 110 passed / 0 failed (both new specs: cold anonymous reader sees output + badge with zero compiles, Run replaces; publish AND push carry outputs). All UATs verified. Unreleased; ships with the next release.
