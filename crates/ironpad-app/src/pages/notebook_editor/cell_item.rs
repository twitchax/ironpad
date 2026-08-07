use crate::components::icon::{Chevron, Icon, IconLabel};
use crate::components::icons;
use ironpad_common::{CellManifest, CellType, CompileResponse, Diagnostic, ExecutionResult};
use leptos::prelude::*;

use crate::components::markdown_cell::MarkdownCell;
use crate::components::monaco_editor::{MonacoEditor, MonacoEditorHandle};
use crate::model::NotebookModel;

use super::cell_output::{CellOutputPanel, CompileResultPanel};
use super::pipeline;
use super::state::{persist_notebook, CellStatus, NotebookState};

// ── Cell item ───────────────────────────────────────────────────────────────

/// A single cell card in the notebook editor.
///
/// Features a tab bar (Code / Cargo.toml) with independent Monaco editors,
/// a header with order badge, editable label, status placeholder, run button,
/// and menu button. The cell body is collapsible.
#[component]
#[allow(clippy::needless_pass_by_value)]
pub(super) fn CellItem(cell: CellManifest) -> impl IntoView {
    let state = expect_context::<NotebookState>();
    let model = expect_context::<NotebookModel>();
    let cell_id = cell.id.clone();
    let cell_id_for_click = cell.id.clone();
    let cell_id_for_delete = cell.id.clone();
    let cell_id_for_delete_cleanup = cell.id.clone();
    let cell_id_for_focus = cell.id.clone();
    #[cfg(feature = "hydrate")]
    let cell_id_for_flush = cell.id.clone();
    let cell_id_for_stale_header = cell.id.clone();
    let cell_id_for_output = cell.id.clone();

    // Live-derived order: after T-001, NotebookContent (and this CellItem) is
    // built once and not rebuilt on reorder, so the `cell.order` captured by
    // value would go stale. Look it up from state.cells (kept live by
    // sync_from_notebook) on every render instead, falling back to the
    // initially-captured order if the cell is momentarily missing (e.g. mid-delete).
    let cell_id_for_order = cell.id.clone();
    let initial_order = cell.order;
    let order_display = Signal::derive(move || {
        state.cells.with(|cells| {
            cells
                .iter()
                .find(|c| c.id == cell_id_for_order)
                .map_or(initial_order, |c| c.order)
        })
    });

    let is_markdown = cell.cell_type == CellType::Markdown;

    // Shared flag, looked up live from the manifest list (like `order_display`)
    // so the toggle below re-renders this cell without a rebuild.
    let cell_id_for_shared = cell.id.clone();
    let initial_shared = cell.shared;
    let is_shared = Signal::derive(move || {
        state.cells.with(|cells| {
            cells
                .iter()
                .find(|c| c.id == cell_id_for_shared)
                .map_or(initial_shared, |c| c.shared)
        })
    });

    let is_active = move || state.active_cell.get().as_deref() == Some(cell_id.as_str());

    let on_click = move |_| {
        state.active_cell.set(Some(cell_id_for_click.clone()));
    };

    // ── Cell status & compile result ────────────────────────────────────

    let cell_status = RwSignal::new(CellStatus::Idle);
    let last_compile: RwSignal<Option<CompileResponse>> = RwSignal::new(None);
    // Live check-on-type (PRD-0045): latest check diagnostics and a
    // generation counter so superseded responses are discarded.
    let last_check: RwSignal<Option<Vec<Diagnostic>>> = RwSignal::new(None);
    let check_generation: RwSignal<u64> = RwSignal::new(0);
    let compile_time_ms: RwSignal<Option<f64>> = RwSignal::new(None);
    let execution_result: RwSignal<Option<ExecutionResult>> = RwSignal::new(None);

    // ── Collapse state ──────────────────────────────────────────────────
    //
    // Two layers: the cell's PERSISTED defaults (`default_*`, set by the
    // header toggles and stored on the cell, so every surface loads the cell
    // in that state) and the LIVE state (`collapsed` / `output_collapsed`,
    // which the transient chevrons also flip without persisting anything).
    // Mode switches don't touch collapse at all — the saved default is the
    // load state, full stop.

    let collapsed = RwSignal::new(cell.collapsed);
    let output_collapsed = RwSignal::new(cell.output_collapsed);
    let default_collapsed = RwSignal::new(cell.collapsed);
    let default_output_collapsed = RwSignal::new(cell.output_collapsed);

    // ── Tab state ───────────────────────────────────────────────────────

    let selected_tab = RwSignal::new("code".to_string());

    // ── Editor handles (for compile flow) ───────────────────────────────

    let source_handle: RwSignal<Option<MonacoEditorHandle>> = RwSignal::new(None);
    let cargo_toml_handle: RwSignal<Option<MonacoEditorHandle>> = RwSignal::new(None);

    // ── Reactive source / cargo_toml state ──────────────────────────────

    let initial_content = state.notebook.with_untracked(|nb_opt| {
        nb_opt.as_ref().and_then(|nb| {
            nb.cells
                .iter()
                .find(|c| c.id == cell.id)
                .map(|c| (c.source.clone(), c.cargo_toml.clone().unwrap_or_default()))
        })
    });
    let source = RwSignal::new(
        initial_content
            .as_ref()
            .map(|c| c.0.clone())
            .unwrap_or_default(),
    );
    let cargo_toml = RwSignal::new(
        initial_content
            .as_ref()
            .map(|c| c.1.clone())
            .unwrap_or_default(),
    );

    // ── Dirty state (unsaved changes indicator) ─────────────────────────

    let source_dirty = RwSignal::new(false);
    let cargo_toml_dirty = RwSignal::new(false);

    // ── Delete action ───────────────────────────────────────────────────

    let cell_id_for_delete_sv = StoredValue::new(cell_id_for_delete);
    let cell_id_for_delete_cleanup_sv = StoredValue::new(cell_id_for_delete_cleanup);

    let delete_cell_fn = move || {
        let cid = cell_id_for_delete_sv.get_value();
        let cid_cleanup = cell_id_for_delete_cleanup_sv.get_value();
        let version = model.cell_version(&cid);
        if model
            .apply(
                ironpad_common::protocol::Mutation::CellDelete {
                    cell_id: cid,
                    version,
                },
                ironpad_common::protocol::ClientId::browser(),
            )
            .is_ok()
        {
            // Clean up execution state (UI concern, not model concern) —
            // the shared scrub, same as the agent CellDelete path.
            state.scrub_deleted_cell(&cid_cleanup);
            persist_notebook(&state);
        }
    };

    // ── Rename action ───────────────────────────────────────────────────

    let label = RwSignal::new(cell.label.clone());
    let cell_id_for_rename = cell.id.clone();

    let on_label_blur = move |_| {
        let current = label.get_untracked();
        let cid = cell_id_for_rename.clone();
        let version = model.cell_version(&cid);
        if model
            .apply(
                ironpad_common::protocol::Mutation::CellUpdate {
                    cell_id: cid,
                    source: None,
                    cargo_toml: None,
                    label: Some(current),
                    shared: None,
                    collapsed: None,
                    output_collapsed: None,
                    version,
                },
                ironpad_common::protocol::ClientId::browser(),
            )
            .is_ok()
        {
            persist_notebook(&state);
        }
    };

    // ── Default-collapse toggles (persisted per cell) ───────────────────
    //
    // Explicit authoring controls, distinct from the transient chevrons:
    // they set the cell's saved load-state, snap the live state to match,
    // and ride CellUpdate so agents and every surface see the same default.
    let cell_id_for_collapse = StoredValue::new(cell.id.clone());
    let persist_collapse_default = move |code: Option<bool>, output: Option<bool>| {
        let cid = cell_id_for_collapse.get_value();
        let version = model.cell_version(&cid);
        if model
            .apply(
                ironpad_common::protocol::Mutation::CellUpdate {
                    cell_id: cid,
                    source: None,
                    cargo_toml: None,
                    label: None,
                    shared: None,
                    collapsed: code,
                    output_collapsed: output,
                    version,
                },
                ironpad_common::protocol::ClientId::browser(),
            )
            .is_ok()
        {
            persist_notebook(&state);
        }
    };
    let on_toggle_default_collapsed = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        let new_val = !default_collapsed.get_untracked();
        default_collapsed.set(new_val);
        collapsed.set(new_val);
        persist_collapse_default(Some(new_val), None);
    };
    let on_toggle_default_output_collapsed = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        let new_val = !default_output_collapsed.get_untracked();
        default_output_collapsed.set(new_val);
        output_collapsed.set(new_val);
        persist_collapse_default(None, Some(new_val));
    };

    // ── Menu state ──────────────────────────────────────────────────────

    let menu_open = RwSignal::new(false);

    // ── Move (reorder) action ───────────────────────────────────────────

    let cell_id_for_move = StoredValue::new(cell.id.clone());

    let reorder_cells_fn = move |new_ids: Vec<String>| {
        if model
            .apply(
                ironpad_common::protocol::Mutation::CellReorder { cell_ids: new_ids },
                ironpad_common::protocol::ClientId::browser(),
            )
            .is_ok()
        {
            persist_notebook(&state);
        }
    };

    let reorder_for_up = reorder_cells_fn;
    let on_move_up = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        menu_open.set(false);
        let cid = cell_id_for_move.get_value();
        let cells = state.cells.get_untracked();
        let Some(my_idx) = cells.iter().position(|c| c.id == cid) else {
            return;
        };
        if my_idx == 0 {
            return;
        }
        let mut ids: Vec<String> = cells.iter().map(|c| c.id.clone()).collect();
        ids.swap(my_idx, my_idx - 1);
        reorder_for_up(ids);
    };

    let reorder_for_down = reorder_cells_fn;
    let on_move_down = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        menu_open.set(false);
        let cid = cell_id_for_move.get_value();
        let cells = state.cells.get_untracked();
        let Some(my_idx) = cells.iter().position(|c| c.id == cid) else {
            return;
        };
        if my_idx + 1 >= cells.len() {
            return;
        }
        let mut ids: Vec<String> = cells.iter().map(|c| c.id.clone()).collect();
        ids.swap(my_idx, my_idx + 1);
        reorder_for_down(ids);
    };

    // ── Duplicate action ────────────────────────────────────────────────

    let cell_id_for_dup = StoredValue::new(cell.id.clone());

    let on_duplicate = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        menu_open.set(false);

        let cid = cell_id_for_dup.get_value();

        // Read the original cell from the notebook.
        let original = state.notebook.with_untracked(|nb_opt| {
            nb_opt
                .as_ref()
                .and_then(|nb| nb.cells.iter().find(|c| c.id == cid).cloned())
        });
        let Some(original) = original else { return };

        if let Ok((result, _event)) = model.apply(
            ironpad_common::protocol::Mutation::CellAdd {
                cell: ironpad_common::protocol::NewCell {
                    source: original.source,
                    cell_type: original.cell_type,
                    label: format!("{} (copy)", original.label),
                    cargo_toml: original.cargo_toml,
                    shared: original.shared,
                },
                after_cell_id: Some(cid),
            },
            ironpad_common::protocol::ClientId::browser(),
        ) {
            if let ironpad_common::protocol::MutationResult::CellAdded { cell_id, .. } = result {
                state.pending_focus_cell.set(Some(cell_id));
            }
            persist_notebook(&state);
        }
    };

    // ── Delete with confirmation ────────────────────────────────────────

    let on_delete_confirmed = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        menu_open.set(false);
        #[cfg(feature = "hydrate")]
        {
            let confirmed = web_sys::window()
                .unwrap()
                .confirm_with_message("Delete this cell? This cannot be undone.")
                .unwrap_or(false);
            if confirmed {
                delete_cell_fn();
            }
        }
        #[cfg(not(feature = "hydrate"))]
        {
            delete_cell_fn();
        }
    };

    // ── Move boundary checks ────────────────────────────────────────────

    let cell_id_for_boundary = StoredValue::new(cell.id.clone());
    let is_first = Signal::derive(move || {
        let cid = cell_id_for_boundary.get_value();
        state.cells.get().first().is_none_or(|c| c.id == cid)
    });

    let is_last = Signal::derive(move || {
        let cid = cell_id_for_boundary.get_value();
        state.cells.get().last().is_none_or(|c| c.id == cid)
    });

    // ── Run cell action (compile flow) ──────────────────────────────────

    // Trigger signal: incrementing this dispatches a compile.
    let run_trigger = RwSignal::new(0u64);
    let cell_id_for_run = StoredValue::new(cell.id.clone());

    // ── Session execution events (PRD-0052) ─────────────────────────────
    //
    // Carried into the pipeline's run context; emission is gated on a live
    // session there. `use_context` (not expect_): CellItem must not require
    // session wiring to render.
    let session_for_events = use_context::<crate::session::SessionState>();

    // ── Run-all queue watcher ───────────────────────────────────────────
    //
    // When this cell appears at the front of the run-all queue, trigger
    // its compile flow.  Non-front cells show a "Queued" status badge.

    let cell_id_for_queue = StoredValue::new(cell.id.clone());

    Effect::new(move || {
        let queue = state.run_all_queue.get();
        let cid = cell_id_for_queue.get_value();

        let my_pos = queue.iter().position(|id| id == &cid);

        match my_pos {
            Some(0) => {
                // Markdown and shared cells skip compilation — advance the
                // queue immediately (shared cells compile as part of every
                // OTHER cell's shared.rs, never on their own).
                if is_markdown || is_shared.get_untracked() {
                    crate::components::run_flow::advance_queue(state.run_all_queue, &cid);
                    return;
                }

                // At the front — trigger compile if not already in progress.
                if !matches!(
                    cell_status.get_untracked(),
                    CellStatus::Compiling | CellStatus::Running
                ) {
                    run_trigger.update(|g| *g += 1);
                }
            }
            Some(_) => {
                // Waiting in queue — show queued indicator.
                if !is_markdown
                    && !matches!(
                        cell_status.get_untracked(),
                        CellStatus::Compiling | CellStatus::Running | CellStatus::Queued
                    )
                {
                    cell_status.set(CellStatus::Queued);
                }
            }
            None => {
                // Not in queue — reset from Queued back to Idle.
                if cell_status.get_untracked() == CellStatus::Queued {
                    cell_status.set(CellStatus::Idle);
                }
            }
        }
    });

    // ── Blocked-by watcher ──────────────────────────────────────────────
    //
    // When this cell appears in `cell_blocked_by`, show a Blocked status.
    // When it is removed (e.g. the blocking cell re-runs successfully, or
    // reactive mode is toggled off), revert to Idle.

    let cell_id_for_blocked = StoredValue::new(cell.id.clone());

    Effect::new(move || {
        let blocked = state.cell_blocked_by.get();
        let cid = cell_id_for_blocked.get_value();
        if blocked.contains_key(&cid) {
            cell_status.set(CellStatus::Blocked);
        } else if cell_status.get_untracked() == CellStatus::Blocked {
            cell_status.set(CellStatus::Idle);
        }
    });

    // ── Shared-cell toggle (PRD-0044) ───────────────────────────────────

    let cell_id_for_shared_toggle = StoredValue::new(cell.id.clone());
    let on_toggle_shared = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        menu_open.set(false);
        let cid = cell_id_for_shared_toggle.get_value();
        let version = model.cell_version(&cid);
        let next = !is_shared.get_untracked();
        if model
            .apply(
                ironpad_common::protocol::Mutation::CellUpdate {
                    cell_id: cid,
                    source: None,
                    cargo_toml: None,
                    label: None,
                    shared: Some(next),
                    collapsed: None,
                    output_collapsed: None,
                    version,
                },
                ironpad_common::protocol::ClientId::browser(),
            )
            .is_ok()
        {
            persist_notebook(&state);
        }
    };

    // ── Run All Below trigger ───────────────────────────────────────────

    let cell_id_for_run_all = StoredValue::new(cell.id.clone());
    let on_run_all_below = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        menu_open.set(false);
        let cid = cell_id_for_run_all.get_value();
        let cells = state.cells.get_untracked();
        let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
        let queue: Vec<String> = cells[my_idx..]
            .iter()
            .filter(|c| c.is_runnable())
            .map(|c| c.id.clone())
            .collect();
        if !queue.is_empty() {
            state.run_all_queue.set(queue);
        }
    };

    // The compile/execute pipeline lives in `pipeline.rs` (PRD-0055 T-001):
    // this component owns the trigger and the rendering, nothing else.
    let run_ctx = pipeline::CellRunCtx {
        state,
        model,
        cell_id: cell_id_for_run,
        is_markdown,
        is_shared,
        cell_status,
        last_compile,
        compile_time_ms,
        execution_result,
        source,
        cargo_toml,
        source_handle,
        session: session_for_events,
    };
    pipeline::wire_run_effect(&run_ctx, run_trigger);

    // ── Keyboard shortcut registration ─────────────────────────────────
    //
    // Once the source Monaco editor handle is available, register:
    // - Ctrl+Enter (Cmd+Enter on Mac): run the current cell
    // - Shift+Enter: run the current cell and advance focus to the next cell

    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::prelude::*;

        let cell_id_for_keys = StoredValue::new(cell.id.clone());

        Effect::new(move || {
            // `try_get` (not `get`): this effect can be queued and then run
            // during the cell's disposal teardown, when the signal's arena slot
            // is already reclaimed — bail instead of panicking on it.
            let Some(handle) = source_handle.try_get().flatten() else {
                return;
            };

            // Register this cell's editor handle for cross-cell focus.
            let cid = cell_id_for_keys.get_value();
            state.editor_handles.update(|m| {
                m.insert(cid, handle);
            });

            // Ctrl+Enter / Cmd+Enter → run cell.
            // Monaco KeyMod.CtrlCmd (2048) | KeyCode.Enter (3) = 2051
            let run_closure = Closure::<dyn Fn()>::new(move || {
                run_trigger.update(|g| *g += 1);
            });
            let run_cb: js_sys::Function = run_closure
                .as_ref()
                .unchecked_ref::<js_sys::Function>()
                .clone();
            StoredValue::new_local(run_closure);
            handle.add_action("ironpad.runCell", &[2051], &run_cb);

            // Shift+Enter → run cell and advance focus to next cell.
            // Monaco KeyMod.Shift (1024) | KeyCode.Enter (3) = 1027
            let advance_closure = Closure::<dyn Fn()>::new(move || {
                run_trigger.update(|g| *g += 1);

                // Find and focus the next cell's editor.
                let cid = cell_id_for_keys.get_value();
                let cells = state.cells.get_untracked();
                // If this cell was removed, don't advance to the wrong index — bail.
                let Some(my_idx) = cells.iter().position(|c| c.id == cid) else {
                    return;
                };
                let handles = state.editor_handles.get_untracked();

                // Look for the next cell (any type) that has a registered editor handle.
                if let Some(next_handle) =
                    cells[my_idx + 1..].iter().find_map(|c| handles.get(&c.id))
                {
                    next_handle.focus();
                }
            });
            let advance_cb: js_sys::Function = advance_closure
                .as_ref()
                .unchecked_ref::<js_sys::Function>()
                .clone();
            StoredValue::new_local(advance_closure);
            handle.add_action("ironpad.runCellAndAdvance", &[1027], &advance_cb);
        });

        // Drop this cell's editor handle when the cell is disposed so handles
        // for deleted cells don't accumulate in the shared map.
        let cid_handle_cleanup = cell_id_for_keys.get_value();
        on_cleanup(move || {
            state.editor_handles.try_update(|m| {
                m.remove(&cid_handle_cleanup);
            });
        });
    }

    // ── Pull remote (agent) source edits into Monaco ────────────────────
    //
    // When an agent edits this cell's content, the model bumps
    // external_content_generation. Refresh Monaco from the model so the change
    // is visible without a reload — but never overwrite the host's own unsaved
    // edits (source_dirty), and skip if Monaco already matches.

    #[cfg(feature = "hydrate")]
    {
        let cell_id_for_external = StoredValue::new(cell.id.clone());

        Effect::new(move || {
            // React to remote content edits.
            state.external_content_generation.get();

            let Some(handle) = source_handle.get_untracked() else {
                return;
            };
            // Don't clobber the host's in-progress local edits.
            if source_dirty.get_untracked() {
                return;
            }

            let cid = cell_id_for_external.get_value();
            let latest = state.notebook.with_untracked(|nb_opt| {
                nb_opt
                    .as_ref()
                    .and_then(|nb| nb.cells.iter().find(|c| c.id == cid))
                    .map(|c| c.source.clone())
            });
            if let Some(latest) = latest {
                if handle.get_value() != latest {
                    handle.set_value(&latest);
                    source.set(latest);
                }
            }
        });
    }

    // ── Autocomplete context ────────────────────────────────────────────
    //
    // Push cell variable context to the Monaco completion provider whenever
    // the editor handle appears or cell outputs change (types may update).

    #[cfg(feature = "hydrate")]
    {
        let cell_id_for_ctx = StoredValue::new(cell.id.clone());

        Effect::new(move || {
            // `try_get` (not `get`): this effect can be queued and then run
            // during the cell's disposal teardown, when the signal's arena slot
            // is already reclaimed — bail instead of panicking on it.
            let Some(handle) = source_handle.try_get().flatten() else {
                return;
            };

            // Re-run when cell_outputs change (new type_tags after execution).
            let outputs = state.cell_outputs.get();
            let cells = state.cells.get_untracked();
            let cid = cell_id_for_ctx.get_value();
            let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);

            let variables = js_sys::Array::new();

            // Piping slots are POSITIONAL over every preceding cell (markdown
            // included, as an empty slot) — the scaffold binds `cellN` by
            // index into that full vector. Enumerate all preceding cells and
            // only SUGGEST the Code ones, so `cellN` matches the actual
            // binding: numbering by code-cell-only index offered the wrong
            // name (and type) in any notebook opening with a markdown cell.
            for (i, c) in cells[..my_idx].iter().enumerate() {
                if c.cell_type != CellType::Code {
                    continue;
                }
                let type_str = outputs
                    .get(&c.id)
                    .and_then(|d| d.type_tag.as_deref())
                    .unwrap_or("unknown");

                let var = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &var,
                    &wasm_bindgen::JsValue::from_str("name"),
                    &wasm_bindgen::JsValue::from_str(&format!("cell{i}")),
                );
                let _ = js_sys::Reflect::set(
                    &var,
                    &wasm_bindgen::JsValue::from_str("type"),
                    &wasm_bindgen::JsValue::from_str(type_str),
                );
                let _ = js_sys::Reflect::set(
                    &var,
                    &wasm_bindgen::JsValue::from_str("doc"),
                    &wasm_bindgen::JsValue::from_str(&format!(
                        "Output of cell {} ({})",
                        c.label, type_str
                    )),
                );
                variables.push(&var);
            }

            // Add `last` alias pointing to the most recent typed Code cell.
            let has_prev_code = cells[..my_idx]
                .iter()
                .any(|c| c.cell_type == CellType::Code);
            if has_prev_code {
                // Walk backwards to find the most recent Code cell with a type_tag.
                let last_type = cells[..my_idx]
                    .iter()
                    .rev()
                    .filter(|c| c.cell_type == CellType::Code)
                    .find_map(|c| {
                        outputs
                            .get(&c.id)
                            .and_then(|d| d.type_tag.as_deref())
                            .map(ToString::to_string)
                    })
                    .unwrap_or_else(|| "unknown".to_string());

                let var = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &var,
                    &wasm_bindgen::JsValue::from_str("name"),
                    &wasm_bindgen::JsValue::from_str("last"),
                );
                let _ = js_sys::Reflect::set(
                    &var,
                    &wasm_bindgen::JsValue::from_str("type"),
                    &wasm_bindgen::JsValue::from_str(&last_type),
                );
                let _ = js_sys::Reflect::set(
                    &var,
                    &wasm_bindgen::JsValue::from_str("doc"),
                    &wasm_bindgen::JsValue::from_str("Output of the most recent cell"),
                );
                variables.push(&var);
            }

            let context = js_sys::Object::new();
            let _ = js_sys::Reflect::set(
                &context,
                &wasm_bindgen::JsValue::from_str("variables"),
                &variables,
            );
            handle.set_cell_context(&wasm_bindgen::JsValue::from(context));
        });
    }

    // ── Inline error markers ────────────────────────────────────────────
    //
    // When `last_compile` changes, convert diagnostics with spans into
    // Monaco model markers so errors/warnings appear inline in the editor.

    #[cfg(feature = "hydrate")]
    {
        Effect::new(move || {
            // `try_get` (not `get`): this marker effect can be queued by a
            // compile result and then run during the cell's disposal teardown,
            // when the signal's arena slot is already reclaimed. Bail on the
            // disposed slot rather than panic (traits.rs "already been disposed").
            let Some(compile) = last_compile.try_get() else {
                return;
            };
            let Some(handle) = source_handle.get_untracked() else {
                return;
            };

            let Some(response) = compile else {
                // Compile was cleared (e.g. new compile started); markers
                // already cleared by the compile-start code above.
                return;
            };

            handle.set_markers(&pipeline::diagnostics_to_markers(&response.diagnostics));
        });

        // Live check-on-type markers (PRD-0045). Compile and check results
        // share the marker surface; whichever finished last wins, which is
        // also the freshest information.
        Effect::new(move || {
            // `try_get`: same disposal-race guard as the compile marker effect
            // above — a check result can queue this effect after the cell is gone.
            let Some(check) = last_check.try_get() else {
                return;
            };
            let Some(handle) = source_handle.get_untracked() else {
                return;
            };
            let Some(diagnostics) = check else {
                return;
            };
            handle.set_markers(&pipeline::diagnostics_to_markers(&diagnostics));
        });
    }

    // ── On-change callbacks for Monaco editors ──────────────────────────
    //
    // Source editor: debounce saves via setTimeout / clearTimeout so that
    // rapid keystrokes are batched into a single server call after 1 s of
    // inactivity.  The debounce plumbing only exists in the `hydrate`
    // (client-side) build; SSR simply updates the reactive signal.

    #[cfg(feature = "hydrate")]
    let on_source_change = {
        use wasm_bindgen::prelude::*;

        let cid_save = cell.id.clone();
        let debounce_handle: RwSignal<i32> = RwSignal::new(0);

        // Build a reusable JS function that reads the *current* source from
        // the signal and persists it via the model.
        let cid_check = cell.id.clone();
        let closure = Closure::<dyn Fn()>::new(move || {
            let val = source.get_untracked();
            let cid = cid_save.clone();
            let version = model.cell_version(&cid);
            if model
                .apply(
                    ironpad_common::protocol::Mutation::CellUpdate {
                        cell_id: cid,
                        source: Some(val),
                        cargo_toml: None,
                        label: None,
                        shared: None,
                        collapsed: None,
                        output_collapsed: None,
                        version,
                    },
                    ironpad_common::protocol::ClientId::browser(),
                )
                .is_ok()
            {
                persist_notebook(&state);
                source_dirty.set(false);

                // Live check-on-type (PRD-0045): the model is current here by
                // construction (the save just landed), so this is the one
                // honest moment to type-check what the user sees.
                pipeline::dispatch_live_check(
                    &state,
                    cid_check.clone(),
                    is_markdown,
                    is_shared,
                    cell_status,
                    source,
                    cargo_toml,
                    last_check,
                    check_generation,
                );
            }
        });
        let save_fn: js_sys::Function =
            closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
        // Hold the closure in the arena (dropped on cell disposal) rather than
        // leaking it with forget().
        StoredValue::new_local(closure);

        // Cancel a pending debounced save when the cell is disposed (e.g.
        // deleted) so it doesn't fire against disposed signals / a gone cell.
        on_cleanup(move || {
            let prev = debounce_handle.get_untracked();
            if prev != 0 {
                if let Some(win) = web_sys::window() {
                    win.clear_timeout_with_handle(prev);
                }
            }
        });

        Callback::new(move |val: String| {
            source.set(val);
            source_dirty.set(true);

            // Clear the previous debounce timer and start a fresh 1 s window.
            let win = web_sys::window().unwrap();
            let prev = debounce_handle.get_untracked();
            if prev != 0 {
                win.clear_timeout_with_handle(prev);
            }
            let handle = win
                .set_timeout_with_callback_and_timeout_and_arguments_0(&save_fn, 1_000)
                .unwrap();
            debounce_handle.set(handle);
        })
    };

    #[cfg(not(feature = "hydrate"))]
    let on_source_change = Callback::new(move |val: String| {
        source.set(val);
    });

    #[cfg(feature = "hydrate")]
    let on_cargo_toml_change = {
        use wasm_bindgen::prelude::*;

        let cid_save = cell.id.clone();
        let debounce_handle: RwSignal<i32> = RwSignal::new(0);

        let closure = Closure::<dyn Fn()>::new(move || {
            let val = cargo_toml.get_untracked();
            let cid = cid_save.clone();
            let version = model.cell_version(&cid);
            if model
                .apply(
                    ironpad_common::protocol::Mutation::CellUpdate {
                        cell_id: cid,
                        source: None,
                        cargo_toml: Some(Some(val)),
                        label: None,
                        shared: None,
                        collapsed: None,
                        output_collapsed: None,
                        version,
                    },
                    ironpad_common::protocol::ClientId::browser(),
                )
                .is_ok()
            {
                persist_notebook(&state);
                cargo_toml_dirty.set(false);
            }
        });
        let save_fn: js_sys::Function =
            closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
        // Hold the closure in the arena (dropped on cell disposal) rather than
        // leaking it with forget().
        StoredValue::new_local(closure);

        // Cancel a pending debounced save on cell disposal.
        on_cleanup(move || {
            let prev = debounce_handle.get_untracked();
            if prev != 0 {
                if let Some(win) = web_sys::window() {
                    win.clear_timeout_with_handle(prev);
                }
            }
        });

        Callback::new(move |val: String| {
            cargo_toml.set(val);
            cargo_toml_dirty.set(true);

            let win = web_sys::window().unwrap();
            let prev = debounce_handle.get_untracked();
            if prev != 0 {
                win.clear_timeout_with_handle(prev);
            }
            let handle = win
                .set_timeout_with_callback_and_timeout_and_arguments_0(&save_fn, 1_000)
                .unwrap();
            debounce_handle.set(handle);
        })
    };

    #[cfg(not(feature = "hydrate"))]
    let on_cargo_toml_change = Callback::new(move |val: String| {
        cargo_toml.set(val);
    });

    // ── Notebook-level save flush ───────────────────────────────────────
    //
    // When the user triggers a notebook save (Ctrl+S or save button),
    // immediately flush this cell's current source and cargo_toml
    // into the notebook signal (the page-level handler persists to IndexedDB).

    #[cfg(feature = "hydrate")]
    {
        let cid_flush = cell_id_for_flush;
        let prev_save_gen = RwSignal::new(state.save_generation.get_untracked());

        Effect::new(move || {
            let gen = state.save_generation.get();
            if gen == prev_save_gen.get_untracked() {
                return;
            }
            prev_save_gen.set(gen);

            let src = source.get_untracked();
            let toml = cargo_toml.get_untracked();
            let cid = cid_flush.clone();
            let version = model.cell_version(&cid);

            // Flush current editor content into the model.
            if model
                .apply(
                    ironpad_common::protocol::Mutation::CellUpdate {
                        cell_id: cid,
                        source: Some(src),
                        cargo_toml: Some(Some(toml)),
                        label: None,
                        shared: None,
                        collapsed: None,
                        output_collapsed: None,
                        version,
                    },
                    ironpad_common::protocol::ClientId::browser(),
                )
                .is_ok()
            {
                source_dirty.set(false);
                cargo_toml_dirty.set(false);
            }
        });
    }

    // ── Clear stale local output when invalidated by an upstream re-run ──
    //
    // When an upstream cell re-runs, it removes this (downstream) cell's cached
    // output from the page-level `cell_outputs` map. The Output/CompileResult
    // panels render from this cell's *local* `execution_result`, which would
    // otherwise keep showing the now-stale result until this cell re-runs. Clear
    // it when our cached output disappears while we're idle — but NOT during this
    // cell's own re-run (status is set to Compiling before it invalidates itself).

    #[cfg(feature = "hydrate")]
    {
        let cid_invalidate = StoredValue::new(cell.id.clone());
        Effect::new(move || {
            let has_output = state
                .cell_outputs
                .with(|m| m.contains_key(&cid_invalidate.get_value()));
            if !has_output
                && execution_result.get_untracked().is_some()
                && !matches!(
                    cell_status.get_untracked(),
                    CellStatus::Compiling | CellStatus::Running | CellStatus::Queued
                )
            {
                execution_result.set(None);
            }
        });
    }

    // ── CSS classes ─────────────────────────────────────────────────────

    // `try_get` keeps the reads safe even if the closure is flushed after the
    // cell is gone.
    let cell_class = move || {
        let mut class = "ironpad-cell-card".to_string();
        if is_markdown {
            class.push_str(" ironpad-cell-card--markdown");
        }
        if is_shared.try_get().unwrap_or(false) {
            class.push_str(" ironpad-cell-card--shared");
        }
        // `is_active` reads state.active_cell, which is page-owned and outlives
        // this cell, so it needs no disposal guard.
        if is_active() {
            class.push_str(" ironpad-cell-card--active");
        }
        class
    };

    let body_class = Signal::derive(move || {
        // `collapsed` is the single source of truth: it loads from the cell's
        // saved default, the header toggle snaps it when the default changes,
        // and the chevron flips it transiently — no per-mode overrides.
        if collapsed.get() {
            "ironpad-cell-body ironpad-cell-body--collapsed"
        } else {
            "ironpad-cell-body"
        }
    });

    // ── Scroll-to & focus when this cell is newly added ─────────────────

    let cell_wrapper_ref: NodeRef<leptos::html::Div> = NodeRef::new();

    Effect::new(move || {
        let pending = state.pending_focus_cell.get();
        if pending.as_deref() != Some(cell_id_for_focus.as_str()) {
            return;
        }

        // Clear the pending focus to avoid re-triggering.
        state.pending_focus_cell.set(None);

        // Scroll the cell card into view.
        #[cfg(feature = "hydrate")]
        if let Some(el) = cell_wrapper_ref.get_untracked() {
            let html_el: &web_sys::Element = &el;
            html_el.scroll_into_view();
        }

        // Focus the source editor after a short delay to allow Monaco to
        // initialise asynchronously via the AMD loader.
        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::prelude::*;

            // One-shot: `once_into_js` frees the Rust closure when the timer
            // fires. It must NOT live in this effect's scope (e.g. via
            // StoredValue): the effect re-runs immediately (it sets
            // `pending_focus_cell` above, which it also reads), disposing this
            // run's scope and dropping the closure before the timeout — JS then
            // throws "closure invoked recursively or after being dropped".
            let handle = source_handle;
            let focus_cb = Closure::once_into_js(move || {
                if let Some(h) = handle.get_untracked() {
                    h.focus();
                }
            });

            let _ = web_sys::window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(
                    focus_cb.unchecked_ref(),
                    300,
                );
        }
    });

    // ── Render ──────────────────────────────────────────────────────────

    view! {
        <div node_ref=cell_wrapper_ref class="ironpad-cell-row">
        <div
            class=cell_class
            on:click=on_click
        >
            <div class="ironpad-cell-card-header">
                <div class="ironpad-cell-header">
                    <button
                        class="ironpad-cell-collapse-btn"
                        on:click=move |ev| {
                            ev.stop_propagation();
                            collapsed.update(|c| *c = !*c);
                        }
                    >
                        <Chevron expanded=Signal::derive(move || !collapsed.get())/>
                    </button>

                    <span class="ironpad-cell-order">
                        {move || format!("[{}]", order_display.get())}
                    </span>

                    <input
                        class="ironpad-cell-label-input"
                        type="text"
                        prop:value=move || label.get()
                        on:input=move |ev| {
                            let val = event_target_value(&ev);
                            label.set(val);
                        }
                        on:blur=on_label_blur
                        on:click=move |ev| {
                            ev.stop_propagation();
                        }
                    />

                    {move || {
                        if is_markdown || is_shared.get() {
                            return view! { <span /> }.into_any();
                        }
                        let is_stale = state.cell_stale.get()
                            .get(&cell_id_for_stale_header)
                            .copied()
                            .unwrap_or(false);
                        if is_stale && state.reactive_mode.get() {
                            view! { <span class="ironpad-stale-indicator ironpad-stale-indicator--pending" title="Pending re-execution (reactive mode)"><Icon icon=icons::PENDING/></span> }.into_any()
                        } else if is_stale {
                            view! { <span class="ironpad-stale-indicator" title="Cell output is stale"><Icon icon=icons::RERUN/></span> }.into_any()
                        } else {
                            view! { <span /> }.into_any()
                        }
                    }}

                    {if is_markdown {
                        view! {
                            <span class="ironpad-tag ironpad-cell-type-badge ironpad-cell-type-badge--markdown">
                                "¶ markdown"
                            </span>
                        }.into_any()
                    } else {
                        view! {
                            {move || if is_shared.get() {
                                view! {
                                    <span class="ironpad-tag ironpad-cell-type-badge ironpad-cell-type-badge--shared">
                                        <IconLabel icon=icons::SHARED label="shared"/>
                                    </span>
                                }.into_any()
                            } else {
                                view! { <span /> }.into_any()
                            }}
                            // `try_get` keeps the reads safe if these closures are
                            // flushed after the cell is gone.
                            <span
                                class=move || {
                                    let suffix = match cell_status.try_get().unwrap_or(CellStatus::Idle) {
                                        CellStatus::Idle => "idle",
                                        CellStatus::Queued => "queued",
                                        CellStatus::Compiling => "compiling",
                                        CellStatus::Running => "running",
                                        CellStatus::Success => "success",
                                        CellStatus::Error => "error",
                                        CellStatus::Blocked => "blocked",
                                    };
                                    let hidden = if is_shared.try_get().unwrap_or(false) {
                                        " ironpad-cell-status--hidden"
                                    } else {
                                        ""
                                    };
                                    format!("ironpad-cell-status ironpad-cell-status--{suffix}{hidden}")
                                }
                            >
                                {move || {
                                    // (icon, text) rather than a formatted String: the
                                    // badge renders an SVG now, and the arms stay a flat
                                    // match instead of seven duplicated view! blocks.
                                    let (icon, text) = match cell_status.try_get().unwrap_or(CellStatus::Idle) {
                                        CellStatus::Idle => (icons::IDLE, "idle".to_string()),
                                        CellStatus::Queued => (icons::QUEUED, "queued".to_string()),
                                        CellStatus::Compiling => (icons::BUSY, "compiling…".to_string()),
                                        CellStatus::Running => (icons::BUSY, "running…".to_string()),
                                        CellStatus::Success => (
                                            icons::SUCCESS,
                                            // Compile time only; runtime is shown in the
                                            // Output panel meta (the "c+r" form read as one number).
                                            match compile_time_ms.try_get().flatten() {
                                                Some(c) => format!("{c:.0} ms"),
                                                None => "done".to_string(),
                                            },
                                        ),
                                        CellStatus::Error => (icons::ERROR, "error".to_string()),
                                        CellStatus::Blocked => (icons::BLOCKED, "blocked".to_string()),
                                    };
                                    view! { <IconLabel icon=icon label=text/> }
                                }}
                            </span>
                        }.into_any()
                    }}

                    // Default-collapse toggles: authoring controls (edit mode
                    // only) that set how the cell LOADS everywhere — distinct
                    // from the transient chevron. Icon-driven; the dimmed
                    // state means "loads collapsed". Shared cells have no
                    // output, markdown cells have neither toggle.
                    {(!is_markdown).then(|| view! {
                        <div class="ironpad-collapse-defaults">
                                <button
                                    class=move || if default_collapsed.get() { "ironpad-collapse-default-btn ironpad-collapse-default-btn--collapsed" } else { "ironpad-collapse-default-btn" }
                                    title=move || if default_collapsed.get() { "Code loads collapsed. Click to load it open." } else { "Code loads open. Click to load it collapsed." }
                                    on:click=on_toggle_default_collapsed
                                >
                                    <Icon icon=icons::CODE_PANEL/>
                                </button>
                                <Show when=move || !is_shared.get()>
                                    <button
                                        class=move || if default_output_collapsed.get() { "ironpad-collapse-default-btn ironpad-collapse-default-btn--collapsed" } else { "ironpad-collapse-default-btn" }
                                        title=move || if default_output_collapsed.get() { "Output loads collapsed. Click to load it open." } else { "Output loads open. Click to load it collapsed." }
                                        on:click=on_toggle_default_output_collapsed
                                    >
                                        <Icon icon=icons::OUTPUT_PANEL/>
                                    </button>
                                </Show>
                        </div>
                    })}
                </div>
            </div>

            {if is_markdown {
                // ── Markdown cell body ──────────────────────────────────
                view! {
                    <div class=body_class>
                        <MarkdownCell
                            source=source.get_untracked()
                            on_change=on_source_change
                            flush_generation=state.save_generation
                        />
                    </div>
                }.into_any()
            } else {
                // ── Code cell body + panels ─────────────────────────────
                view! {
                    <div class=body_class>
                        <div class="ironpad-cell-tabs">
                            <button
                                class=move || if selected_tab.get() == "code" { "ironpad-cell-tab ironpad-cell-tab--selected" } else { "ironpad-cell-tab" }
                                on:click=move |_| selected_tab.set("code".to_string())
                            >
                                "Code"<Show when=move || source_dirty.get()><span class="ironpad-tab-dirty" title="Unsaved changes"><Icon icon=icons::IDLE/></span></Show>
                            </button>
                            <button
                                class=move || if selected_tab.get() == "cargo-toml" { "ironpad-cell-tab ironpad-cell-tab--selected" } else { "ironpad-cell-tab" }
                                on:click=move |_| selected_tab.set("cargo-toml".to_string())
                            >
                                "Cargo.toml"<Show when=move || cargo_toml_dirty.get()><span class="ironpad-tab-dirty" title="Unsaved changes"><Icon icon=icons::IDLE/></span></Show>
                            </button>
                        </div>

                        <div
                            class="ironpad-cell-editor-pane"
                            style:display=move || {
                                if selected_tab.get() == "code" { "block" } else { "none" }
                            }
                        >
                            <MonacoEditor
                                initial_value=source.get_untracked()
                                language="rust"
                                on_change=on_source_change
                                handle=source_handle
                            />
                        </div>
                        <div
                            class="ironpad-cell-editor-pane"
                            style:display=move || {
                                if selected_tab.get() == "cargo-toml" { "block" } else { "none" }
                            }
                        >
                            <MonacoEditor
                                initial_value=cargo_toml.get_untracked()
                                language="toml"
                                on_change=on_cargo_toml_change
                                handle=cargo_toml_handle
                            />
                        </div>
                    </div>

                    // ── Compile result panel ────────────────────────────────────
                    <CompileResultPanel
                        cell_status=cell_status
                        last_compile=last_compile
                        compile_time_ms=compile_time_ms
                        execution_result=execution_result
                    />

                    // ── Execution output panel ──────────────────────────────────
                    <CellOutputPanel
                        execution_result=execution_result
                        cell_id=cell_id_for_output.clone()
                        cell_outputs=state.cell_outputs
                        cell_stale=state.cell_stale
                        cells=state.cells
                        run_all_queue=state.run_all_queue
                        collapsed=output_collapsed
                    />
                }.into_any()
            }}
        </div>

        // ── Side action buttons ─────────────────────────────────────────
        <div class="ironpad-cell-side-actions">
            {(!is_markdown).then(|| view! {
                <div class="ironpad-drag-handle" title="Drag to reorder">
                    <Icon icon=icons::DRAG/>
                </div>
            })}
            {move || if is_markdown || is_shared.get() {
                view! { <span /> }.into_any()
            } else {
                view! {
                    <button
                        class=Signal::derive(move || {
                            if matches!(cell_status.get(), CellStatus::Compiling | CellStatus::Running) {
                                "ironpad-side-btn ironpad-side-btn--cancel"
                            } else {
                                "ironpad-side-btn"
                            }
                        })
                        title=Signal::derive(move || {
                            if matches!(cell_status.get(), CellStatus::Compiling | CellStatus::Running) {
                                "Cancel execution"
                            } else {
                                "Run cell"
                            }
                        })
                        on:click=move |ev: leptos::ev::MouseEvent| {
                            ev.stop_propagation();
                            match cell_status.get_untracked() {
                                // During compile the in-flight work is the
                                // (unabortable) compile server fn — terminate()
                                // would only respawn the worker and evict every
                                // other loaded cell for nothing. Mark Idle; the
                                // pipeline discards the result when it resolves.
                                CellStatus::Compiling => cell_status.set(CellStatus::Idle),
                                // During execution the worker holds the run, so
                                // terminate() aborts it (AbortError path).
                                CellStatus::Running => {
                                    #[cfg(feature = "hydrate")]
                                    crate::components::executor::terminate_executor();
                                }
                                _ => run_trigger.update(|g| *g += 1),
                            }
                        }
                    >
                        {move || {
                            if matches!(cell_status.get(), CellStatus::Compiling | CellStatus::Running) {
                                view! { <Icon icon=icons::STOP/> }
                            } else {
                                view! { <Icon icon=icons::RUN/> }
                            }
                        }}
                    </button>
                    <button
                        class="ironpad-side-btn"
                        title="Cell settings (Cargo.toml)"
                        on:click=move |ev: leptos::ev::MouseEvent| {
                            ev.stop_propagation();
                            selected_tab.update(|tab| {
                                *tab = if tab == "cargo-toml" {
                                    "code".to_string()
                                } else {
                                    "cargo-toml".to_string()
                                };
                            });
                        }
                    >
                        <Icon icon=icons::SETTINGS/>
                    </button>
                }.into_any()
            }}

            <div class="ironpad-cell-menu-wrapper">
                <button
                    class="ironpad-side-btn"
                    title="Cell menu"
                    on:click=move |ev: leptos::ev::MouseEvent| {
                        ev.stop_propagation();
                        menu_open.update(|v| *v = !*v);
                    }
                >
                    <Icon icon=icons::MORE/>
                </button>

                {move || {
                    if !menu_open.get() {
                        return view! { <div /> }.into_any();
                    }

                    let first = is_first.get();
                    let last = is_last.get();

                    view! {
                        <div
                            class="ironpad-cell-menu-backdrop"
                            on:click=move |_| menu_open.set(false)
                        />
                        <div class="ironpad-cell-menu">
                            <button
                                class="ironpad-cell-menu-item"
                                disabled=first
                                on:click=on_move_up
                            >
                                <IconLabel icon=icons::SORT_UP label="Move Up"/>
                            </button>
                            <button
                                class="ironpad-cell-menu-item"
                                disabled=last
                                on:click=on_move_down
                            >
                                <IconLabel icon=icons::SORT_DOWN label="Move Down"/>
                            </button>
                            <button
                                class="ironpad-cell-menu-item"
                                on:click=on_duplicate
                            >
                                <IconLabel icon=icons::COPY label="Duplicate"/>
                            </button>
                            {if is_markdown {
                                view! { <span /> }.into_any()
                            } else {
                                view! {
                                    {move || if is_shared.get() {
                                        view! { <span /> }.into_any()
                                    } else {
                                        view! {
                                            <button
                                                class="ironpad-cell-menu-item"
                                                on:click=on_run_all_below
                                            >
                                                <IconLabel icon=icons::RUN_ALL label="Run All Below"/>
                                            </button>
                                        }.into_any()
                                    }}
                                    <button
                                        class="ironpad-cell-menu-item"
                                        on:click=on_toggle_shared
                                    >
                                        {move || if is_shared.get() {
                                            view! { <IconLabel icon=icons::SHARED label="Unmark Shared"/> }
                                        } else {
                                            view! { <IconLabel icon=icons::SHARED label="Mark as Shared"/> }
                                        }}
                                    </button>
                                }.into_any()
                            }}
                            <div class="ironpad-cell-menu-divider" />
                            <button
                                class="ironpad-cell-menu-item ironpad-cell-menu-item--danger"
                                on:click=on_delete_confirmed
                            >
                                <IconLabel icon=icons::DELETE label="Delete"/>
                            </button>
                        </div>
                    }.into_any()
                }}
            </div>
        </div>
        </div>
    }
}
