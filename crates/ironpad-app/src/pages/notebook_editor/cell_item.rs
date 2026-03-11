use ironpad_common::{
    CellManifest, CellType, CompileRequest, CompileResponse, Diagnostic, ExecutionResult,
    IronpadCell, Severity,
};
use leptos::prelude::*;
use thaw::{Card, CardHeader, Tab, TabList, Tag, TagSize};

use crate::components::markdown_cell::MarkdownCell;
use crate::components::monaco_editor::{MonacoEditor, MonacoEditorHandle};
use crate::server_fns::compile_cell;

use super::cell_output::{CellOutputPanel, CompileResultPanel};
use super::state::{
    persist_notebook, sync_cells_from_notebook, CellOutputData, CellStatus, NotebookState,
};

// ── Cell item ───────────────────────────────────────────────────────────────

/// A single cell card in the notebook editor.
///
/// Features a tab bar (Code / Cargo.toml) with independent Monaco editors,
/// a header with order badge, editable label, status placeholder, run button,
/// and menu button. The cell body is collapsible.
#[component]
pub(super) fn CellItem(cell: CellManifest) -> impl IntoView {
    let state = expect_context::<NotebookState>();
    let cell_id = cell.id.clone();
    let cell_id_for_click = cell.id.clone();
    let cell_id_for_delete = cell.id.clone();
    let cell_id_for_delete_cleanup = cell.id.clone();
    let cell_id_for_focus = cell.id.clone();
    let cell_id_for_flush = cell.id.clone();
    let cell_id_for_stale_src = cell.id.clone();
    let cell_id_for_stale_toml = cell.id.clone();
    let cell_id_for_stale_header = cell.id.clone();
    let cell_id_for_output = cell.id.clone();
    let cell_id_for_markdown = StoredValue::new(cell.id.clone());

    let is_markdown = cell.cell_type == CellType::Markdown;

    let is_active = move || state.active_cell.get().as_deref() == Some(cell_id.as_str());

    let on_click = move |_| {
        state.active_cell.set(Some(cell_id_for_click.clone()));
    };

    // ── Cell status & compile result ────────────────────────────────────

    let cell_status = RwSignal::new(CellStatus::Idle);
    let last_compile: RwSignal<Option<CompileResponse>> = RwSignal::new(None);
    let compile_time_ms: RwSignal<Option<f64>> = RwSignal::new(None);
    let execution_result: RwSignal<Option<ExecutionResult>> = RwSignal::new(None);

    // ── Collapse state ──────────────────────────────────────────────────

    let collapsed = RwSignal::new(false);
    let saved_collapsed = RwSignal::new(false);

    // Save/restore collapse state when toggling view mode.
    // Only collapse Code cells; Markdown cells keep their body visible
    // so the rendered preview remains on screen.
    Effect::new(move || {
        if state.is_view_mode.get() {
            saved_collapsed.set(collapsed.get_untracked());
            if !is_markdown {
                collapsed.set(true);
            }
        } else {
            collapsed.set(saved_collapsed.get_untracked());
        }
    });

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
        state.notebook.update(|nb_opt| {
            if let Some(nb) = nb_opt {
                nb.cells.retain(|c| c.id != cid);
                for (i, cell) in nb.cells.iter_mut().enumerate() {
                    cell.order = i as u32;
                }
            }
        });
        state.cell_outputs.update(|map| {
            map.remove(&cid_cleanup);
        });
        state.cell_display_texts.update(|map| {
            map.remove(&cid_cleanup);
        });
        sync_cells_from_notebook(&state);
        persist_notebook(&state);
    };

    // ── Rename action ───────────────────────────────────────────────────

    let label = RwSignal::new(cell.label.clone());
    let cell_id_for_rename = cell.id.clone();

    let on_label_blur = move |_| {
        let current = label.get_untracked();
        let cid = cell_id_for_rename.clone();
        state.notebook.update(|nb_opt| {
            if let Some(nb) = nb_opt {
                if let Some(cell) = nb.cells.iter_mut().find(|c| c.id == cid) {
                    cell.label = current;
                }
            }
        });
        sync_cells_from_notebook(&state);
        persist_notebook(&state);
    };

    // ── Menu state ──────────────────────────────────────────────────────

    let menu_open = RwSignal::new(false);

    // ── Move (reorder) action ───────────────────────────────────────────

    let cell_id_for_move = StoredValue::new(cell.id.clone());

    let reorder_cells_fn = move |new_ids: Vec<String>| {
        state.notebook.update(|nb_opt| {
            let Some(nb) = nb_opt else { return };
            let mut reordered = Vec::with_capacity(new_ids.len());
            for id in &new_ids {
                if let Some(pos) = nb.cells.iter().position(|c| &c.id == id) {
                    reordered.push(nb.cells.remove(pos));
                }
            }
            // Append any cells not in new_ids (shouldn't happen, but safe).
            reordered.append(&mut nb.cells);
            for (i, cell) in reordered.iter_mut().enumerate() {
                cell.order = i as u32;
            }
            nb.cells = reordered;
        });
        sync_cells_from_notebook(&state);
        persist_notebook(&state);
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
        let new_id = {
            #[cfg(feature = "hydrate")]
            {
                uuid::Uuid::new_v4().to_string()
            }
            #[cfg(not(feature = "hydrate"))]
            {
                format!("{cid}_dup")
            }
        };

        state.notebook.update(|nb_opt| {
            let Some(nb) = nb_opt else { return };
            let Some(idx) = nb.cells.iter().position(|c| c.id == cid) else {
                return;
            };
            let original = nb.cells[idx].clone();
            let new_cell = IronpadCell {
                id: new_id.clone(),
                order: 0,
                label: format!("{} (copy)", original.label),
                cell_type: original.cell_type,
                source: original.source,
                cargo_toml: original.cargo_toml,
            };
            nb.cells.insert(idx + 1, new_cell);
            for (i, cell) in nb.cells.iter_mut().enumerate() {
                cell.order = i as u32;
            }
        });

        state.pending_focus_cell.set(Some(new_id));
        sync_cells_from_notebook(&state);
        persist_notebook(&state);
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
        state
            .cells
            .get()
            .first()
            .map(|c| c.id == cid)
            .unwrap_or(true)
    });

    let is_last = Signal::derive(move || {
        let cid = cell_id_for_boundary.get_value();
        state
            .cells
            .get()
            .last()
            .map(|c| c.id == cid)
            .unwrap_or(true)
    });

    // ── Run cell action (compile flow) ──────────────────────────────────

    // Trigger signal: incrementing this dispatches a compile.
    let run_trigger = RwSignal::new(0u64);
    let cell_id_for_run = StoredValue::new(cell.id.clone());

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
                // Markdown cells skip compilation — advance the queue immediately.
                if is_markdown {
                    state.run_all_queue.update(|q| {
                        if q.first().map(|id| id == &cid).unwrap_or(false) {
                            q.remove(0);
                        }
                    });
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
            .filter(|c| c.cell_type == CellType::Code)
            .map(|c| c.id.clone())
            .collect();
        if !queue.is_empty() {
            state.run_all_queue.set(queue);
        }
    };

    // The actual compile flow, driven by `run_trigger`.
    Effect::new(move || {
        let gen = run_trigger.get();
        if gen == 0 {
            return;
        }

        // Markdown cells skip compilation entirely.
        if is_markdown {
            return;
        }

        // Avoid double-dispatch while already compiling or running.
        if matches!(
            cell_status.get_untracked(),
            CellStatus::Compiling | CellStatus::Running
        ) {
            return;
        }

        let cid = cell_id_for_run.get_value();
        let current_source = source.get_untracked();
        let current_cargo_toml = cargo_toml.get_untracked();

        // ── Cascading execution ─────────────────────────────────────────
        // If any predecessor Code cells have not been executed yet, auto-queue
        // them (plus this cell) so they run in order first.
        // Markdown cells are skipped — they don't need execution.
        {
            let cells = state.cells.get_untracked();
            let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
            let outputs = state.cell_outputs.get_untracked();
            let unexecuted: Vec<String> = cells[..my_idx]
                .iter()
                .filter(|c| c.cell_type == CellType::Code && !outputs.contains_key(&c.id))
                .map(|c| c.id.clone())
                .collect();

            if !unexecuted.is_empty() {
                let mut queue = unexecuted;
                queue.push(cid.clone());
                state.run_all_queue.set(queue);
                return;
            }
        }

        // Collect previous Code cell outputs for the I/O pipeline.
        // Markdown cells are skipped — they produce no output.
        let (input_bytes, previous_cell_types) = {
            let cells = state.cells.get_untracked();
            let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
            let outputs = state.cell_outputs.get_untracked();

            if my_idx == 0 {
                (vec![], vec![])
            } else {
                let prev_code_cells: Vec<&CellManifest> = cells[..my_idx]
                    .iter()
                    .filter(|c| c.cell_type == CellType::Code)
                    .collect();
                let mut all_outputs: Vec<&[u8]> = Vec::new();
                let mut types: Vec<Option<String>> = Vec::new();

                for c in &prev_code_cells {
                    if let Some(data) = outputs.get(&c.id) {
                        all_outputs.push(&data.bytes);
                        types.push(data.type_tag.clone());
                    } else {
                        all_outputs.push(&[]);
                        types.push(None);
                    }
                }

                // Serialize using CellInputs wire format (length-prefixed):
                // [u32 LE: count][u32 LE: len0][bytes0...][u32 LE: len1][bytes1...]...
                let mut buf = Vec::new();
                buf.extend_from_slice(&(all_outputs.len() as u32).to_le_bytes());
                for output in &all_outputs {
                    buf.extend_from_slice(&(output.len() as u32).to_le_bytes());
                    buf.extend_from_slice(output);
                }

                (buf, types)
            }
        };

        // Invalidate downstream Code cells' cached outputs (this cell and all after it).
        {
            let cells = state.cells.get_untracked();
            let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
            let downstream_ids: Vec<String> = cells[my_idx..]
                .iter()
                .filter(|c| c.cell_type == CellType::Code)
                .map(|c| c.id.clone())
                .collect();
            state.cell_outputs.update(|map| {
                for id in &downstream_ids {
                    map.remove(id);
                }
            });
            state.cell_display_texts.update(|map| {
                for id in &downstream_ids {
                    map.remove(id);
                }
            });
        }

        cell_status.set(CellStatus::Compiling);
        last_compile.set(None);
        compile_time_ms.set(None);

        // Clear inline markers before each compile.
        #[cfg(feature = "hydrate")]
        {
            if let Some(handle) = source_handle.get_untracked() {
                handle.clear_markers();
            }
        }

        leptos::task::spawn_local(async move {
            #[cfg(feature = "hydrate")]
            let start = js_sys::Date::now();

            let cell_id_for_exec = cid.clone();

            let request = CompileRequest {
                notebook_id: state.notebook_id.get_untracked(),
                cell_id: cid,
                source: current_source,
                cargo_toml: current_cargo_toml,
                previous_cell_types,
                shared_cargo_toml: state.shared_cargo_toml.get_untracked(),
                shared_source: state.shared_source.get_untracked(),
            };

            let result = compile_cell(request).await;

            #[cfg(feature = "hydrate")]
            compile_time_ms.set(Some(js_sys::Date::now() - start));
            #[cfg(not(feature = "hydrate"))]
            compile_time_ms.set(Some(0.0));

            match result {
                Ok(response) => {
                    let has_errors = response
                        .diagnostics
                        .iter()
                        .any(|d| d.severity == Severity::Error);

                    if !response.wasm_blob.is_empty() && !has_errors {
                        // Compilation succeeded — load and execute the WASM blob.
                        #[cfg(feature = "hydrate")]
                        {
                            use crate::components::executor;

                            cell_status.set(CellStatus::Running);

                            let blob = response.wasm_blob.clone();
                            let js_glue = response.js_glue.clone();
                            last_compile.set(Some(response));

                            let hash = executor::hash_wasm_blob(&blob);

                            let exec_err =
                                match executor::load_blob(&cell_id_for_exec, &hash, &blob, js_glue)
                                    .await
                                {
                                    Ok(()) => {
                                        let exec_start = js_sys::Date::now();
                                        match executor::execute_cell(
                                            &cell_id_for_exec,
                                            &input_bytes,
                                        )
                                        .await
                                        {
                                            Ok((output_bytes, display_text, type_tag)) => {
                                                // Store output for downstream cells.
                                                state.cell_outputs.update(|map| {
                                                    map.insert(
                                                        cell_id_for_exec.clone(),
                                                        CellOutputData {
                                                            bytes: output_bytes.clone(),
                                                            type_tag: type_tag.clone(),
                                                        },
                                                    );
                                                });

                                                // Store display text for export.
                                                if let Some(ref dt) = display_text {
                                                    state.cell_display_texts.update(|map| {
                                                        map.insert(
                                                            cell_id_for_exec.clone(),
                                                            dt.clone(),
                                                        );
                                                    });
                                                }

                                                execution_result.set(Some(ExecutionResult {
                                                    display_text,
                                                    output_bytes,
                                                    execution_time_ms: js_sys::Date::now()
                                                        - exec_start,
                                                    type_tag,
                                                }));
                                                cell_status.set(CellStatus::Success);

                                                // Clear stale flag on successful execution.
                                                state.cell_stale.update(|stale| {
                                                    stale.remove(&cell_id_for_exec);
                                                });

                                                // Advance run-all queue on success.
                                                state.run_all_queue.update(|q| {
                                                    if q.first()
                                                        .map(|id| id == &cell_id_for_exec)
                                                        .unwrap_or(false)
                                                    {
                                                        q.remove(0);
                                                    }
                                                });

                                                None
                                            }
                                            Err(e) => Some(format!("Execution error: {e}")),
                                        }
                                    }
                                    Err(e) => Some(format!("WASM load error: {e}")),
                                };

                            if let Some(err_msg) = exec_err {
                                execution_result.set(Some(ExecutionResult {
                                    display_text: Some(err_msg),
                                    output_bytes: vec![],
                                    execution_time_ms: 0.0,
                                    type_tag: None,
                                }));
                                cell_status.set(CellStatus::Error);

                                // Stop run-all on execution error.
                                state.run_all_queue.set(vec![]);
                            }
                        }

                        #[cfg(not(feature = "hydrate"))]
                        {
                            cell_status.set(CellStatus::Success);
                            last_compile.set(Some(response));

                            // Clear stale flag on successful execution (SSR path).
                            state.cell_stale.update(|stale| {
                                stale.remove(&cell_id_for_exec);
                            });

                            // Advance run-all queue (SSR path).
                            state.run_all_queue.update(|q| {
                                if q.first().map(|id| id == &cell_id_for_exec).unwrap_or(false) {
                                    q.remove(0);
                                }
                            });
                        }
                    } else {
                        cell_status.set(CellStatus::Error);
                        last_compile.set(Some(response));

                        // Stop run-all on compile error.
                        state.run_all_queue.set(vec![]);
                    }
                }
                Err(e) => {
                    cell_status.set(CellStatus::Error);
                    last_compile.set(Some(CompileResponse {
                        wasm_blob: vec![],
                        diagnostics: vec![Diagnostic {
                            message: format!("Server error: {e}"),
                            severity: Severity::Error,
                            spans: vec![],
                            code: None,
                        }],
                        cached: false,
                        preamble_lines: 0,
                        js_glue: None,
                    }));

                    // Stop run-all on server error.
                    state.run_all_queue.set(vec![]);
                }
            }
        });
    });

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
            let Some(handle) = source_handle.get() else {
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
            run_closure.forget();
            handle.add_action("ironpad.runCell", &[2051], &run_cb);

            // Shift+Enter → run cell and advance focus to next cell.
            // Monaco KeyMod.Shift (1024) | KeyCode.Enter (3) = 1027
            let advance_closure = Closure::<dyn Fn()>::new(move || {
                run_trigger.update(|g| *g += 1);

                // Find and focus the next cell's editor.
                let cid = cell_id_for_keys.get_value();
                let cells = state.cells.get_untracked();
                let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
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
            advance_closure.forget();
            handle.add_action("ironpad.runCellAndAdvance", &[1027], &advance_cb);
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
            let Some(handle) = source_handle.get() else {
                return;
            };

            // Re-run when cell_outputs change (new type_tags after execution).
            let outputs = state.cell_outputs.get();
            let cells = state.cells.get_untracked();
            let cid = cell_id_for_ctx.get_value();
            let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);

            let variables = js_sys::Array::new();

            // Only Code cells produce output — skip markdown cells and use
            // sequential indices so variable names match CellInputs indices.
            for (i, c) in cells[..my_idx]
                .iter()
                .filter(|c| c.cell_type == CellType::Code)
                .enumerate()
            {
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
                            .map(|t| t.to_string())
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
            let compile = last_compile.get();
            let Some(handle) = source_handle.get_untracked() else {
                return;
            };

            let Some(response) = compile else {
                // Compile was cleared (e.g. new compile started); markers
                // already cleared by the compile-start code above.
                return;
            };

            let markers = js_sys::Array::new();

            for diag in &response.diagnostics {
                let severity: u32 = match diag.severity {
                    Severity::Error => 8,
                    Severity::Warning => 4,
                    Severity::Note => 2,
                };

                // If spans are available, create a marker per span.
                if diag.spans.is_empty() {
                    continue;
                }

                for span in &diag.spans {
                    let marker = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(
                        &marker,
                        &"startLineNumber".into(),
                        &(span.line_start).into(),
                    );
                    let _ = js_sys::Reflect::set(
                        &marker,
                        &"startColumn".into(),
                        &(span.col_start).into(),
                    );
                    let _ = js_sys::Reflect::set(
                        &marker,
                        &"endLineNumber".into(),
                        &(span.line_end).into(),
                    );
                    let _ =
                        js_sys::Reflect::set(&marker, &"endColumn".into(), &(span.col_end).into());

                    // Use span label if available, else fall back to diagnostic message.
                    let msg = span.label.as_deref().unwrap_or(&diag.message);
                    let _ = js_sys::Reflect::set(&marker, &"message".into(), &msg.into());
                    let _ = js_sys::Reflect::set(&marker, &"severity".into(), &severity.into());

                    markers.push(&marker);
                }
            }

            handle.set_markers(&markers);
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
        // the signal and persists it to IndexedDB via the notebook signal.
        let closure = Closure::<dyn Fn()>::new(move || {
            let val = source.get_untracked();
            let cid = cid_save.clone();
            state.notebook.update_untracked(|nb_opt| {
                if let Some(nb) = nb_opt {
                    if let Some(cell) = nb.cells.iter_mut().find(|c| c.id == cid) {
                        cell.source = val;
                    }
                }
            });
            persist_notebook(&state);
            source_dirty.set(false);
        });
        let save_fn: js_sys::Function =
            closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
        closure.forget();

        Callback::new(move |val: String| {
            source.set(val);
            source_dirty.set(true);

            // Mark this cell and all subsequent Code cells as stale.
            if !is_markdown {
                let cid = cell_id_for_stale_src.clone();
                state.cell_stale.update(|stale| {
                    let cells = state.cells.get_untracked();
                    let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
                    for cell in &cells[my_idx..] {
                        if cell.cell_type == CellType::Code {
                            stale.insert(cell.id.clone(), true);
                        }
                    }
                });
            }

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
            state.notebook.update_untracked(|nb_opt| {
                if let Some(nb) = nb_opt {
                    if let Some(cell) = nb.cells.iter_mut().find(|c| c.id == cid) {
                        cell.cargo_toml = Some(val);
                    }
                }
            });
            persist_notebook(&state);
            cargo_toml_dirty.set(false);
        });
        let save_fn: js_sys::Function =
            closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
        closure.forget();

        Callback::new(move |val: String| {
            cargo_toml.set(val);
            cargo_toml_dirty.set(true);

            // Mark this cell and all subsequent Code cells as stale.
            if !is_markdown {
                let cid = cell_id_for_stale_toml.clone();
                state.cell_stale.update(|stale| {
                    let cells = state.cells.get_untracked();
                    let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
                    for cell in &cells[my_idx..] {
                        if cell.cell_type == CellType::Code {
                            stale.insert(cell.id.clone(), true);
                        }
                    }
                });
            }

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

            state.notebook.update_untracked(|nb_opt| {
                if let Some(nb) = nb_opt {
                    if let Some(cell) = nb.cells.iter_mut().find(|c| c.id == cid) {
                        cell.source = src;
                        cell.cargo_toml = Some(toml);
                    }
                }
            });

            source_dirty.set(false);
            cargo_toml_dirty.set(false);
        });
    }

    // ── CSS classes ─────────────────────────────────────────────────────

    let cell_class = Signal::derive(move || {
        let mut class = "ironpad-cell-card".to_string();
        if is_markdown {
            class.push_str(" ironpad-cell-card--markdown");
        }
        if is_active() {
            class.push_str(" ironpad-cell-card--active");
        }
        if state.is_view_mode.get() {
            class.push_str(" ironpad-cell--view-mode");
        }
        class
    });

    let collapse_icon = Signal::derive(move || if collapsed.get() { "▸" } else { "▾" });

    let body_class = Signal::derive(move || {
        // In view mode, collapse the body only for Code cells (hides tabs
        // and editors while output panels remain visible outside the body).
        // Markdown cells keep the body open so the rendered preview shows.
        let should_collapse = collapsed.get() || (state.is_view_mode.get() && !is_markdown);
        if should_collapse {
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

            let handle = source_handle;
            let closure = Closure::<dyn Fn()>::new(move || {
                if let Some(h) = handle.get_untracked() {
                    h.focus();
                }
            });
            let focus_fn: js_sys::Function =
                closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
            closure.forget();

            let _ = web_sys::window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(&focus_fn, 300);
        }
    });

    // ── Render ──────────────────────────────────────────────────────────

    view! {
        <div node_ref=cell_wrapper_ref class="ironpad-cell-row">
        <Card
            class=cell_class
            on:click=on_click
        >
            <CardHeader>
                <div class="ironpad-cell-header">
                    <button
                        class="ironpad-cell-collapse-btn"
                        on:click=move |ev| {
                            ev.stop_propagation();
                            collapsed.update(|c| *c = !*c);
                        }
                    >
                        {collapse_icon}
                    </button>

                    <span class="ironpad-cell-order">
                        {format!("[{}]", cell.order)}
                    </span>

                    <input
                        class="ironpad-cell-label-input"
                        type="text"
                        prop:value=move || label.get()
                        prop:readOnly=move || state.is_view_mode.get()
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
                        if is_markdown {
                            return view! { <span /> }.into_any();
                        }
                        let is_stale = state.cell_stale.get()
                            .get(&cell_id_for_stale_header)
                            .copied()
                            .unwrap_or(false);
                        if is_stale {
                            view! { <span class="ironpad-stale-indicator" title="Cell output is stale">"⟳"</span> }.into_any()
                        } else {
                            view! { <span /> }.into_any()
                        }
                    }}

                    {if !is_markdown {
                        view! {
                            <Tag
                                size=TagSize::ExtraSmall
                                class=Signal::derive(move || {
                                    let suffix = match cell_status.get() {
                                        CellStatus::Idle => "idle",
                                        CellStatus::Queued => "queued",
                                        CellStatus::Compiling => "compiling",
                                        CellStatus::Running => "running",
                                        CellStatus::Success => "success",
                                        CellStatus::Error => "error",
                                    };
                                    format!("ironpad-cell-status ironpad-cell-status--{suffix}")
                                })
                            >
                                {move || {
                                    match cell_status.get() {
                                        CellStatus::Idle => "● idle".to_string(),
                                        CellStatus::Queued => "◎ queued".to_string(),
                                        CellStatus::Compiling => "◐ compiling…".to_string(),
                                        CellStatus::Running => "◐ running…".to_string(),
                                        CellStatus::Success => {
                                            match compile_time_ms.get() {
                                                Some(ms) => format!("✓ {ms:.0}ms"),
                                                None => "✓ done".to_string(),
                                            }
                                        }
                                        CellStatus::Error => "✕ error".to_string(),
                                    }
                                }}
                            </Tag>
                        }.into_any()
                    } else {
                        view! {
                            <Tag size=TagSize::ExtraSmall class="ironpad-cell-type-badge ironpad-cell-type-badge--markdown">
                                "📝 markdown"
                            </Tag>
                        }.into_any()
                    }}
                </div>
            </CardHeader>

            {if is_markdown {
                // ── Markdown cell body ──────────────────────────────────
                view! {
                    <div class=body_class>
                        <MarkdownCell
                            source=source.get_untracked()
                            on_change=on_source_change
                            cell_id=cell_id_for_markdown.get_value()
                        />
                    </div>
                }.into_any()
            } else {
                // ── Code cell body + panels ─────────────────────────────
                view! {
                    <div class=body_class>
                        <div class="ironpad-cell-tabs">
                            <TabList selected_value=selected_tab>
                                <Tab value="code">
                                    {move || if source_dirty.get() { "Code ●" } else { "Code" }}
                                </Tab>
                                <Tab value="cargo-toml">
                                    {move || if cargo_toml_dirty.get() { "Cargo.toml ●" } else { "Cargo.toml" }}
                                </Tab>
                            </TabList>
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
                    />

                    // ── Execution output panel ──────────────────────────────────
                    <CellOutputPanel
                        execution_result=execution_result
                        cell_id=cell_id_for_output.clone()
                        cell_outputs=state.cell_outputs
                        cell_stale=state.cell_stale
                        cells=state.cells
                        run_all_queue=state.run_all_queue
                    />
                }.into_any()
            }}
        </Card>

        // ── Side action buttons ─────────────────────────────────────────
        <div class="ironpad-cell-side-actions">
            {if !is_markdown {
                view! {
                    <button
                        class="ironpad-side-btn"
                        title="Run cell"
                        on:click=move |ev: leptos::ev::MouseEvent| {
                            ev.stop_propagation();
                            run_trigger.update(|g| *g += 1);
                        }
                    >
                        {move || {
                            if matches!(cell_status.get(), CellStatus::Compiling | CellStatus::Running) {
                                "⏳"
                            } else {
                                "▶"
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
                        "⚙"
                    </button>
                }.into_any()
            } else {
                view! { <span /> }.into_any()
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
                    "⋯"
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
                                "↑ Move Up"
                            </button>
                            <button
                                class="ironpad-cell-menu-item"
                                disabled=last
                                on:click=on_move_down
                            >
                                "↓ Move Down"
                            </button>
                            <button
                                class="ironpad-cell-menu-item"
                                on:click=on_duplicate
                            >
                                "⧉ Duplicate"
                            </button>
                            {if !is_markdown {
                                view! {
                                    <button
                                        class="ironpad-cell-menu-item"
                                        on:click=on_run_all_below
                                    >
                                        "▶▶ Run All Below"
                                    </button>
                                }.into_any()
                            } else {
                                view! { <span /> }.into_any()
                            }}
                            <div class="ironpad-cell-menu-divider" />
                            <button
                                class="ironpad-cell-menu-item ironpad-cell-menu-item--danger"
                                on:click=on_delete_confirmed
                            >
                                "🗑 Delete"
                            </button>
                        </div>
                    }.into_any()
                }}
            </div>
        </div>
        </div>
    }
}
