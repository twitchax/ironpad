//! The cell compile/execute pipeline and the live check-on-type dispatch.
//!
//! Extracted from `CellItem` (PRD-0055 T-001): the component renders and
//! wires; this module runs cells. The fanout review's evidence for the
//! split is concrete — workflow logic written inline in view code is where
//! the shipped bugs were (the inline Unpublish flow missed the flush
//! discipline every extracted sibling had). The stages consume the shared
//! seams minted by the review waves (`assemble_cell_inputs`,
//! `unexecuted_dependencies`, `probe_local`/`store_unless_served`,
//! `fail_in_run_queue`) rather than re-wrapping them.

use std::collections::{HashMap, HashSet};

use ironpad_common::{
    CellManifest, CellType, CompileRequest, CompileResponse, Diagnostic, ExecutionResult,
    IronpadNotebook, Severity,
};
use leptos::prelude::*;

use crate::components::monaco_editor::MonacoEditorHandle;
use crate::components::run_flow::RunGuard;
use crate::model::NotebookModel;
#[cfg(not(feature = "hydrate"))]
use crate::server_fns::compile_cell;
use crate::session::SessionState;

use super::state::{CellOutputData, CellStatus, NotebookState};

// ── Run context ─────────────────────────────────────────────────────────────

/// Everything the run pipeline touches for one cell, bundled so the stages
/// are free functions instead of closures over a 2,000-line component scope.
/// All fields are `Copy` (signals, stored values, and the two state handles).
#[derive(Clone, Copy)]
pub(super) struct CellRunCtx {
    pub(super) state: NotebookState,
    pub(super) model: NotebookModel,
    pub(super) cell_id: StoredValue<String>,
    /// What the cell IS. Was `is_markdown: bool`, which silently conflated
    /// three different questions — "render as prose", "cannot run here", and
    /// "different chrome" — and answered the second one wrong for `Linux`
    /// cells, which are not markdown and so were handed a Run button that
    /// compiled them through the Code scaffold (PRD-0066).
    pub(super) cell_type: CellType,
    pub(super) is_shared: Signal<bool>,
    pub(super) cell_status: RwSignal<CellStatus>,
    pub(super) last_compile: RwSignal<Option<CompileSummary>>,
    pub(super) compile_time_ms: RwSignal<Option<f64>>,
    pub(super) execution_result: RwSignal<Option<ExecutionResult>>,
    pub(super) source: RwSignal<String>,
    pub(super) cargo_toml: RwSignal<String>,
    pub(super) source_handle: RwSignal<Option<MonacoEditorHandle>>,
    /// `None` when the editor mounts without session wiring; events are
    /// emitted only while a session is live (PRD-0052) — otherwise they
    /// would pile into the model's buffer and flush as a stale backlog the
    /// moment a session starts.
    pub(super) session: Option<SessionState>,
    /// The epoch behind this cell's [`RunGuard`]: minted when a run starts
    /// compiling, revoked by a Stop.
    pub(super) run_epoch: RwSignal<u64>,
}

/// Emit a compile/execution event for connected agents, gated on a live
/// session (see [`CellRunCtx::session`]). Free so the page-level cancel path,
/// which has no cell context, gates the same way.
fn emit_session_event(
    model: NotebookModel,
    session: Option<SessionState>,
    event: ironpad_common::protocol::Event,
) {
    if session.is_some_and(|s| s.active.try_get_untracked() == Some(true)) {
        model.emit_event(event);
    }
}

/// What a cancel reports: the stopped cell first (a direct run never queued
/// it, and an agent may be waiting on it), then every queued cell the cancel
/// dropped. With no stopped cell (leaving the editor) it is just the drops.
fn queue_drops_on_cancel(target: Option<&str>, dropped: Vec<String>) -> Vec<String> {
    target
        .map(str::to_string)
        .into_iter()
        .chain(dropped)
        .collect()
}

/// Empty the run queue for a cancel and report it to agents as ONE
/// `RunCancelled` naming `target` (the stopped cell, if any) and every
/// dropped queued cell. Free rather than a `CellRunCtx` method because the
/// editor's Preview toggle cancels too, and it has no cell. Drops are always
/// reported (PRD-0052): without this, a waiting agent learned only at its
/// timeout.
pub(super) fn cancel_run_queue(
    state: &NotebookState,
    model: NotebookModel,
    session: Option<SessionState>,
    target: Option<&str>,
) {
    let cell_ids = queue_drops_on_cancel(target, state.run_all_queue.cancel(target));
    if !cell_ids.is_empty() {
        emit_session_event(
            model,
            session,
            ironpad_common::protocol::Event::RunCancelled { cell_ids },
        );
    }
}

impl CellRunCtx {
    /// Emit a compile/execution event for connected agents, gated on a live
    /// session (see the `session` field docs).
    fn emit_session_event(&self, event: ironpad_common::protocol::Event) {
        emit_session_event(self.model, self.session, event);
    }

