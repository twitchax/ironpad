//! The cell compile/execute pipeline and the live check-on-type dispatch.
//!
//! Extracted from `CellItem` (PRD-0055 T-001): the component renders and
//! wires; this module runs cells. The fanout review's evidence for the
//! split is concrete — workflow logic written inline in view code is where
//! the shipped bugs were (the inline Unpublish flow missed the flush
//! discipline every extracted sibling had). The stages consume the shared
//! seams minted by the review waves (`assemble_cell_inputs`,
//! `unexecuted_upstream`, `probe_local`/`store_unless_served`,
//! `abort_run_cascade`) rather than re-wrapping them.

use ironpad_common::{
    CellManifest, CellType, CompileRequest, CompileResponse, Diagnostic, ExecutionResult, Severity,
};
use leptos::prelude::*;

use crate::components::monaco_editor::MonacoEditorHandle;
use crate::model::NotebookModel;
use crate::server_fns::compile_cell;

#[cfg(feature = "hydrate")]
use super::state::CellOutputData;
use super::state::{CellStatus, NotebookState};

// ── Run context ─────────────────────────────────────────────────────────────

/// Everything the run pipeline touches for one cell, bundled so the stages
/// are free functions instead of closures over a 2,000-line component scope.
/// All fields are `Copy` (signals, stored values, and the two state handles).
#[derive(Clone, Copy)]
pub(super) struct CellRunCtx {
    pub(super) state: NotebookState,
    pub(super) model: NotebookModel,
    pub(super) cell_id: StoredValue<String>,
    pub(super) is_markdown: bool,
    pub(super) is_shared: Signal<bool>,
    pub(super) cell_status: RwSignal<CellStatus>,
    pub(super) last_compile: RwSignal<Option<CompileResponse>>,
    pub(super) compile_time_ms: RwSignal<Option<f64>>,
    pub(super) execution_result: RwSignal<Option<ExecutionResult>>,
    pub(super) source: RwSignal<String>,
    pub(super) cargo_toml: RwSignal<String>,
    pub(super) source_handle: RwSignal<Option<MonacoEditorHandle>>,
    /// `None` when the editor mounts without session wiring; events are
    /// emitted only while a session is live (PRD-0052) — otherwise they
    /// would pile into the model's buffer and flush as a stale backlog the
    /// moment a session starts.
    pub(super) session: Option<crate::session::SessionState>,
}

impl CellRunCtx {
    /// Emit a compile/execution event for connected agents, gated on a live
    /// session (see the `session` field docs).
    fn emit_session_event(&self, event: ironpad_common::protocol::Event) {
        if self
            .session
            .is_some_and(|s| s.active.try_get_untracked() == Some(true))
        {
            self.model.emit_event(event);
        }
    }
}

// ── Pure stages ─────────────────────────────────────────────────────────────

/// The invalidation set for a (re)run of `cell_id`: that cell and every Code
/// cell after it, whose cached outputs and display texts are cleared before
/// the compile so downstream cells cannot pipe from a stale world.
fn downstream_code_ids(cells: &[CellManifest], cell_id: &str) -> Vec<String> {
    let my_idx = cells.iter().position(|c| c.id == cell_id).unwrap_or(0);
    cells[my_idx..]
        .iter()
        .filter(|c| c.cell_type == CellType::Code)
        .map(|c| c.id.clone())
        .collect()
}

// ── The run pipeline ────────────────────────────────────────────────────────

