//! The shared cell-run engine: blob acquisition through execution.
//!
//! Extracted from the editor's `pipeline.rs` and the viewer's
//! `ViewOnlyCodeCell` (PRD-0059): the two surfaces had grown divergent
//! copies of the same async pipeline — the viewer retried compile transport
//! errors and the editor didn't. This module owns the one policy:
//!
//! 1. share snapshot (viewers of `/shared` with a blob manifest, PRD-0047)
//! 2. local blob probe (`blob_cache::probe_local`, both surfaces)
//! 3. server compile with transport retries (idempotent, cache-keyed)
//!
//! …then `load_and_execute` for the WASM half. Surfaces keep their own
//! signals, status models, session events, and (until PRD-0060) failure
//! policies; the engine is the part that must never fork.

#[cfg(feature = "hydrate")]
use ironpad_common::{CompileRequest, CompileResponse};
use leptos::prelude::*;

// ── Transport retry policy ──────────────────────────────────────────────────

/// Compile transport retries beyond the first attempt. Compile requests are
/// idempotent (cache-keyed server-side), so TRANSPORT failures — a dropped
/// connection mid-build, a network change on a mobile client — are safe to
/// retry; the server's incremental target dir makes retried builds much
/// cheaper than the first attempt. Compile diagnostics are Ok responses and
/// never retried, and neither is anything [`is_transport_error`] does not
/// name.
#[cfg(feature = "hydrate")]
const COMPILE_TRANSPORT_RETRIES: i32 = 2;

/// Is `e` a failure of the TRANSPORT rather than a verdict from the server?
///
/// An allowlist, not "anything but `ServerError`" (review bug-compiler-6):
/// every refusal `compile_cell` issues (at capacity, rate limited, timed out,
/// an invalid cell id) arrives as `ServerError`, and re-asking ran each one
/// up to three times, charging a build token and queueing up to 180s per
/// attempt. `Args` and `MissingArg` are verdicts too. A gateway 502 with an
/// HTML body decodes as `Deserialization`, so it still retries.
///
/// A build that exceeds its budget is NOT retried either: its artifacts
/// persist, so the author's next Run converges, and they hear about it after
/// one budget rather than three.
#[cfg(any(test, feature = "hydrate"))]
fn is_transport_error(e: &ServerFnError) -> bool {
    matches!(
        e,
        ServerFnError::Request(_) | ServerFnError::Response(_) | ServerFnError::Deserialization(_)
    )
}

/// Base backoff (ms) between transport retries, scaled by attempt number.
#[cfg(feature = "hydrate")]
const COMPILE_TRANSPORT_BACKOFF_MS: i32 = 1_500;