    /// The Stop button: ONE policy for both phases, so neither can skip the
    /// report. During the compile there is nothing to abort (the server fn
    /// is unabortable and the worker holds nothing of this run), so the run
    /// gives up its claim and its result is discarded when it lands; the
    /// queue is cancelled and reported here. During execution the worker
    /// holds the run, so `terminate()` aborts it and the `AbortError`
    /// completion cancels the queue (see `wire_run_effect`).
    pub(super) fn cancel(&self) {
        match self.cell_status.get_untracked() {
            CellStatus::Compiling => {
                RunGuard::revoke(self.run_epoch);
                self.cell_status.set(CellStatus::Idle);
                let cid = self.cell_id.get_value();
                cancel_run_queue(&self.state, self.model, self.session, Some(&cid));
            }
            CellStatus::Running => {
                #[cfg(feature = "hydrate")]
                crate::components::executor::terminate_executor();
            }
            _ => {}
        }
    }

    /// Continue-past-failures with the drop REPORT structurally tied to the
    /// drop decision: drop `failed_cell_id`'s transitive dependents from the
    /// queue and emit `CellBlocked` for each — the browser makes the drop
    /// decision, so it reports it, and a waiting agent terminal-izes on the
    /// report instead of inferring dependencies from a possibly stale
    /// notebook cache. ONE call for every failure path; a future path that
    /// drops without reporting cannot exist by construction.
    fn fail_and_report(&self, failed_cell_id: &str) {
        for dep in self.state.fail_in_run_queue(failed_cell_id) {
            self.emit_session_event(ironpad_common::protocol::Event::CellBlocked {
                cell_id: dep,
                blocked_by: failed_cell_id.to_string(),
            });
        }
    }
}

// ── Compile summary ─────────────────────────────────────────────────────────

/// What a cell keeps of its last compile: everything its readers use (the
/// inline markers, the "Compiled (… KB" line, the error panel) and never the
/// wasm blob itself. Holding the whole `CompileResponse` kept a copy of every
/// cell's blob (often 1 MB or more for rayon/simd cells) resident for the
/// page's lifetime, and cloned it again on each panel render.
#[derive(Clone, Debug)]
pub(super) struct CompileSummary {
    pub(super) diagnostics: Vec<Diagnostic>,
    pub(super) blob_len: usize,
    pub(super) cached: bool,
}