/// Wire the compile/execute flow to `run_trigger`: cascade prerequisites,
/// assemble piped inputs, compile (local blob cache first), load + execute,
/// and do the status/output/queue bookkeeping — including the session events
/// agents correlate on (PRD-0052).
pub(super) fn wire_run_effect(ctx: &CellRunCtx, run_trigger: RwSignal<u64>) {
    let ctx = *ctx;
    let CellRunCtx {
        state,
        cell_id,
        is_markdown,
        is_shared,
        cell_status,
        last_compile,
        compile_time_ms,
        execution_result,
        source,
        cargo_toml,
        source_handle,
        ..
    } = ctx;
    let emit_session_event = move |event: ironpad_common::protocol::Event| {
        ctx.emit_session_event(event);
    };

    // The actual compile flow, driven by `run_trigger`.
    Effect::new(move || {
        let gen = run_trigger.get();
        if gen == 0 {
            return;
        }

        // Markdown cells skip compilation entirely; shared cells compile as
        // part of every other cell's shared.rs, never on their own.
        if is_markdown || is_shared.get_untracked() {
            return;
        }

        // Avoid double-dispatch while already compiling or running.
        if matches!(
            cell_status.get_untracked(),
            CellStatus::Compiling | CellStatus::Running
        ) {
            return;
        }

        let cid = cell_id.get_value();
        let current_source = source.get_untracked();
        let current_cargo_toml = cargo_toml.get_untracked();

        // ── Cascading execution ─────────────────────────────────────────
        // Unexecuted upstream runnable cells queue first (plus this cell) —
        // the one shared recipe (`unexecuted_upstream`), same as the viewer
        // and the agent path.
        {
            let cells = state.cells.get_untracked();
            let outputs = state.cell_outputs.get_untracked();
            let unexecuted =
                crate::components::executor::unexecuted_upstream(&cells, &cid, &outputs);

            if !unexecuted.is_empty() {
                let mut queue = unexecuted;
                queue.push(cid.clone());
                state.run_all_queue.set(queue);
                return;
            }
        }

        // Collect previous cell outputs for the I/O pipeline — the one
        // shared recipe (`assemble_cell_inputs`): positional slots, empty
        // for markdown/unexecuted, types feeding the cache key.
        let (input_bytes, previous_cell_types) = {
            let cells = state.cells.get_untracked();
            let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
            let outputs = state.cell_outputs.get_untracked();
            crate::components::executor::assemble_cell_inputs(&cells, my_idx, &outputs)
        };

        // Used inside #[cfg(feature = "hydrate")] below.
        let _ = &input_bytes;

        // Mark compiling BEFORE invalidating outputs so the downstream-clear effect
        // (see below) can tell this cell's own re-run (status == Compiling) apart from
        // an upstream re-run that invalidated us (status still Idle/Success/Error).
        cell_status.set(CellStatus::Compiling);
        emit_session_event(ironpad_common::protocol::Event::CellCompiling {
            cell_id: cid.clone(),
        });

        // Invalidate downstream Code cells' cached outputs (this cell and all after it).
        {
            let cells = state.cells.get_untracked();
            let downstream_ids = downstream_code_ids(&cells, &cid);
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

            let request_cargo_toml_for_warmth = current_cargo_toml.clone();
            let request = CompileRequest {
                notebook_id: state.notebook_id.get_untracked(),
                cell_id: cid,
                source: current_source,
                cargo_toml: current_cargo_toml,
                previous_cell_types,
                shared_cargo_toml: state.shared_cargo_toml.get_untracked(),
                // Notebook-level shared source plus every shared cell, in
                // notebook order — the single assembly defined in
                // ironpad-common (PRD-0044).
                shared_source: state.notebook.with_untracked(|nb| {
                    nb.as_ref()
                        .and_then(ironpad_common::IronpadNotebook::effective_shared_source)
                }),
                force: state.force_recompile.get_untracked(),
                shared_check: None,
            };

            // Local blob store (PRD-0047): cache mode probes IndexedDB before
            // paying the compile round trip; Fresh mode always compiles, and
            // the fresh result overwrites the local entry below.
            #[cfg(feature = "hydrate")]
            let (result, served_locally, request_hash) = {
                let (request_hash, local_hit) = crate::blob_cache::probe_local(&request).await;
                let served_locally = local_hit.is_some();
                let result = match local_hit {
                    Some(response) => Ok(response),
                    None => compile_cell(request).await,
                };
                (result, served_locally, request_hash)
            };
            #[cfg(not(feature = "hydrate"))]
            let result = compile_cell(request).await;

            // If the cell (or the whole page) was disposed while the compile was
            // in flight, its signals are gone — drop the result rather than panic
            // on a reclaimed reactive slot (PRD-0045 async lifecycle).
            if cell_status.try_get_untracked().is_none() {
                return;
            }

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

                    emit_session_event(ironpad_common::protocol::Event::CellCompiled {
                        cell_id: cell_id_for_exec.clone(),
                        diagnostics: response.diagnostics.clone(),
                        success: !has_errors,
                    });

                    // A clean compile proves this manifest's dependency tree
                    // is built — custom-deps cells become live-check eligible
                    // (PRD-0045 warmth policy).
                    if !has_errors {
                        let key = warm_manifest_key(
                            state.shared_cargo_toml.get_untracked().as_deref(),
                            &request_cargo_toml_for_warmth,
                        );
                        state.warm_manifests.update(|set| {
                            set.insert(key);
                        });
                    }

                    if !response.wasm_blob.is_empty() && !has_errors {
                        // Compilation succeeded — load and execute the WASM blob.
                        #[cfg(feature = "hydrate")]
                        {
                            use crate::components::executor;

                            cell_status.set(CellStatus::Running);

                            // Persist server results locally (PRD-0047).
                            crate::blob_cache::store_unless_served(
                                served_locally,
                                request_hash.as_deref(),
                                &response,
                            )
                            .await;

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
                                            Ok((
                                                output_bytes,
                                                display_text,
                                                type_tag,
                                                ran_on_main_thread,
                                            )) => {
                                                // Disposed during load/exec — bail
                                                // before touching cell/page signals.
                                                if cell_status.try_get_untracked().is_none() {
                                                    return;
                                                }
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

                                                let exec_ms = js_sys::Date::now() - exec_start;
                                                emit_session_event(
                                                    ironpad_common::protocol::Event::CellExecuted {
                                                        cell_id: cell_id_for_exec.clone(),
                                                        display_text: display_text.clone(),
                                                        type_tag: type_tag.clone(),
                                                        execution_time_ms: exec_ms,
                                                        success: true,
                                                    },
                                                );
                                                execution_result.set(Some(ExecutionResult {
                                                    display_text,
                                                    output_bytes,
                                                    execution_time_ms: exec_ms,
                                                    type_tag,
                                                    ran_on_main_thread,
                                                }));
                                                cell_status.set(CellStatus::Success);

                                                // Clear stale flag on successful execution.
                                                state.cell_stale.update(|stale| {
                                                    stale.remove(&cell_id_for_exec);
                                                });

                                                // Clear blocked-by entries that referenced this cell.
                                                state.cell_blocked_by.update(|blocked| {
                                                    blocked.retain(|_, blocker| {
                                                        *blocker != cell_id_for_exec
                                                    });
                                                });

                                                // Advance run-all queue on success.
                                                state.run_all_queue.update(|q| {
                                                    if q.first()
                                                        .is_some_and(|id| id == &cell_id_for_exec)
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

                            // Disposed during load/exec (error path) — bail before
                            // writing status onto a reclaimed reactive slot.
                            if cell_status.try_get_untracked().is_none() {
                                return;
                            }

                            if let Some(err_msg) = exec_err {
                                // Terminal for a waiting agent either way: the
                                // run will not produce a success event.
                                emit_session_event(ironpad_common::protocol::Event::CellExecuted {
                                    cell_id: cell_id_for_exec.clone(),
                                    display_text: Some(err_msg.clone()),
                                    type_tag: None,
                                    execution_time_ms: 0.0,
                                    success: false,
                                });
                                // AbortError means the user cancelled via terminate().
                                if err_msg.contains("AbortError") {
                                    cell_status.set(CellStatus::Idle);
                                    state.run_all_queue.set(vec![]);
                                } else {
                                    execution_result.set(Some(ExecutionResult {
                                        display_text: Some(err_msg),
                                        output_bytes: vec![],
                                        execution_time_ms: 0.0,
                                        type_tag: None,
                                        ran_on_main_thread: false,
                                    }));
                                    cell_status.set(CellStatus::Error);

                                    // Stop run-all on execution error.
                                    state.abort_run_cascade(&cell_id_for_exec);
                                }
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

                            // Clear blocked-by entries that referenced this cell (SSR path).
                            state.cell_blocked_by.update(|blocked| {
                                blocked.retain(|_, blocker| *blocker != cell_id_for_exec);
                            });

                            // Advance run-all queue (SSR path).
                            state.run_all_queue.update(|q| {
                                if q.first().is_some_and(|id| id == &cell_id_for_exec) {
                                    q.remove(0);
                                }
                            });
                        }
                    } else {
                        cell_status.set(CellStatus::Error);
                        last_compile.set(Some(response));

                        // Stop run-all on compile error.
                        state.abort_run_cascade(&cell_id_for_exec);
                    }
                }
                Err(e) => {
                    cell_status.set(CellStatus::Error);
                    let diagnostics = vec![Diagnostic {
                        message: format!("Server error: {e}"),
                        severity: Severity::Error,
                        spans: vec![],
                        code: None,
                    }];
                    emit_session_event(ironpad_common::protocol::Event::CellCompiled {
                        cell_id: cell_id_for_exec.clone(),
                        diagnostics: diagnostics.clone(),
                        success: false,
                    });
                    last_compile.set(Some(CompileResponse {
                        wasm_blob: vec![],
                        diagnostics,
                        cached: false,
                        preamble_lines: 0,
                        js_glue: None,
                    }));

                    // Stop run-all on server error.
                    state.abort_run_cascade(&cell_id_for_exec);
                }
            }
        });
    });
}

// ── Live check-on-type helpers (PRD-0045) ───────────────────────────────────

/// Key identifying a dependency manifest pair for the warmth policy.
pub(super) fn warm_manifest_key(shared_cargo_toml: Option<&str>, cell_cargo_toml: &str) -> String {
    format!(
        "{}\u{0}{}",
        shared_cargo_toml.unwrap_or(""),
        cell_cargo_toml
    )
}

/// Convert preamble-adjusted diagnostics into Monaco model markers.
#[cfg(feature = "hydrate")]
pub(super) fn diagnostics_to_markers(diagnostics: &[Diagnostic]) -> js_sys::Array {
    let markers = js_sys::Array::new();

    for diag in diagnostics {
        let severity: u32 = match diag.severity {
            Severity::Error => 8,
            Severity::Warning => 4,
            Severity::Note => 2,
        };

        for span in &diag.spans {
            let marker = js_sys::Object::new();
            let _ = js_sys::Reflect::set(
                &marker,
                &"startLineNumber".into(),
                &(span.line_start).into(),
            );
            let _ = js_sys::Reflect::set(&marker, &"startColumn".into(), &(span.col_start).into());
            let _ = js_sys::Reflect::set(&marker, &"endLineNumber".into(), &(span.line_end).into());
            let _ = js_sys::Reflect::set(&marker, &"endColumn".into(), &(span.col_end).into());

            // Use span label if available, else fall back to diagnostic message.
            let msg = span.label.as_deref().unwrap_or(&diag.message);
            let _ = js_sys::Reflect::set(&marker, &"message".into(), &msg.into());
            let _ = js_sys::Reflect::set(&marker, &"severity".into(), &severity.into());

            markers.push(&marker);
        }
    }

    markers
}

/// Fire a live check for `cid` if the cell is eligible, discarding the
/// response unless it is still the newest dispatch (generation match).
///
/// Eligibility (PRD-0045): code cell, not mid-compile, warm by the manifest
/// policy, and not referencing piped inputs that have no outputs yet (those
/// would produce false "cannot find value cellN" noise — the run path
/// cascades predecessors, a check cannot).
///
/// Shared cells (PRD-0046) check the assembled `shared.rs` instead: the
/// request carries a throwaway cell body plus the target cell's line slice,
/// and the server returns only diagnostics anchored inside that slice,
/// remapped to cell-local lines.
#[cfg(feature = "hydrate")]
#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch_live_check(
    state: &NotebookState,
    cid: String,
    is_markdown: bool,
    is_shared: Signal<bool>,
    cell_status: RwSignal<CellStatus>,
    source: RwSignal<String>,
    cargo_toml: RwSignal<String>,
    last_check: RwSignal<Option<Vec<Diagnostic>>>,
    check_generation: RwSignal<u64>,
) {
    use ironpad_common::{manifest_has_custom_deps, CheckStatus, SharedCheckRange};

    /// The cell body sent with a shared-cell check: the real code under test
    /// rides in `shared_source`, but the scaffold still needs a valid cell.
    const SHARED_CHECK_BODY: &str = "String::new()";

    if is_markdown {
        return;
    }
    if matches!(
        cell_status.get_untracked(),
        CellStatus::Compiling | CellStatus::Running
    ) {
        return;
    }

    let shared = is_shared.get_untracked();
    let current_source = source.get_untracked();
    let shared_cargo = state.shared_cargo_toml.get_untracked();

    // A shared cell's own manifest never reaches the build (consumers merge
    // THEIR manifest with the shared one), so its check compiles under the
    // default cell manifest.
    let current_cargo_toml = if shared {
        "[dependencies]".to_string()
    } else {
        cargo_toml.get_untracked()
    };

    // Warmth policy: default deps are pre-checked by the image warmup;
    // custom deps must have compiled once this session.
    if manifest_has_custom_deps(shared_cargo.as_deref(), &current_cargo_toml) {
        let key = warm_manifest_key(shared_cargo.as_deref(), &current_cargo_toml);
        if !state
            .warm_manifests
            .with_untracked(|set| set.contains(&key))
        {
            return;
        }
    }

    // Shared cells: locate this cell's slice inside the assembly, from the
    // same model state the shared_source below is assembled from.
    let shared_check: Option<SharedCheckRange> = if shared {
        let range = state.notebook.with_untracked(|nb| {
            let nb = nb.as_ref()?;
            let start_line = ironpad_common::shared_cell_line_offset(
                nb.shared_source.as_deref(),
                &nb.cells,
                &cid,
            )?;
            let cell = nb.cells.iter().find(|c| c.id == cid)?;
            let line_count = u32::try_from(cell.source.split('\n').count()).ok()?;
            Some(SharedCheckRange {
                start_line,
                line_count,
            })
        });
        match range {
            Some(r) => Some(r),
            // Not (yet) in the assembly — e.g. the shared toggle is still in
            // flight. No slice to check against; try again next debounce.
            None => return,
        }
    } else {
        None
    };

    // Piped-input references with missing predecessor outputs would produce
    // false errors; the compile path cascades, a check can't. (Shared cells
    // hold empty piping slots and their check body references nothing.)
    let previous_cell_types: Vec<String> = if shared {
        vec![]
    } else {
        let cells = state.cells.get_untracked();
        let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
        let outputs = state.cell_outputs.get_untracked();
        let types: Vec<String> = cells[..my_idx]
            .iter()
            .map(|c| {
                if c.cell_type == CellType::Code {
                    outputs
                        .get(&c.id)
                        .and_then(|d| d.type_tag.clone())
                        .unwrap_or_default()
                } else {
                    String::new()
                }
            })
            .collect();
        let references_pipes = (0..10).any(|i| current_source.contains(&format!("cell{i}")));
        if references_pipes && types.iter().all(String::is_empty) {
            return;
        }
        types
    };

    let generation = check_generation.get_untracked() + 1;
    check_generation.set(generation);

    let request = CompileRequest {
        notebook_id: state.notebook_id.get_untracked(),
        cell_id: cid,
        source: if shared {
            SHARED_CHECK_BODY.to_string()
        } else {
            current_source
        },
        cargo_toml: current_cargo_toml,
        previous_cell_types,
        shared_cargo_toml: shared_cargo,
        shared_source: state.notebook.with_untracked(|nb| {
            nb.as_ref()
                .and_then(ironpad_common::IronpadNotebook::effective_shared_source)
        }),
        force: false,
        shared_check,
    };

    leptos::task::spawn_local(async move {
        let Ok(response) = crate::server_fns::check_cell(request).await else {
            // Transport/infra failure: leave existing markers alone.
            return;
        };
        // If the cell (or the whole page) was disposed while the check was in
        // flight, its signals are gone — drop the result rather than panic on a
        // reclaimed reactive slot. A newer dispatch supersedes this one too.
        let Some(current_generation) = check_generation.try_get_untracked() else {
            return;
        };
        if current_generation != generation {
            return;
        }
        match response.status {
            CheckStatus::Clean | CheckStatus::Errors => {
                last_check.set(Some(response.diagnostics));
            }
            // Busy or cold: no information this round; keep current markers.
            CheckStatus::Skipped | CheckStatus::TimedOut => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, cell_type: CellType) -> CellManifest {
        CellManifest {
            id: id.to_string(),
            order: 0,
            label: id.to_string(),
            cell_type,
            shared: false,
            collapsed: false,
            output_collapsed: false,
        }
    }

    #[test]
    fn downstream_invalidation_covers_self_and_later_code_cells_only() {
        let cells = vec![
            manifest("before", CellType::Code),
            manifest("me", CellType::Code),
            manifest("md", CellType::Markdown),
            manifest("after", CellType::Code),
        ];
        assert_eq!(
            downstream_code_ids(&cells, "me"),
            vec!["me".to_string(), "after".to_string()],
            "upstream and markdown cells keep their outputs"
        );
        // Unknown id degrades to index 0 (invalidate everything runnable),
        // never to a panic.
        assert_eq!(downstream_code_ids(&cells, "gone").len(), 3);
    }

    #[test]
    fn warm_manifest_key_separates_the_pair_unambiguously() {
        // NUL-separated so ("a", "b") and ("ab", "") cannot collide.
        assert_ne!(
            warm_manifest_key(Some("a"), "b"),
            warm_manifest_key(Some("ab"), "")
        );
        assert_eq!(
            warm_manifest_key(None, "x"),
            warm_manifest_key(Some(""), "x")
        );
    }
}