/// Await `ms` milliseconds via a `setTimeout`-backed promise — THE async
/// sleeper for this crate (there's no async-timer dependency in the
/// workspace). Retry backoffs, effect-queue yields, and autosave polls all
/// delegate here; a mechanism change (timer future, clamping, the no-window
/// arm) lands once.
#[cfg(feature = "hydrate")]
pub(crate) async fn sleep_ms(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(window) = leptos::web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        } else {
            // No window (shouldn't happen in hydrate) — resolve immediately.
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

// ── Blob acquisition ────────────────────────────────────────────────────────

/// The outcome of [`acquire_blob`]: the compile result plus what the
/// store-back policy needs (`store_unless_served` stays a caller call so the
/// editor's store-before-load ordering is preserved).
#[cfg(feature = "hydrate")]
pub struct BlobAcquisition {
    pub result: Result<CompileResponse, ServerFnError>,
    /// Served by the share snapshot or the local store — the store-back
    /// half skips these (nothing new to persist).
    pub served_without_server: bool,
    /// Content hash for the local store, when the fingerprint was available.
    pub request_hash: Option<String>,
}

/// Acquire a compiled blob for `request` through every cache layer in order:
/// share snapshot (when the caller has a manifest entry and the request is
/// not forced), local `IndexedDB` store, then the server — with transport
/// retries. Fresh mode (`request.force`) bypasses snapshot and local store
/// (the probe self-gates) so the server really recompiles.
///
/// `is_cancelled` says whether the caller still wants the result (a Stop, a
/// newer run, or a disposed cell). A transport failure is not retried once it
/// does: a reader who navigated away would otherwise keep paying for
/// compiles of a page that is gone.
#[cfg(feature = "hydrate")]
pub async fn acquire_blob(
    request: &CompileRequest,
    share_blob: Option<&ironpad_common::ShareBlobEntry>,
    is_cancelled: impl Fn() -> bool,
) -> BlobAcquisition {
    // Snapshotted share blob (PRD-0047): in cache mode, a manifest entry
    // replaces the compile round trip with an immutable static GET. Any
    // fetch failure falls back to the live pipeline below.
    let mut snapshot_response: Option<CompileResponse> = None;
    if !request.force {
        if let Some(entry) = share_blob {
            match crate::components::executor::fetch_share_blob(entry).await {
                Ok((wasm_blob, js_glue)) => {
                    snapshot_response = Some(CompileResponse {
                        wasm_blob,
                        diagnostics: vec![],
                        cached: true,
                        preamble_lines: 0,
                        js_glue,
                    });
                }
                Err(e) => {
                    leptos::logging::warn!(
                        "share blob fetch failed ({e}); falling back to compile"
                    );
                }
            }
        }
    }

    // Local blob store (PRD-0047): skipped entirely when the share snapshot
    // already served the blob, which keeps a snapshot replay free of even
    // the fingerprint round trip.
    let request_hash = if snapshot_response.is_some() {
        None
    } else {
        let (hash, local_hit) = crate::blob_cache::probe_local(request).await;
        snapshot_response = local_hit;
        hash
    };
    let served_without_server = snapshot_response.is_some();

    let result = if let Some(response) = snapshot_response {
        Ok(response)
    } else {
        let mut attempt: i32 = 0;
        loop {
            match crate::server_fns::compile_cell(request.clone()).await {
                Err(e)
                    if attempt < COMPILE_TRANSPORT_RETRIES
                        && is_transport_error(&e)
                        && !is_cancelled() =>
                {
                    attempt += 1;
                    leptos::logging::warn!(
                        "compile transport error (attempt {attempt}): {e}; retrying"
                    );
                    sleep_ms(COMPILE_TRANSPORT_BACKOFF_MS * attempt).await;
                }
                other => break other,
            }
        }
    };

    BlobAcquisition {
        result,
        served_without_server,
        request_hash,
    }
}

// ── Load + execute ──────────────────────────────────────────────────────────

/// Load the compiled blob into the executor and run it with the assembled
/// piped inputs. Returns the execution tuple plus wall-clock execution time,
/// or the pre-formatted error message both surfaces render (`WASM load
/// error: …` / `Execution error: …` — the latter carries `AbortError` when
/// the user cancelled via `terminate()`).
#[cfg(feature = "hydrate")]
pub async fn load_and_execute(
    cell_id: &str,
    response: &CompileResponse,
    input_bytes: &[u8],
) -> Result<(crate::components::executor::CellExecResult, f64), String> {
    use crate::components::executor;

    let hash = executor::hash_wasm_blob(&response.wasm_blob);
    executor::load_blob(
        cell_id,
        &hash,
        &response.wasm_blob,
        response.js_glue.clone(),
    )
    .await
    .map_err(|e| format!("WASM load error: {e}"))?;

    let exec_start = js_sys::Date::now();
    let result = executor::execute_cell(cell_id, input_bytes)
        .await
        .map_err(|e| format!("Execution error: {e}"))?;
    Ok((result, js_sys::Date::now() - exec_start))
}

// ── Run ownership ───────────────────────────────────────────────────────────

/// A run's claim on its cell, as an epoch that a Stop and the next run both
/// invalidate.
///
/// A Stop during the compile has no other lever: the compile server fn
/// cannot be aborted (and for a Linux cell the pod runtime is not even loaded
/// yet), so the click can only be honoured by the spawned task abandoning its
/// result when it lands. What makes this an epoch rather than a flag or a
/// status check is the race those lose: Stop, edit, then Run again while the
/// first compile is still in flight. A flag is cleared by the second click and
/// the status reads `Compiling` again, so the FIRST compile would pass the
/// gate, run the old source, and the second's result would be the one
/// dropped. The editor's `run_trigger` cannot serve either: Ctrl+Enter bumps
/// it mid-compile.
#[derive(Clone, Copy)]
pub(crate) struct RunGuard {
    epoch: u64,
    current: RwSignal<u64>,
}

impl RunGuard {
    /// Claim the cell for a new run, invalidating every earlier claim.
    pub(crate) fn mint(current: RwSignal<u64>) -> Self {
        let epoch = current.try_get_untracked().unwrap_or(0).wrapping_add(1);
        current.set(epoch);
        Self { epoch, current }
    }

    /// Invalidate whatever run holds the cell (a Stop).
    pub(crate) fn revoke(current: RwSignal<u64>) {
        current.try_update(|epoch| *epoch = epoch.wrapping_add(1));
    }

    /// Whether this run still owns its cell: no Stop, no newer run, and the
    /// component not disposed.
    pub(crate) fn is_current(self) -> bool {
        Self::is_current_at(self.current.try_get_untracked(), self.epoch)
    }

    /// The decision itself, separated from the signal so the invariant is
    /// testable: a bumped epoch means a Stop or a newer run, and `None` means
    /// the component was disposed mid-run (navigating away is the ordinary
    /// way to get there). Both must refuse.
    pub(crate) fn is_current_at(current: Option<u64>, epoch: u64) -> bool {
        current == Some(epoch)
    }
}

// ── The run queue ───────────────────────────────────────────────────────────

/// The ordered queue of cells waiting to run, front first: the front is the
/// cell running now, and it stays PINNED until it finishes (one cell runs at a
/// time). Every cell that leaves the queue other than by finishing is reported
/// by the caller (`CellBlocked` for a dependency failure, `RunCancelled` for a
/// cancel).
///
/// A newtype with a private field so that policy has ONE owner. The queue was
/// a raw signal that a dozen sites wrote under four policies (merge, replace,
/// set-if-empty, append, clear), and the replacing ones broke both rules at
/// once: Run All Below or a button widget during a run dropped the queued
/// cells unreported (an agent waiting on one waited out its timeout) and
/// unpinned the mid-compile front, so the next cell compiled concurrently. An
/// enqueue now always MERGES, so nothing is dropped and nothing needs
/// reporting; the only ways out are [`Self::advance`], [`Self::remove`],
/// [`Self::fail`] and [`Self::cancel`], and the last two hand back what they
/// dropped.
///
/// Every read is a `try_` read: detached completion tasks outlive their page,
/// and reading a disposed signal panics where a write only no-ops.
#[derive(Clone, Copy)]
pub(crate) struct RunQueue(RwSignal<Vec<String>>);

impl RunQueue {
    pub(crate) fn new() -> Self {
        Self(RwSignal::new(Vec::new()))
    }

    /// Tracked: `id`'s place in the queue (`Some(0)` is the running front),
    /// `None` when it is not queued.
    pub(crate) fn position(&self, id: &str) -> Option<usize> {
        self.0
            .try_with(|queue| queue.iter().position(|queued| queued == id))
            .flatten()
    }

    /// Tracked: whether nothing is queued. A disposed queue reads idle.
    pub(crate) fn is_idle(&self) -> bool {
        self.0.try_with(Vec::is_empty).unwrap_or(true)
    }

    /// Untracked: whether `id` is the front, the cell the queue is running.
    pub(crate) fn is_front_untracked(&self, id: &str) -> bool {
        self.0
            .try_with_untracked(|queue| queue.first().is_some_and(|front| front == id))
            .unwrap_or(false)
    }

    /// Untracked [`Self::is_idle`].
    pub(crate) fn is_empty_untracked(&self) -> bool {
        self.0.try_with_untracked(Vec::is_empty).unwrap_or(true)
    }

    /// Untracked copy of the queue, for the failure policies that compute
    /// dependents against it. Empty when disposed.
    pub(crate) fn snapshot_untracked(&self) -> Vec<String> {
        self.0.try_get_untracked().unwrap_or_default()
    }

    /// Route a single-cell run of `target` (with its unexecuted
    /// `dependencies`) through the queue when it has to wait: when there are
    /// dependencies to run first, or another cell holds the queue. Returns
    /// `true` when the request was merged in and the queue will dispatch it,
    /// `false` when the caller should run `target` directly (nothing to
    /// cascade, and the queue is idle or already has `target` at the front).
    pub(crate) fn request(&self, order: &[&str], dependencies: &[String], target: &str) -> bool {
        let Some(next) = self.0.try_with_untracked(|queue| {
            let front_is_target = queue.first().is_some_and(|id| id == target);
            let must_wait = !dependencies.is_empty() || (!queue.is_empty() && !front_is_target);
            must_wait.then(|| {
                let mut additions = dependencies.to_vec();
                additions.push(target.to_string());
                merged(order, queue, &additions, Some(target))
            })
        }) else {
            // Disposed: nothing is left to run it.
            return true;
        };
        match next {
            Some(queue) => {
                self.0.set(queue);
                true
            }
            None => false,
        }
    }

    /// Queue `ids` (Run All, Run All Below, a button widget). Merges into a
    /// running queue rather than replacing it: the front stays pinned and
    /// every queued cell keeps its place in notebook order.
    pub(crate) fn enqueue_many(&self, order: &[&str], ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        if let Some(next) = self
            .0
            .try_with_untracked(|queue| merged(order, queue, ids, None))
        {
            self.0.set(next);
        }
    }

    /// Start a queue of `ids` only when none is running (the viewer's Run and
    /// Run All, autorun, the reactive timer). Returns whether it started.
    pub(crate) fn set_if_idle(&self, ids: Vec<String>) -> bool {
        if ids.is_empty() || !self.is_empty_untracked() {
            return false;
        }
        self.0.set(ids);
        true
    }

    /// Append `id` unless it is already queued (an agent's `cells run`). Its
    /// dependencies cascade when it reaches the front.
    pub(crate) fn push_unique(&self, id: &str) {
        self.0.try_update(|queue| {
            if !queue.iter().any(|queued| queued == id) {
                queue.push(id.to_string());
            }
        });
    }

    /// `id` finished: pop it if it is the front. A no-op when another cell
    /// holds the front (a stale completion must not eat its turn).
    pub(crate) fn advance(&self, id: &str) {
        self.0.try_update(|queue| {
            if queue.first().is_some_and(|front| front == id) {
                queue.remove(0);
            }
        });
    }

    /// Drop `id` wherever it is (a deleted cell).
    pub(crate) fn remove(&self, id: &str) {
        self.0
            .try_update(|queue| queue.retain(|queued| queued != id));
    }

    /// Continue-past-failures (PRD-0060): drop `failed_id` and its
    /// `dependents`, recording blame for each dependent. The two signals are
    /// distinct arena slots, so the nested update is safe; blame lands before
    /// the queue's subscribers run, so a watcher never sees a dropped cell
    /// without its reason.
    pub(crate) fn fail(
        &self,
        blocked_by: RwSignal<std::collections::HashMap<String, String>>,
        failed_id: &str,
        dependents: &[String],
    ) {
        self.0.try_update(|queue| {
            blocked_by.try_update(|blocked| {
                fail_in_queue_core(queue, blocked, failed_id, dependents);
            });
        });
    }

    /// Empty the queue (a Stop, or leaving the surface that drains it) and
    /// return every dropped cell except `except`, for the caller to report.
    pub(crate) fn cancel(&self, except: Option<&str>) -> Vec<String> {
        let Some(dropped) = self
            .0
            .try_with_untracked(|queue| (!queue.is_empty()).then(|| without(queue, except)))
        else {
            return Vec::new();
        };
        match dropped {
            Some(dropped) => {
                self.0.set(Vec::new());
                dropped
            }
            None => Vec::new(),
        }
    }
}

/// The one merge every enqueue goes through. The front is pinned unless it
/// is `unpinned` or no longer in `order`: a request's own target at the front
/// has not started yet (the dispatch guard returns before that) and its
/// dependencies must run first, and a front deleted mid-run is a ghost no
/// watcher can advance. Everything else wanted (the queue's other entries
/// plus `additions`) follows in notebook order, which runs a dependency
/// before its consumer because a dependency is always upstream. Ids not in
/// `order` are dropped.
fn merged(
    order: &[&str],
    queue: &[String],
    additions: &[String],
    unpinned: Option<&str>,
) -> Vec<String> {
    let mut wanted: std::collections::HashSet<&str> = queue.iter().map(String::as_str).collect();
    wanted.extend(additions.iter().map(String::as_str));

    let mut merged: Vec<String> = Vec::with_capacity(wanted.len());
    if let Some(front) = queue.first() {
        if Some(front.as_str()) != unpinned && order.contains(&front.as_str()) {
            wanted.remove(front.as_str());
            merged.push(front.clone());
        }
    }
    merged.extend(
        order
            .iter()
            .filter(|id| wanted.contains(*id))
            .map(|id| (*id).to_string()),
    );
    merged
}

/// `queue` without `except`: what a cancel drops.
fn without(queue: &[String], except: Option<&str>) -> Vec<String> {
    queue
        .iter()
        .filter(|id| Some(id.as_str()) != except)
        .cloned()
        .collect()
}

/// Continue-past-failures core (PRD-0060): record blame for `dependents`
/// (blocked-by the failed cell) and drop them, plus `failed_id` itself, from
/// the queue. Independent queued cells survive. ONE policy for the editor
/// and the viewer — the two surfaces once hand-rolled diverging copies, and
/// the divergence shipped a sticky-blocked bug.
pub fn fail_in_queue_core(
    queue: &mut Vec<String>,
    blocked: &mut std::collections::HashMap<String, String>,
    failed_id: &str,
    dependents: &[String],
) {
    for dep in dependents {
        blocked.insert(dep.clone(), failed_id.to_string());
    }
    queue.retain(|id| id != failed_id && !dependents.iter().any(|d| d == id));
}

/// A dispatched run of `cell_id` is a fresh attempt: any blocked notice it
/// carries is stale the moment it retries. Without this, a blocked cell
/// edited to drop the failing dependency would run to success and then snap
/// back to "blocked" — the surviving map entry outvoted the fresh result.
///
/// Disposal-safe by construction: callers include detached completion tasks
/// that can outlive their page, and a READ on a disposed arena signal
/// panics (writes no-op) — so the has-anything-to-do guard uses the `try`
/// read and treats "disposed" as "nothing to do".
pub fn clear_own_blame(
    blocked_by: RwSignal<std::collections::HashMap<String, String>>,
    cell_id: &str,
) {
    if blocked_by
        .try_with_untracked(|b| b.contains_key(cell_id))
        .unwrap_or(false)
    {
        blocked_by.update(|b| {
            b.remove(cell_id);
        });
    }
}

/// `cell_id` produced a fresh successful result: blame entries naming it as
/// the blocker are stale. Guarded so a run that blocked nothing does not
/// notify every blocked-by watcher for no reason; the guard is a `try` read
/// for the same disposal reason as [`clear_own_blame`] — the viewer calls
/// this from a detached task that can complete after navigation.
pub fn clear_blame_held_by(
    blocked_by: RwSignal<std::collections::HashMap<String, String>>,
    cell_id: &str,
) {
    if blocked_by
        .try_with_untracked(|b| b.values().any(|blocker| blocker == cell_id))
        .unwrap_or(false)
    {
        blocked_by.update(|b| b.retain(|_, blocker| blocker != cell_id));
    }
}

#[cfg(test)]
mod tests {
    use super::{fail_in_queue_core, is_transport_error, merged, RunGuard, RunQueue};
    use leptos::prelude::*;
    use std::collections::HashMap;

    /// Only transport failures are retried (review bug-compiler-6); every
    /// server verdict, including the admission refusals, is final.
    #[test]
    fn only_transport_failures_are_retried() {
        for verdict in [
            ServerFnError::new("the server is at build capacity right now"),
            ServerFnError::new("rate limited: too many builds started from this client"),
            ServerFnError::new("build invocation failed: compilation timed out after 300s"),
            ServerFnError::Args("bad args".into()),
            ServerFnError::MissingArg("request".into()),
        ] {
            assert!(!is_transport_error(&verdict), "{verdict:?} is a verdict");
        }
        for transport in [
            ServerFnError::Request("connection reset".into()),
            ServerFnError::Response("body cut short".into()),
            ServerFnError::Deserialization("<html>502 Bad Gateway</html>".into()),
        ] {
            assert!(is_transport_error(&transport), "{transport:?} is transport");
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(ToString::to_string).collect()
    }

    /// The merge a single-cell request performs: its unexecuted
    /// dependencies plus the target, with the target never pinned.
    fn request_merge(
        order: &[&str],
        queue: &[&str],
        unexecuted: &[&str],
        target: &str,
    ) -> Vec<String> {
        let mut additions = ids(unexecuted);
        additions.push(target.to_string());
        merged(order, &ids(queue), &additions, Some(target))
    }

    // ── Run ownership ───────────────────────────────────────────────────

    #[test]
    fn a_click_meaning_stop_never_lets_the_run_continue() {
        // Moved from linux_cell.rs, where a continued run is a metered pod
        // boot; in the editor it is the old source executing.
        assert!(
            RunGuard::is_current_at(Some(7), 7),
            "nothing has intervened"
        );
        // A Stop, which bumps the epoch.
        assert!(!RunGuard::is_current_at(Some(8), 7));
        // Disposed mid-run: nobody is left to read the output.
        assert!(!RunGuard::is_current_at(None, 7));
        // Why an epoch rather than a flag: Stop and then Run again while the
        // first compile is still in flight. A flag would have been cleared by
        // the second click and the first task would carry on anyway.
        assert!(!RunGuard::is_current_at(Some(9), 7));
    }

    #[test]
    fn a_bumped_epoch_invalidates_the_guard() {
        Owner::new().with(|| {
            let epoch = RwSignal::new(0u64);
            let first = RunGuard::mint(epoch);
            assert!(first.is_current());

            // Stop: the in-flight run loses its claim.
            RunGuard::revoke(epoch);
            assert!(!first.is_current());

            // Stop, edit, Run: the new run owns the cell, the old one never
            // gets it back.
            let second = RunGuard::mint(epoch);
            assert!(second.is_current());
            assert!(!first.is_current());
        });
    }

    // ── The merge ───────────────────────────────────────────────────────

    #[test]
    fn merged_run_queue_from_empty_is_deps_then_target() {
        assert_eq!(
            request_merge(&["a", "b", "c"], &[], &["a"], "c"),
            ids(&["a", "c"])
        );
    }

    #[test]
    fn merged_run_queue_preserves_queued_independents() {
        // Run All in flight ([a, b, c], a at front), user clicks Run on
        // queued c (which depends on a): the old wholesale replacement
        // produced [a, c] and silently dropped b.
        assert_eq!(
            request_merge(&["a", "b", "c"], &["a", "b", "c"], &["a"], "c"),
            ids(&["a", "b", "c"])
        );
    }

    #[test]
    fn merged_run_queue_front_target_yields_to_deps_and_keeps_the_tail() {
        // Agent path: queue [y, z], y reaches the front with a cold
        // dependency d. The old replacement produced [d, y] and z's waiting
        // agent timed out.
        assert_eq!(
            request_merge(&["d", "y", "z"], &["y", "z"], &["d"], "y"),
            ids(&["d", "y", "z"])
        );
    }

    #[test]
    fn merged_run_queue_drops_a_ghost_front() {
        // The queued front was deleted mid-run (agent CellDelete): it must
        // not be pinned (nothing can ever advance a front that has no
        // watcher) and it vanishes from the merge entirely.
        assert_eq!(
            request_merge(&["b", "c"], &["ghost", "b", "c"], &[], "c"),
            ids(&["b", "c"])
        );
    }

    #[test]
    fn merged_run_queue_routes_a_direct_click_through_a_busy_queue() {
        // Independent cell x clicked while [a, b] runs: x joins in notebook
        // order behind the pinned (possibly mid-compile) front.
        assert_eq!(
            request_merge(&["a", "x", "b"], &["a", "b"], &[], "x"),
            ids(&["a", "x", "b"])
        );
    }

    #[test]
    fn run_all_during_a_run_pins_the_front_and_keeps_queued_cells() {
        // Review soc-app-client-5: an agent waits on z with [x, y, z] in
        // flight and x mid-compile. Run All Below on w used to REPLACE the
        // queue with [w]: y and z went Idle unreported and x's completion
        // could no longer advance a queue it did not lead.
        let order = ["a", "x", "y", "z", "w"];
        let running = ids(&["x", "y", "z"]);
        assert_eq!(
            merged(&order, &running, &ids(&["w"]), None),
            ids(&["x", "y", "z", "w"])
        );
        // Run All from the top keeps everything too, and x stays in front
        // rather than re-sorting behind a.
        assert_eq!(
            merged(&order, &running, &ids(&order), None),
            ids(&["x", "a", "y", "z", "w"])
        );
    }

    #[test]
    fn button_widget_during_a_run_drops_nothing() {
        // A button widget in b enqueues the cells after it while [a, c]
        // runs: the queue grows, it is never replaced.
        Owner::new().with(|| {
            let queue = RunQueue::new();
            let order = ["a", "b", "c", "d"];
            queue.enqueue_many(&order, &ids(&["a", "c"]));
            queue.enqueue_many(&order, &ids(&["c", "d"]));
            assert_eq!(queue.snapshot_untracked(), ids(&["a", "c", "d"]));
            assert_eq!(queue.position("a"), Some(0));
        });
    }

    #[test]
    fn set_if_idle_ignores_a_busy_queue() {
        Owner::new().with(|| {
            let queue = RunQueue::new();
            assert!(!queue.set_if_idle(Vec::new()), "nothing to start");
            assert!(queue.set_if_idle(ids(&["a", "b"])));
            assert!(!queue.set_if_idle(ids(&["c"])), "a run is in flight");
            assert_eq!(queue.snapshot_untracked(), ids(&["a", "b"]));
        });
    }

    #[test]
    fn cancel_reports_everything_but_the_excepted_cell() {
        Owner::new().with(|| {
            let queue = RunQueue::new();
            queue.enqueue_many(&["a", "b", "c"], &ids(&["a", "b", "c"]));
            assert_eq!(queue.cancel(Some("a")), ids(&["b", "c"]));
            assert!(queue.is_empty_untracked());

            // No exception names the front too.
            queue.enqueue_many(&["a", "b"], &ids(&["a", "b"]));
            assert_eq!(queue.cancel(None), ids(&["a", "b"]));

            // A queue holding only the excepted cell still empties.
            queue.push_unique("a");
            assert!(queue.cancel(Some("a")).is_empty());
            assert!(queue.is_empty_untracked());
            assert!(queue.cancel(None).is_empty(), "an idle queue drops nothing");
        });
    }

    #[test]
    fn request_runs_directly_only_when_nothing_must_wait() {
        Owner::new().with(|| {
            let queue = RunQueue::new();
            let order = ["d", "t", "x"];
            assert!(
                !queue.request(&order, &[], "t"),
                "idle and no deps: run now"
            );
            assert!(queue.is_empty_untracked());

            assert!(
                queue.request(&order, &ids(&["d"]), "t"),
                "a cold dependency queues"
            );
            assert_eq!(queue.snapshot_untracked(), ids(&["d", "t"]));

            assert!(
                queue.request(&order, &[], "x"),
                "another cell holds the queue"
            );
            assert_eq!(queue.snapshot_untracked(), ids(&["d", "t", "x"]));

            queue.advance("d");
            assert!(
                !queue.request(&order, &[], "t"),
                "the queue dispatching its front"
            );
        });
    }

    #[test]
    fn push_unique_advance_and_remove_keep_the_front_rule() {
        Owner::new().with(|| {
            let queue = RunQueue::new();
            queue.push_unique("a");
            queue.push_unique("b");
            queue.push_unique("a");
            assert_eq!(queue.snapshot_untracked(), ids(&["a", "b"]));
            queue.advance("b");
            assert_eq!(
                queue.snapshot_untracked(),
                ids(&["a", "b"]),
                "b is not the front"
            );
            queue.remove("a");
            assert_eq!(queue.position("b"), Some(0));
        });
    }

    #[test]
    fn fail_in_queue_drops_failed_and_dependents_keeps_independents() {
        let mut queue = ids(&["a", "b", "c", "d"]);
        let mut blocked = HashMap::new();
        fail_in_queue_core(&mut queue, &mut blocked, "a", &ids(&["c"]));
        assert_eq!(queue, ids(&["b", "d"]));
        assert_eq!(blocked.get("c").map(String::as_str), Some("a"));
        assert!(!blocked.contains_key("b"));
    }

    #[test]
    fn fail_in_queue_with_no_dependents_only_drops_the_failed_cell() {
        let mut queue = ids(&["a", "b"]);
        let mut blocked = HashMap::new();
        fail_in_queue_core(&mut queue, &mut blocked, "a", &[]);
        assert_eq!(queue, ids(&["b"]));
        assert!(blocked.is_empty());
    }
}