impl From<&CompileResponse> for CompileSummary {
    fn from(response: &CompileResponse) -> Self {
        Self {
            diagnostics: response.diagnostics.clone(),
            blob_len: response.wasm_blob.len(),
            cached: response.cached,
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

// ── Superseded runs ─────────────────────────────────────────────────────────

/// What a run was built from, captured at dispatch and compared when it
/// completes. Nothing else ties a result to the text it compiled: an edit
/// during a compile (a human typing on, or an agent's `cells update` then
/// `cells run`, which the in-flight run satisfies) used to be reported as
/// that edit's result, and the completion wiped the stale flag the edit set.
#[derive(Clone, PartialEq)]
struct RunInputs {
    source: String,
    cargo_toml: String,
    shared_cargo_toml: Option<String>,
    /// The notebook-level shared source plus every shared cell (PRD-0044).
    shared_source: Option<String>,
    /// Each upstream runnable cell's source. Not compiled into this cell
    /// (its OUTPUT is, and an edit does not change that), but editing one
    /// marks this cell stale, and a run must not wipe a flag set after its
    /// inputs were taken.
    upstream: Vec<String>,
}

impl RunInputs {
    /// `cell_id`'s inputs right now; `None` once the cell or page is gone.
    /// The cell's own signals are read first, so a disposed page is never
    /// reached through `state`.
    fn current(ctx: &CellRunCtx, cell_id: &str) -> Option<Self> {
        let source = ctx.source.try_get_untracked()?;
        let cargo_toml = ctx.cargo_toml.try_get_untracked()?;
        let state = ctx.state;
        let shared_cargo_toml = state.shared_cargo_toml.try_get_untracked()?;
        let shared_source = state.notebook.try_with_untracked(|nb| {
            nb.as_ref()
                .and_then(IronpadNotebook::effective_shared_source)
        })?;
        let sources = state.current_cell_sources();
        let upstream = state
            .cells
            .try_with_untracked(|cells| upstream_runnable_sources(cells, cell_id, &sources))?;
        Some(Self {
            source,
            cargo_toml,
            shared_cargo_toml,
            shared_source,
            upstream,
        })
    }

    /// Whether `now` would compile a different program than `self` did.
    fn recompile_needed(&self, now: &Self) -> bool {
        self.source != now.source
            || self.cargo_toml != now.cargo_toml
            || self.shared_cargo_toml != now.shared_cargo_toml
            || self.shared_source != now.shared_source
    }
}

/// The sources of the runnable cells before `cell_id`, in notebook order.
/// Positional, like the stale marking it guards (`mark_downstream_stale`).
fn upstream_runnable_sources(
    cells: &[CellManifest],
    cell_id: &str,
    sources: &HashMap<String, String>,
) -> Vec<String> {
    let my_idx = cells.iter().position(|c| c.id == cell_id).unwrap_or(0);
    cells[..my_idx]
        .iter()
        .filter(|c| c.is_runnable())
        .map(|c| sources.get(&c.id).cloned().unwrap_or_default())
        .collect()
}

/// A run whose compiled inputs changed while it was in flight, at the front
/// of the queue: drop its result and dispatch the cell again on what it says
/// now. Returns whether it did. Nothing is reported for the dropped result,
/// so a waiting `cells run` sees only the new run's events (a fresh
/// `CellCompiling` first) and never a stale success or `compile_error`. No
/// wire format changes, and older daemons are unaffected.
///
/// A run that is not the queue's (a human's direct click) keeps its result
/// and its events; the cell just stays stale (see the completion below).
fn redispatch_if_superseded(
    ctx: &CellRunCtx,
    compiled: &RunInputs,
    cell_id: &str,
    run_trigger: RwSignal<u64>,
) -> bool {
    let superseded =
        RunInputs::current(ctx, cell_id).is_some_and(|now| compiled.recompile_needed(&now));
    if !superseded || !ctx.state.run_all_queue.is_front_untracked(cell_id) {
        return false;
    }
    ctx.cell_status.set(CellStatus::Idle);
    run_trigger.update(|g| *g += 1);
    true
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
        cell_type,
        is_shared,
        cell_status,
        last_compile,
        compile_time_ms,
        execution_result,
        source,
        cargo_toml,
        source_handle,
        run_epoch,
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
        // Not "is prose": "does not execute here". Markdown has no code,
        // shared cells compile into every OTHER cell, and Linux cells run in
        // the viewer's pod rather than the editor's executor.
        if cell_type != CellType::Code || is_shared.get_untracked() {
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
        // Unexecuted cells this one transitively CONSUMES queue first (plus
        // this cell) — the one shared recipe (`unexecuted_dependencies`,
        // PRD-0060), same as the viewer and the agent path. Independent
        // upstream cells no longer cascade: the scaffold binds only
        // referenced slots, so skipping them is safe. While a queue is in
        // flight, the request MERGES into it (`RunQueue::request`): the
        // queue owns execution, and this cell waits its turn.
        {
            let sources = state.current_cell_sources();
            // Borrowed, not cloned: the helpers take slices and maps.
            let queued = state.cells.with_untracked(|cells| {
                let unexecuted = state.cell_outputs.with_untracked(|outputs| {
                    crate::components::executor::unexecuted_dependencies(
                        cells,
                        &cid,
                        outputs,
                        |id| sources.get(id).cloned(),
                    )
                });
                let order: Vec<&str> = cells.iter().map(|c| c.id.as_str()).collect();
                state.run_all_queue.request(&order, &unexecuted, &cid)
            });
            if queued {
                return;
            }
        }

        // A dispatched run is a fresh attempt: clear any blocked notice this
        // cell carries (same policy as the viewer) before status changes.
        crate::components::run_flow::clear_own_blame(state.cell_blocked_by, &cid);

        // Collect previous cell outputs for the I/O pipeline — the one
        // shared recipe (`assemble_cell_inputs`): positional slots, empty
        // for markdown/unexecuted, types feeding the cache key.
        let (input_bytes, previous_cell_types) = state.cells.with_untracked(|cells| {
            let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
            state.cell_outputs.with_untracked(|outputs| {
                crate::components::executor::assemble_cell_inputs(cells, my_idx, outputs)
            })
        });

        // Used inside #[cfg(feature = "hydrate")] below.
        let _ = &input_bytes;

        // What this run compiles, for the completion to compare against.
        let Some(compiled) = RunInputs::current(&ctx, &cid) else {
            return;
        };

        // Mark compiling BEFORE invalidating outputs so the downstream-clear effect
        // (see below) can tell this cell's own re-run (status == Compiling) apart from
        // an upstream re-run that invalidated us (status still Idle/Success/Error).
        cell_status.set(CellStatus::Compiling);
        // This run's claim on the cell: a Stop, a newer run, or disposal
        // revokes it, and the completion below checks it.
        let guard = RunGuard::mint(run_epoch);
        emit_session_event(ironpad_common::protocol::Event::CellCompiling {
            cell_id: cid.clone(),
        });

        // Invalidate downstream Code cells' cached outputs (this cell and all after it).
        {
            let downstream_ids = state
                .cells
                .with_untracked(|cells| downstream_code_ids(cells, &cid));
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
                // Derived, never named: a constant here compiles whatever the
                // author wrote through the Code scaffold, and for a Linux cell
                // that succeeds silently (its `fn main` becomes a nested fn
                // that is never called). Gating the Run button is the other
                // half; this is the half that survives a new caller.
                cell_type,
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

            // The shared acquisition engine (PRD-0059): local blob probe
            // (PRD-0047) then server compile with transport retries. The
            // editor has no share snapshot, so that layer is `None`.
            #[cfg(feature = "hydrate")]
            let (result, served_locally, request_hash) = {
                let acquisition =
                    crate::components::run_flow::acquire_blob(&request, None, move || {
                        !guard.is_current()
                    })
                    .await;
                (
                    acquisition.result,
                    acquisition.served_without_server,
                    acquisition.request_hash,
                )
            };
            #[cfg(not(feature = "hydrate"))]
            let result = compile_cell(request).await;

            // Proceed only if this run still owns the cell. Three cases land
            // here and must drop the result: the cell/page was DISPOSED (a
            // reclaimed slot would panic on a plain read, PRD-0045), the user
            // STOPPED the compile (the compile server fn is unabortable, so
            // this is the compile-phase cancel), or stopped it and started a
            // NEWER run whose compile is still in flight. A status check
            // cannot tell the last apart: the status reads Compiling again,
            // and this old compile would run the old source.
            if !guard.is_current() {
                return;
            }
            // Edited while compiling and dispatched by the queue: this
            // result describes text the cell no longer has.
            if redispatch_if_superseded(&ctx, &compiled, &cell_id_for_exec, run_trigger) {
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
                            use crate::components::run_flow;

                            cell_status.set(CellStatus::Running);

                            // Persist server results locally (PRD-0047).
                            crate::blob_cache::store_unless_served(
                                served_locally,
                                request_hash.as_deref(),
                                &response,
                            )
                            .await;

                            last_compile.set(Some(CompileSummary::from(&response)));

                            let executed = run_flow::load_and_execute(
                                &cell_id_for_exec,
                                &response,
                                &input_bytes,
                            )
                            .await;
                            // Edited during the run. A user cancel keeps its
                            // own path below, or a Stop would re-dispatch.
                            let aborted = matches!(&executed, Err(e) if e.contains("AbortError"));
                            if !aborted
                                && redispatch_if_superseded(
                                    &ctx,
                                    &compiled,
                                    &cell_id_for_exec,
                                    run_trigger,
                                )
                            {
                                return;
                            }
                            let exec_err = match executed {
                                Ok((
                                    (output_bytes, display_text, type_tag, ran_on_main_thread),
                                    exec_ms,
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
                                            map.insert(cell_id_for_exec.clone(), dt.clone());
                                        });
                                    }

                                    emit_session_event(
                                        ironpad_common::protocol::Event::CellExecuted {
                                            cell_id: cell_id_for_exec.clone(),
                                            display_text:
                                                ironpad_common::protocol::bounded_event_display(
                                                    display_text.as_deref(),
                                                ),
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

                                    // Clear the stale flag, unless the cell
                                    // (or an upstream cell) was edited while
                                    // this ran: the output does not describe
                                    // it any more, and reactive mode re-runs
                                    // it.
                                    let edited_since = RunInputs::current(&ctx, &cell_id_for_exec)
                                        .is_some_and(|now| now != compiled);
                                    if edited_since {
                                        if state.reactive_mode.get_untracked() {
                                            state.schedule_reactive_execution();
                                        }
                                    } else {
                                        state.cell_stale.update(|stale| {
                                            stale.remove(&cell_id_for_exec);
                                        });
                                    }

                                    // Blame held by this cell is stale now.
                                    run_flow::clear_blame_held_by(
                                        state.cell_blocked_by,
                                        &cell_id_for_exec,
                                    );

                                    // Advance run-all queue on success.
                                    state.run_all_queue.advance(&cell_id_for_exec);

                                    None
                                }
                                Err(e) => Some(e),
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
                                    // Cancelling drops every queued cell; the
                                    // one report shared with a compile-phase
                                    // Stop (`cancel_run_queue`).
                                    cancel_run_queue(
                                        &state,
                                        ctx.model,
                                        ctx.session,
                                        Some(&cell_id_for_exec),
                                    );
                                } else {
                                    execution_result.set(Some(ExecutionResult {
                                        display_text: Some(err_msg),
                                        output_bytes: vec![],
                                        execution_time_ms: 0.0,
                                        type_tag: None,
                                        ran_on_main_thread: false,
                                    }));
                                    cell_status.set(CellStatus::Error);

                                    // Drop dependents; independents keep
                                    // running (PRD-0060). Drop + report is
                                    // one act (`fail_and_report`).
                                    ctx.fail_and_report(&cell_id_for_exec);
                                }
                            }
                        }

                        #[cfg(not(feature = "hydrate"))]
                        {
                            cell_status.set(CellStatus::Success);
                            last_compile.set(Some(CompileSummary::from(&response)));

                            // Clear stale flag on successful execution (SSR path).
                            state.cell_stale.update(|stale| {
                                stale.remove(&cell_id_for_exec);
                            });

                            // Blame held by this cell is stale now (SSR path).
                            crate::components::run_flow::clear_blame_held_by(
                                state.cell_blocked_by,
                                &cell_id_for_exec,
                            );

                            // Advance run-all queue (SSR path).
                            state.run_all_queue.advance(&cell_id_for_exec);
                        }
                    } else {
                        cell_status.set(CellStatus::Error);
                        last_compile.set(Some(CompileSummary::from(&response)));

                        // Drop + report is one act (`fail_and_report`).
                        ctx.fail_and_report(&cell_id_for_exec);
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
                    last_compile.set(Some(CompileSummary {
                        diagnostics,
                        blob_len: 0,
                        cached: false,
                    }));

                    // Drop + report is one act (`fail_and_report`).
                    ctx.fail_and_report(&cell_id_for_exec);
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

/// Everything the live-check policy reads, borrowed, so the policy itself
/// ([`plan_live_check`]) is a pure function with no signals in it.
pub(super) struct LiveCheckInput<'a> {
    pub(super) notebook_id: &'a str,
    pub(super) cell_id: &'a str,
    pub(super) cell_type: CellType,
    pub(super) shared: bool,
    /// Compiling or running: the run reports its own diagnostics.
    pub(super) busy: bool,
    /// The editor's live text (the model can lag it by the save debounce).
    pub(super) source: &'a str,
    pub(super) cargo_toml: &'a str,
    pub(super) notebook: &'a IronpadNotebook,
    pub(super) cells: &'a [CellManifest],
    pub(super) outputs: &'a HashMap<String, CellOutputData>,
    /// Manifests that compiled cleanly this session (`warm_manifest_key`).
    pub(super) warm: &'a HashSet<String>,
}

/// The cell body sent with a shared-cell check: the real code under test
/// rides in `shared_source`, but the scaffold still needs a valid cell.
const SHARED_CHECK_BODY: &str = "String::new()";

/// The live-check policy (PRD-0045): the request to send for this cell, or
/// `None` when it should not be checked.
///
/// Eligible means: a cell that compiles, not busy, warm by the manifest
/// policy (default deps are pre-checked by the image warmup; custom deps must
/// have compiled once this session), and not reading a piped slot that has no
/// type yet. The scaffold binds a slot only when its OWN type is known, so a
/// reference to an unrun slot is a false "cannot find value `cellN`" squiggle
/// on code that runs fine (Run cascades the slot's cell first; a check
/// cannot). An out-of-range slot is a real error and stays checked, and a
/// raw-buffer reference is always bound.
///
/// Shared cells (PRD-0046) check the assembled `shared.rs` instead: the
/// request carries a throwaway body plus the cell's line slice, and the
/// server returns only diagnostics anchored inside that slice, remapped to
/// cell-local lines.
fn plan_live_check(input: &LiveCheckInput<'_>) -> Option<CompileRequest> {
    use ironpad_common::{manifest_has_custom_deps, SharedCheckRange};

    let &LiveCheckInput {
        cell_type, shared, ..
    } = input;

    // Nothing to check without code. `Unsupported` is included: this build
    // cannot know what that cell meant, so it must not compile it to guess.
    if !cell_type.compiles() || input.busy {
        return None;
    }

    let shared_cargo = input.notebook.shared_cargo_toml.as_deref();
    // A shared cell's own manifest never reaches the build (consumers merge
    // THEIR manifest with the shared one), so its check compiles under the
    // default cell manifest.
    let cargo_toml = if shared {
        ironpad_common::DEFAULT_CELL_CARGO_TOML
    } else {
        input.cargo_toml
    };
    if manifest_has_custom_deps(shared_cargo, cargo_toml)
        && !input
            .warm
            .contains(&warm_manifest_key(shared_cargo, cargo_toml))
    {
        return None;
    }

    let (shared_check, previous_cell_types) = if shared {
        // This cell's slice inside the assembly, from the same model state
        // the shared source below is assembled from. Not (yet) in it (the
        // shared toggle is still in flight): no slice, so try again next
        // debounce. Shared cells hold empty piping slots and the check body
        // references nothing.
        let start_line = ironpad_common::shared_cell_line_offset(
            input.notebook.shared_source.as_deref(),
            &input.notebook.cells,
            input.cell_id,
        )?;
        let cell = input
            .notebook
            .cells
            .iter()
            .find(|c| c.id == input.cell_id)?;
        let line_count = u32::try_from(cell.source.split('\n').count()).ok()?;
        (
            Some(SharedCheckRange {
                start_line,
                line_count,
            }),
            Vec::new(),
        )
    } else {
        // THE shared projection: the same vector the compile path scaffolds
        // and cache-keys from (`assemble_cell_inputs`), so a check never
        // forks its scaffold or cache key from the eventual compile.
        let my_idx = input
            .cells
            .iter()
            .position(|c| c.id == input.cell_id)
            .unwrap_or(0);
        let types =
            crate::components::executor::previous_cell_types(input.cells, my_idx, input.outputs);
        // The shared detection recipe (PRD-0060), slot by slot.
        let refs = ironpad_common::cell_deps::referenced_slots(input.source);
        let unbound_slot = refs
            .slots
            .iter()
            .any(|&i| types.get(i).is_some_and(String::is_empty));
        if unbound_slot || (refs.last && types.iter().all(String::is_empty)) {
            return None;
        }
        (None, types)
    };

    Some(CompileRequest {
        // A shared cell's check compiles SHARED_CHECK_BODY through the Code
        // scaffold regardless of the cell's own type, so it reports Code; any
        // other cell reports what it is, and the server derives the target.
        cell_type: if shared { CellType::Code } else { cell_type },
        notebook_id: input.notebook_id.to_string(),
        cell_id: input.cell_id.to_string(),
        source: if shared {
            SHARED_CHECK_BODY
        } else {
            input.source
        }
        .to_string(),
        cargo_toml: cargo_toml.to_string(),
        previous_cell_types,
        shared_cargo_toml: shared_cargo.map(str::to_string),
        shared_source: input.notebook.effective_shared_source(),
        force: false,
        shared_check,
    })
}

/// What a live-check dispatch touches: the cell's run context plus the
/// check's own two signals.
#[cfg(feature = "hydrate")]
#[derive(Clone, Copy)]
pub(super) struct LiveCheckCtx {
    pub(super) run: CellRunCtx,
    pub(super) last_check: RwSignal<Option<Vec<Diagnostic>>>,
    pub(super) check_generation: RwSignal<u64>,
}

/// Fire a live check for the cell if [`plan_live_check`] says it is
/// eligible, discarding the response unless it is still the newest dispatch
/// (generation match).
#[cfg(feature = "hydrate")]
pub(super) fn dispatch_live_check(ctx: &LiveCheckCtx) {
    dispatch_live_check_with_retries(ctx, CHECK_SKIP_RETRIES);
}

/// Retries after a `Skipped` check (the pool was busy). Each retry re-reads
/// the CURRENT source and re-runs eligibility, so it never checks stale
/// text; the generation guard makes a superseding edit's own dispatch win.
#[cfg(feature = "hydrate")]
const CHECK_SKIP_RETRIES: u8 = 5;

/// Delay before a skip retry: long enough for a build slot to free, short
/// enough that the squiggle still feels live.
#[cfg(feature = "hydrate")]
const CHECK_SKIP_RETRY_MS: i32 = 3_000;

#[cfg(feature = "hydrate")]
fn dispatch_live_check_with_retries(ctx: &LiveCheckCtx, retries_left: u8) {
    use ironpad_common::CheckStatus;

    let LiveCheckCtx {
        run,
        last_check,
        check_generation,
    } = *ctx;
    let state = run.state;
    let cid = run.cell_id.get_value();
    let source = run.source.get_untracked();
    let cargo_toml = run.cargo_toml.get_untracked();
    let notebook_id = state.notebook_id.get_untracked();

    // Borrowed: this runs on every 1s typing debounce, and cloning the cells
    // and every output's bytes to read type tags was the recurring cost here.
    let request = state.notebook.with_untracked(|nb| {
        let notebook = nb.as_ref()?;
        state.cells.with_untracked(|cells| {
            state.cell_outputs.with_untracked(|outputs| {
                state.warm_manifests.with_untracked(|warm| {
                    plan_live_check(&LiveCheckInput {
                        notebook_id: &notebook_id,
                        cell_id: &cid,
                        cell_type: run.cell_type,
                        shared: run.is_shared.get_untracked(),
                        busy: matches!(
                            run.cell_status.get_untracked(),
                            CellStatus::Compiling | CellStatus::Running
                        ),
                        source: &source,
                        cargo_toml: &cargo_toml,
                        notebook,
                        cells,
                        outputs,
                        warm,
                    })
                })
            })
        })
    });
    let Some(request) = request else {
        return;
    };

    let generation = check_generation.get_untracked() + 1;
    check_generation.set(generation);

    // Copied for the skip-retry redispatch: leptos context is not reliably
    // reachable from a detached task, and the context is `Copy` anyway.
    let retry_ctx = *ctx;

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
            // Busy: the pool had no free slot THIS round. Without a retry
            // the squiggle never appears even though the server frees up
            // seconds later (the live-check e2e caught this under suite
            // concurrency). Bounded, and the redispatch re-reads current
            // state, so it degrades to a plain skip under sustained load.
            CheckStatus::Skipped => {
                if retries_left > 0 {
                    crate::components::run_flow::sleep_ms(CHECK_SKIP_RETRY_MS).await;
                    // Disposed while waiting: nothing to check.
                    if check_generation.try_get_untracked().is_none() {
                        return;
                    }
                    dispatch_live_check_with_retries(&retry_ctx, retries_left - 1);
                }
            }
            // Cold: the server spent its whole time budget; a quick retry
            // would spend another for the same answer.
            CheckStatus::TimedOut => {}
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
    fn queue_drops_on_cancel_name_the_cancelled_cell_once() {
        // Review soc-app-client-2: a Stop reports the stopped cell itself
        // (an agent waiting on it may never have queued it, a direct run
        // never does) and every queued cell the Stop dropped.
        let drops = |v: &[&str]| v.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            queue_drops_on_cancel(Some("a"), drops(&["b", "c"])),
            drops(&["a", "b", "c"])
        );
        assert_eq!(queue_drops_on_cancel(Some("a"), Vec::new()), drops(&["a"]));
        // Leaving the editor names whatever was queued, front included.
        assert_eq!(
            queue_drops_on_cancel(None, drops(&["a", "b"])),
            drops(&["a", "b"])
        );
        assert!(queue_drops_on_cancel(None, Vec::new()).is_empty());
    }

    // ── Superseded runs (review bug-common-cli-6) ───────────────────────

    fn inputs(source: &str) -> RunInputs {
        RunInputs {
            source: source.to_string(),
            cargo_toml: "[dependencies]".to_string(),
            shared_cargo_toml: None,
            shared_source: None,
            upstream: vec!["1 + 1".to_string()],
        }
    }

    #[test]
    fn an_edit_to_what_was_compiled_supersedes_the_run() {
        let compiled = inputs("old");
        assert!(!compiled.recompile_needed(&inputs("old")));
        assert!(compiled.recompile_needed(&inputs("new")), "own source");

        let mut manifest = inputs("old");
        manifest.cargo_toml = "[dependencies]\nrand = \"0.8\"".to_string();
        assert!(compiled.recompile_needed(&manifest), "own manifest");

        let mut shared = inputs("old");
        shared.shared_source = Some("fn helper() {}".to_string());
        assert!(
            compiled.recompile_needed(&shared),
            "shared source is compiled in"
        );

        let mut shared_manifest = inputs("old");
        shared_manifest.shared_cargo_toml = Some("[dependencies]".to_string());
        assert!(compiled.recompile_needed(&shared_manifest));
    }

    #[test]
    fn an_upstream_edit_keeps_the_cell_stale_without_a_recompile() {
        // The upstream cell's TEXT is not compiled into this one (its output
        // is, and that did not change), so the result stands; but the edit
        // marked this cell stale after its inputs were taken, and the run
        // must not wipe that.
        let compiled = inputs("x");
        let mut now = inputs("x");
        now.upstream = vec!["2 + 2".to_string()];
        assert!(!compiled.recompile_needed(&now));
        assert!(compiled != now);
    }

    #[test]
    fn upstream_sources_are_the_runnable_cells_before_this_one() {
        let mut shared = manifest("shared", CellType::Code);
        shared.shared = true;
        let cells = vec![
            manifest("a", CellType::Code),
            manifest("md", CellType::Markdown),
            shared,
            manifest("me", CellType::Code),
            manifest("after", CellType::Code),
        ];
        let sources: std::collections::HashMap<String, String> = [
            ("a", "1"),
            ("md", "# hi"),
            ("shared", "fn f() {}"),
            ("me", "cell0"),
            ("after", "2"),
        ]
        .into_iter()
        .map(|(id, src)| (id.to_string(), src.to_string()))
        .collect();
        assert_eq!(
            upstream_runnable_sources(&cells, "me", &sources),
            vec!["1".to_string()]
        );
    }

    #[test]
    fn compile_summary_keeps_what_the_panels_read_and_drops_the_blob() {
        let response = CompileResponse {
            wasm_blob: vec![0; 1234],
            diagnostics: vec![Diagnostic {
                message: "unused variable".to_string(),
                severity: Severity::Warning,
                spans: vec![],
                code: Some("unused_variables".to_string()),
            }],
            cached: true,
            preamble_lines: 7,
            js_glue: Some("glue".to_string()),
        };
        let summary = CompileSummary::from(&response);
        assert_eq!(summary.blob_len, response.wasm_blob.len());
        assert!(summary.cached);
        assert_eq!(summary.diagnostics.len(), 1);
        assert_eq!(summary.diagnostics[0].message, "unused variable");
        assert_eq!(summary.diagnostics[0].severity, Severity::Warning);
        assert_eq!(
            summary.diagnostics[0].code.as_deref(),
            Some("unused_variables")
        );
    }

    // ── Live-check eligibility (PRD-0045, review soc-app-client-4) ──────

    /// A notebook through serde, the way storage hands one over, so the
    /// fixture names no field it does not care about.
    fn notebook(cells: &[(&str, &str, bool)], shared_source: Option<&str>) -> IronpadNotebook {
        let cells: Vec<serde_json::Value> = cells
            .iter()
            .enumerate()
            .map(|(order, (id, source, shared))| {
                serde_json::json!({
                    "id": id, "order": order, "label": id,
                    "cell_type": "Code", "source": source, "shared": shared,
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "id": "00000000-0000-0000-0000-000000000000",
            "title": "live check",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "shared_source": shared_source,
            "cells": cells,
        }))
        .expect("fixture notebook")
    }

    fn typed(ids: &[&str]) -> HashMap<String, CellOutputData> {
        ids.iter()
            .map(|id| {
                (
                    (*id).to_string(),
                    CellOutputData {
                        bytes: vec![0; 4],
                        type_tag: Some("u32".to_string()),
                    },
                )
            })
            .collect()
    }

    /// Plan a check of `cell_id` with `source` in a three-cell notebook
    /// `a`, `b`, `c` whose cells `ran` have typed outputs.
    fn plan(cell_id: &str, source: &str, ran: &[&str]) -> Option<CompileRequest> {
        let nb = notebook(
            &[
                ("a", "1u32", false),
                ("b", "cell0 + 1", false),
                ("c", "2u32", false),
            ],
            None,
        );
        let cells: Vec<CellManifest> = nb.cells.iter().map(manifest_of).collect();
        let outputs = typed(ran);
        let warm = HashSet::new();
        plan_live_check(&LiveCheckInput {
            notebook_id: "nb",
            cell_id,
            cell_type: CellType::Code,
            shared: false,
            busy: false,
            source,
            cargo_toml: "[dependencies]",
            notebook: &nb,
            cells: &cells,
            outputs: &outputs,
            warm: &warm,
        })
    }

    fn manifest_of(cell: &ironpad_common::IronpadCell) -> CellManifest {
        let mut m = manifest(&cell.id, cell.cell_type);
        m.shared = cell.shared;
        m
    }

    #[test]
    fn skips_when_a_referenced_slot_is_unrun() {
        // The regression: run only `a`, then type `cell1 + 1` in `c`. The
        // types are ["u32", ""], not all empty, so the old gate checked, and
        // the scaffold (which binds a slot only when its own type is known)
        // emitted no `cell1`: a false E0425 squiggle on code that runs fine.
        assert!(plan("c", "cell1 + 1", &["a"]).is_none());
    }

    #[test]
    fn checks_when_every_referenced_slot_is_typed() {
        let request = plan("c", "cell1 + 1", &["a", "b"]).expect("eligible");
        assert_eq!(
            request.previous_cell_types,
            vec!["u32".to_string(), "u32".to_string()]
        );
        assert_eq!(request.source, "cell1 + 1");
        assert!(request.shared_check.is_none());
        assert!(!request.force);
    }

    #[test]
    fn skips_last_with_nothing_typed() {
        assert!(plan("c", "last + 1", &[]).is_none());
        assert!(
            plan("c", "last + 1", &["a"]).is_some(),
            "the alias has a target"
        );
    }

    #[test]
    fn checks_an_out_of_range_slot() {
        // `cell5` in the second cell is a real error the author should see.
        assert!(plan("b", "cell5 + 1", &["a"]).is_some());
    }

    #[test]
    fn checks_a_raw_only_reference() {
        // The scaffold binds the raw buffer whenever any slot exists.
        assert!(plan("c", "__ironpad_inputs__.len()", &[]).is_some());
    }

    #[test]
    fn checks_a_cell_that_references_nothing() {
        assert!(plan("c", "2u32 + 2", &[]).is_some());
    }

    #[test]
    fn cold_custom_deps_skip_and_warm_ones_check() {
        let nb = notebook(&[("a", "1u32", false)], None);
        let cells: Vec<CellManifest> = nb.cells.iter().map(manifest_of).collect();
        let outputs = HashMap::new();
        let cargo = "[dependencies]\nrand = \"0.8\"";
        let mut warm = HashSet::new();
        let input = |warm: &HashSet<String>| {
            plan_live_check(&LiveCheckInput {
                notebook_id: "nb",
                cell_id: "a",
                cell_type: CellType::Code,
                shared: false,
                busy: false,
                source: "1u32",
                cargo_toml: cargo,
                notebook: &nb,
                cells: &cells,
                outputs: &outputs,
                warm,
            })
        };
        assert!(
            input(&warm).is_none(),
            "custom deps never built this session"
        );
        warm.insert(warm_manifest_key(None, cargo));
        assert!(input(&warm).is_some());
    }

    #[test]
    fn shared_cell_sends_the_stub_body_as_code_with_its_line_range() {
        let nb = notebook(
            &[
                ("helpers", "fn double(x: u32) -> u32 {\n    x * 2\n}", true),
                ("a", "1u32", false),
            ],
            Some("const K: u32 = 1;"),
        );
        let cells: Vec<CellManifest> = nb.cells.iter().map(manifest_of).collect();
        let outputs = HashMap::new();
        let warm = HashSet::new();
        let request = plan_live_check(&LiveCheckInput {
            notebook_id: "nb",
            cell_id: "helpers",
            cell_type: CellType::Code,
            shared: true,
            busy: false,
            source: "fn double(x: u32) -> u32 {\n    x * 2\n}",
            // A shared cell's own manifest never reaches the build.
            cargo_toml: "[dependencies]\nrand = \"0.8\"",
            notebook: &nb,
            cells: &cells,
            outputs: &outputs,
            warm: &warm,
        })
        .expect("a shared cell checks the assembly");
        assert_eq!(request.cell_type, CellType::Code);
        assert_eq!(request.source, "String::new()");
        assert_eq!(request.cargo_toml, ironpad_common::DEFAULT_CELL_CARGO_TOML);
        assert!(request.previous_cell_types.is_empty());
        let range = request.shared_check.expect("the cell's slice");
        assert_eq!(
            range.start_line, 2,
            "after the notebook-level source and its separator"
        );
        assert_eq!(range.line_count, 3);
        assert_eq!(request.shared_source, nb.effective_shared_source());
    }

    #[test]
    fn shared_cell_missing_from_the_assembly_skips() {
        // The shared toggle is still in flight: the model does not yet hold
        // the cell as shared, so there is no slice to check against.
        let nb = notebook(&[("helpers", "fn f() {}", false)], None);
        let cells: Vec<CellManifest> = nb.cells.iter().map(manifest_of).collect();
        let outputs = HashMap::new();
        let warm = HashSet::new();
        assert!(plan_live_check(&LiveCheckInput {
            notebook_id: "nb",
            cell_id: "helpers",
            cell_type: CellType::Code,
            shared: true,
            busy: false,
            source: "fn f() {}",
            cargo_toml: "[dependencies]",
            notebook: &nb,
            cells: &cells,
            outputs: &outputs,
            warm: &warm,
        })
        .is_none());
    }

    #[test]
    fn busy_cell_skips() {
        let nb = notebook(&[("a", "1u32", false)], None);
        let cells: Vec<CellManifest> = nb.cells.iter().map(manifest_of).collect();
        let outputs = HashMap::new();
        let warm = HashSet::new();
        let input = |busy, cell_type| LiveCheckInput {
            notebook_id: "nb",
            cell_id: "a",
            cell_type,
            shared: false,
            busy,
            source: "1u32",
            cargo_toml: "[dependencies]",
            notebook: &nb,
            cells: &cells,
            outputs: &outputs,
            warm: &warm,
        };
        assert!(plan_live_check(&input(true, CellType::Code)).is_none());
        assert!(plan_live_check(&input(false, CellType::Markdown)).is_none());
        assert!(plan_live_check(&input(false, CellType::Code)).is_some());
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
