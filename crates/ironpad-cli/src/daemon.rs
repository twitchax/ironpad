//! Long-lived daemon process.
//!
//! Maintains a WebSocket connection to the ironpad server and exposes
//! a Unix socket for fast CLI IPC. Caches notebook state locally so
//! read queries don't require a WebSocket round-trip.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};
use tokio_tungstenite::tungstenite;

use ironpad_common::notebook_ops;
use ironpad_common::protocol::{self, MessageKind, Query};
use ironpad_common::IronpadNotebook;

use crate::ipc::{IpcRequest, IpcResponse};

// ── Paths ───────────────────────────────────────────────────────────────────

/// Directory for daemon runtime files (`~/.ironpad`).
pub fn daemon_dir() -> PathBuf {
    home_dir().join(".ironpad")
}

fn home_dir() -> PathBuf {
    std::env::var("HOME").map_or_else(
        |_| {
            eprintln!("error: HOME environment variable is not set");
            std::process::exit(1);
        },
        PathBuf::from,
    )
}

pub fn socket_path() -> PathBuf {
    daemon_dir().join("daemon.sock")
}

pub fn pid_path() -> PathBuf {
    daemon_dir().join("daemon.pid")
}

// ── Daemon state ────────────────────────────────────────────────────────────

struct DaemonState {
    /// Whether the WebSocket is connected.
    connected: RwLock<bool>,
    /// Cached notebook (populated on connect via `NotebookGet` query).
    notebook: RwLock<Option<IronpadNotebook>>,
    /// Pending request-response pairs keyed by message ID.
    pending: RwLock<HashMap<String, oneshot::Sender<String>>>,
    /// Channel to send messages over the WebSocket. `None` until connected.
    ws_tx: RwLock<Option<mpsc::UnboundedSender<String>>>,
    /// Fan-out of every incoming event envelope (PRD-0052). `cells.run`
    /// subscribes here to await execution results, which are correlated by
    /// `cell_id` rather than message id (a run can cascade into prerequisites).
    events: broadcast::Sender<protocol::EventEnvelope>,
    /// Message id of the initial `NotebookGet`, so the recv path can cache
    /// its response INLINE — strictly ordered with the event stream. Caching
    /// it on the main task (after the oneshot resolved) left a window where
    /// an event arriving right behind the response was applied to a
    /// still-`None` cache and silently dropped: a permanently stale snapshot.
    init_query_id: RwLock<Option<String>>,
}

// ── Run daemon ──────────────────────────────────────────────────────────────

/// Start the daemon: bind Unix socket, connect to the server, run forever.
///
/// The socket is bound **before** the WebSocket connection so that CLI
/// commands (e.g. `status`) are reachable even while the WS handshake is
/// in progress. Commands that require a live WS return an error until the
/// connection is established.
pub async fn run(host: &str, token: &str) -> anyhow::Result<()> {
    // Ensure daemon directory exists, restricted to the owner. The socket and
    // pidfile inside carry live session authority, so a co-tenant user must not
    // be able to traverse in — don't rely on the ambient umask.
    let dir = daemon_dir();
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .await
        .with_context(|| format!("restricting permissions on {}", dir.display()))?;

    // Clean up stale socket.
    let sock = socket_path();
    if sock.exists() {
        tokio::fs::remove_file(&sock).await?;
    }

    // Write pidfile.
    let pid = pid_path();
    tokio::fs::write(&pid, std::process::id().to_string()).await?;

    // ── Bind Unix socket FIRST ──────────────────────────────────────────

    let listener = UnixListener::bind(&sock)?;
    // Restrict the socket to the owner so a co-tenant user can't drive the
    // daemon (issue mutations) or read cached notebook state through it.
    tokio::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))
        .await
        .with_context(|| format!("restricting permissions on {}", sock.display()))?;
    tracing::info!(path = %sock.display(), "listening on Unix socket");

    let state = Arc::new(DaemonState {
        connected: RwLock::new(false),
        notebook: RwLock::new(None),
        pending: RwLock::new(HashMap::new()),
        ws_tx: RwLock::new(None),
        // Capacity sized for bursts of execution events; a slow `cells.run`
        // waiter that lags simply skips ahead (handled at the recv site).
        events: broadcast::channel(256).0,
        init_query_id: RwLock::new(None),
    });

    // ── Task: Unix socket listener ──────────────────────────────────────

    let state_ipc = Arc::clone(&state);
    let ipc_task = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let st = Arc::clone(&state_ipc);
                    tokio::spawn(handle_ipc_connection(stream, st));
                }
                Err(e) => {
                    tracing::error!(error = %e, "Unix socket accept error");
                    break;
                }
            }
        }
    });

    // ── Connect WebSocket ───────────────────────────────────────────────

    let ws_url = format!("{host}/ws/connect?token={token}");
    // Redact the bearer token from logs (the full URL is only used to connect).
    tracing::info!(url = %format!("{host}/ws/connect?token=<redacted>"), "connecting to server");

    let ws_result = tokio_tungstenite::connect_async(&ws_url).await;

    let ws_tasks = match ws_result {
        Ok((ws_stream, _response)) => {
            tracing::info!("WebSocket connected");

            let (mut ws_sink, mut ws_source) = ws_stream.split();
            let (ws_tx, mut ws_rx) = mpsc::unbounded_channel::<String>();

            // Publish the WS sender so IPC handlers can use it.
            *state.ws_tx.write().await = Some(ws_tx.clone());
            *state.connected.write().await = true;

            // Request initial notebook state.
            let init_id = uuid::Uuid::new_v4().to_string();
            let init_msg = protocol::Message {
                id: init_id.clone(),
                kind: MessageKind::Query(Query::NotebookGet),
            };
            let (init_tx, init_rx) = oneshot::channel::<String>();
            *state.init_query_id.write().await = Some(init_id.clone());
            state.pending.write().await.insert(init_id, init_tx);
            if ws_tx.send(serde_json::to_string(&init_msg)?).is_err() {
                tracing::warn!("failed to send initial NotebookGet query — WS channel closed");
            }

            // ── Task: forward channel → WebSocket ───────────────────────
            let ws_send_task = tokio::spawn(async move {
                while let Some(msg) = ws_rx.recv().await {
                    if ws_sink
                        .send(tungstenite::Message::Text(msg.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });

            // ── Task: read WebSocket → dispatch ─────────────────────────
            let state_ws = Arc::clone(&state);
            let ws_recv_task = tokio::spawn(async move {
                while let Some(Ok(msg)) = ws_source.next().await {
                    if let tungstenite::Message::Text(text) = msg {
                        handle_ws_message(&text, &state_ws).await;
                    }
                }
                tracing::info!("WebSocket closed");
            });

            // Wait for the initial notebook fetch. The recv path cached it
            // (in stream order); this only gates startup on its arrival.
            match tokio::time::timeout(std::time::Duration::from_secs(10), init_rx).await {
                Ok(Ok(_)) => tracing::debug!("initial notebook response received"),
                Ok(Err(_)) => tracing::warn!("NotebookGet response channel closed"),
                Err(_) => tracing::warn!("timed out waiting for initial notebook state"),
            }

            Some((ws_send_task, ws_recv_task))
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to connect WebSocket — daemon will accept IPC but cannot reach server");
            None
        }
    };

    // ── Wait for shutdown ───────────────────────────────────────────────
    //
    // The daemon is intentionally session-scoped: it exits when the WebSocket
    // closes (the recv task ends). A closed socket means the host browser is
    // gone or the session ended, so there is nothing left to relay — exiting is
    // correct, and a reconnect loop would only keep a zombie daemon alive past
    // the session it belongs to. The CLI starts a fresh daemon per session.

    match ws_tasks {
        Some((ws_send_task, ws_recv_task)) => {
            tokio::select! {
                _ = ws_send_task => tracing::info!("WS send task ended"),
                _ = ws_recv_task => tracing::info!("WS recv task ended"),
                _ = ipc_task => tracing::info!("IPC task ended"),
                sig = shutdown_signal() => tracing::info!("received {sig}, shutting down"),
            }
        }
        None => {
            tokio::select! {
                _ = ipc_task => tracing::info!("IPC task ended"),
                sig = shutdown_signal() => tracing::info!("received {sig}, shutting down"),
            }
        }
    }

    // Cleanup.
    cleanup(&sock, &pid).await;
    Ok(())
}

/// Await a shutdown signal, returning its name for logging.
///
/// The daemon must run [`cleanup`] (remove socket + pidfile) on **both** an
/// interactive Ctrl-C (SIGINT) and the documented `daemon-stop` path, which
/// sends SIGTERM (`main.rs` → `libc::kill(pid, SIGTERM)`). Without a SIGTERM
/// handler the default disposition terminates the process immediately, so
/// `cleanup()` never runs and a stale socket + pidfile are left behind.
async fn shutdown_signal() -> &'static str {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            // Degrade to SIGINT-only; SIGTERM keeps its default disposition.
            tracing::warn!(error = %e, "failed to install SIGTERM handler; stop-path cleanup disabled");
            let _ = tokio::signal::ctrl_c().await;
            return "SIGINT";
        }
    };

    tokio::select! {
        _ = tokio::signal::ctrl_c() => "SIGINT",
        _ = sigterm.recv() => "SIGTERM",
    }
}

async fn cleanup(sock: &Path, pid: &Path) {
    let _ = tokio::fs::remove_file(sock).await;
    let _ = tokio::fs::remove_file(pid).await;
    tracing::info!("daemon shutdown complete");
}

// ── WebSocket message handling ──────────────────────────────────────────────

async fn handle_ws_message(text: &str, state: &DaemonState) {
    let Ok(msg) = serde_json::from_str::<protocol::Message>(text) else {
        tracing::warn!("received invalid JSON from server");
        return;
    };

    match &msg.kind {
        // Response to a pending query/mutation.
        MessageKind::Response(response) => {
            // The initial NotebookGet snapshot is cached HERE so it is
            // strictly ordered with the event stream (see the field docs on
            // `init_query_id` for the race this closes).
            let is_init = state.init_query_id.read().await.as_deref() == Some(msg.id.as_str());
            if is_init {
                if let protocol::Response::Notebook { notebook } = response {
                    *state.notebook.write().await = Some(notebook.clone());
                    tracing::info!("notebook state cached");
                } else {
                    tracing::warn!("unexpected response kind for NotebookGet: {response:?}");
                }
                *state.init_query_id.write().await = None;
            }
            let mut pending = state.pending.write().await;
            if let Some(tx) = pending.remove(&msg.id) {
                let _ = tx.send(text.to_string());
            }
        }

        // Event from the host browser — update local cache.
        // If the event has a pending message ID, it's the response to a
        // mutation we sent — resolve the pending request as well.
        MessageKind::Event(envelope) => {
            update_cache_from_event(&envelope.event, state).await;
            // Fan out to any `cells.run` waiters (PRD-0052). An error just
            // means nobody is subscribed right now.
            let _ = state.events.send(envelope.clone());
            if !msg.id.is_empty() {
                let mut pending = state.pending.write().await;
                if let Some(tx) = pending.remove(&msg.id) {
                    let _ = tx.send(text.to_string());
                }
            }
        }

        // Control messages (session ended, etc.).
        MessageKind::Control(ctrl) => {
            tracing::info!(control = ?ctrl, "received control message");
        }

        MessageKind::Mutation(_) | MessageKind::Query(_) => {
            tracing::debug!("unexpected mutation/query from server");
        }
    }
}

async fn update_cache_from_event(event: &protocol::Event, state: &DaemonState) {
    let mut nb_guard = state.notebook.write().await;
    let Some(nb) = nb_guard.as_mut() else { return };

    match event {
        protocol::Event::CellAdded {
            cell,
            after_cell_id,
        } => notebook_ops::insert_cell(&mut nb.cells, cell.clone(), after_cell_id.as_deref()),
        protocol::Event::CellUpdated {
            cell_id,
            source,
            cargo_toml,
            label,
            shared,
            collapsed,
            output_collapsed,
            version,
        } => {
            if let Some(cell) = nb.cells.iter_mut().find(|c| &c.id == cell_id) {
                notebook_ops::CellPatch {
                    source: source.as_ref(),
                    cargo_toml: cargo_toml.as_ref(),
                    label: label.as_ref(),
                    shared: *shared,
                    collapsed: *collapsed,
                    output_collapsed: *output_collapsed,
                }
                .apply_to(cell, *version);
            }
        }
        protocol::Event::CellDeleted { cell_id } => {
            notebook_ops::delete_cell(&mut nb.cells, cell_id);
        }
        protocol::Event::CellReordered { cell_ids } => {
            notebook_ops::reorder_cells(&mut nb.cells, cell_ids);
        }
        protocol::Event::NotebookMetaUpdated { meta } => meta.apply_to(nb),
        // Compilation/execution events don't affect the notebook structure; an
        // unknown event from a newer peer is likewise ignored here.
        protocol::Event::CellCompiling { .. }
        | protocol::Event::CellCompiled { .. }
        | protocol::Event::CellExecuted { .. }
        | protocol::Event::CellBlocked { .. }
        | protocol::Event::RunCancelled { .. }
        | protocol::Event::Error { .. }
        | protocol::Event::Unknown => {}
    }
}

// The per-event structural appliers (insert/update/delete/reorder/renumber)
// used to live here as hand-written mirrors of the browser model's mutation
// semantics. They are now `ironpad_common::notebook_ops`, shared with
// `NotebookModel` — same unification the metadata mirror already went
// through (`NotebookMetaPatch::apply_to`) — so the daemon's cached notebook
// and the authoritative one cannot disagree.

// ── IPC connection handling ─────────────────────────────────────────────────

async fn handle_ipc_connection(stream: tokio::net::UnixStream, state: Arc<DaemonState>) {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    loop {
        let line = match crate::ipc::read_frame(&mut reader).await {
            Ok(Some(line)) => line,
            Ok(None) => break, // clean EOF
            Err(e) => {
                // Oversized/unreadable frame — drop the connection rather than
                // buffer without bound.
                tracing::warn!(error = %e, "rejecting IPC frame");
                break;
            }
        };
        let response = handle_ipc_request(&line, &state).await;
        let mut json = serde_json::to_string(&response)
            .unwrap_or_else(|_| r#"{"ok":false,"error":"serialization error"}"#.to_string());
        json.push('\n');
        if writer.write_all(json.as_bytes()).await.is_err() {
            break;
        }
    }
}

async fn handle_ipc_request(line: &str, state: &DaemonState) -> IpcResponse {
    let Ok(req) = serde_json::from_str::<IpcRequest>(line) else {
        return IpcResponse::error("invalid JSON request");
    };

    match req.command.as_str() {
        "notebook.get" => serve_notebook_get(state).await,
        "cells.list" => serve_cells_list(state).await,
        "cells.get" => serve_cells_get(&req, state).await,
        "cells.run" => serve_cells_run(&req, state).await,
        "status" => serve_status(state).await,
        _ => forward_to_server(&req, state).await,
    }
}

// ── Per-command IPC handlers ────────────────────────────────────────────────

async fn serve_notebook_get(state: &DaemonState) -> IpcResponse {
    let nb = state.notebook.read().await;
    match nb.as_ref() {
        Some(notebook) => {
            IpcResponse::success(serde_json::to_value(notebook).expect("notebook serialization"))
        }
        None => IpcResponse::error("no notebook cached"),
    }
}

async fn serve_cells_list(state: &DaemonState) -> IpcResponse {
    let nb = state.notebook.read().await;
    match nb.as_ref() {
        Some(notebook) => {
            let cells: Vec<serde_json::Value> = notebook
                .cells
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "order": c.order,
                        "label": c.label,
                        "cell_type": c.cell_type,
                        "shared": c.shared,
                        "collapsed": c.collapsed,
                        "output_collapsed": c.output_collapsed,
                        "source_preview": c.source.chars().take(80).collect::<String>(),
                        "version": c.version,
                    })
                })
                .collect();
            IpcResponse::success(serde_json::Value::Array(cells))
        }
        None => IpcResponse::error("no notebook cached"),
    }
}

async fn serve_cells_get(req: &IpcRequest, state: &DaemonState) -> IpcResponse {
    let cell_id = req.args.get("cell_id").and_then(|v| v.as_str());
    let Some(cell_id) = cell_id else {
        return IpcResponse::error("missing cell_id argument");
    };
    let nb = state.notebook.read().await;
    match nb
        .as_ref()
        .and_then(|nb| nb.cells.iter().find(|c| c.id == cell_id))
    {
        Some(cell) => IpcResponse::success(serde_json::to_value(cell).expect("cell serialization")),
        None => IpcResponse::error_with_code("cell not found", "CellNotFound"),
    }
}

async fn serve_status(state: &DaemonState) -> IpcResponse {
    IpcResponse::success(serde_json::json!({
        "connected": *state.connected.read().await,
        "cached": state.notebook.read().await.is_some(),
    }))
}

/// Default wait for a `cells.run` execution result: the build timeout (300s)
/// plus headroom for queueing, wasm-opt, and the run itself. Also the clap
/// default for `cells run --timeout-secs` in `main.rs` — one constant.
pub const RUN_WAIT_DEFAULT_SECS: u64 = 360;

/// `cells.run` (PRD-0052): ask the hosting browser to run a cell, then wait
/// for its terminal execution event.
///
/// Subscribes to the event fan-out BEFORE sending — a cache-hit compile of a
/// trivial cell can finish fast enough to race a late subscription — then
/// confirms the `CellRunStarted` ack and waits for the outcome, correlated by
/// `cell_id` (see [`run_outcome`]).
async fn serve_cells_run(req: &IpcRequest, state: &DaemonState) -> IpcResponse {
    let Some(cell_id) = req.args.get("cell_id").and_then(|v| v.as_str()) else {
        return IpcResponse::error("missing cell_id argument");
    };
    let wait = req
        .args
        .get("wait")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let timeout_secs = req
        .args
        .get("timeout_secs")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(RUN_WAIT_DEFAULT_SECS);

    let mut events = state.events.subscribe();

    let ack = forward_to_server(req, state).await;
    if !ack.ok || !wait {
        return ack;
    }

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    // A dependency-inference verdict is HELD for a grace window rather than
    // returned: the browser reports real drops (`CellBlocked`, protocol v6)
    // right after the failure event, and our own cell's activity refutes the
    // inference outright — the inference only wins when neither arrives
    // (a pre-v6 browser, or a genuinely dead queue).
    let mut held: Option<serde_json::Value> = None;
    let mut grace_until: Option<tokio::time::Instant> = None;
    loop {
        let wait_until = grace_until.map_or(deadline, |g| g.min(deadline));
        let event = match tokio::time::timeout_at(wait_until, events.recv()).await {
            Ok(Ok(envelope)) => envelope.event,
            // Lagged: older events were dropped from the ring; ours may be
            // among the survivors, so keep reading.
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                return IpcResponse::error_with_code(
                    "event stream closed before the run finished",
                    "connection_error",
                )
            }
            Err(_) => {
                // Grace (or the overall deadline) expired with a held
                // inference and no authoritative signal either way — the
                // pre-v6 fallback verdict stands.
                if let Some(verdict) = held.take() {
                    return IpcResponse::success(verdict);
                }
                return IpcResponse::error_with_code(
                    format!("timed out after {timeout_secs}s waiting for the run to finish"),
                    "timeout",
                );
            }
        };
        // A dependency check only matters for ANOTHER cell's failure, and
        // it needs the cached notebook — read it lazily per candidate event.
        let needs_dep_check = matches!(
            &event,
            protocol::Event::CellExecuted { cell_id: id, success: false, .. }
            | protocol::Event::CellCompiled { cell_id: id, success: false, .. }
                if id != cell_id
        );
        let notebook = if needs_dep_check {
            state.notebook.read().await.clone()
        } else {
            None
        };
        if let Some(outcome) =
            apply_signal(&mut held, run_signal(cell_id, &event, notebook.as_ref()))
        {
            return IpcResponse::success(outcome);
        }
        grace_until = match (&held, grace_until) {
            (Some(_), None) => Some(
                tokio::time::Instant::now() + std::time::Duration::from_millis(INFERENCE_GRACE_MS),
            ),
            (Some(_), some) => some,
            (None, _) => None,
        };
    }
}

/// How long a dependency-INFERENCE verdict is held before it is trusted.
/// A v6 browser emits `CellBlocked` synchronously after the failure event,
/// so the real report arrives within relay latency (tens of ms); the window
/// only elapses fully against a pre-v6 browser or a genuinely stale guess.
const INFERENCE_GRACE_MS: u64 = 1_500;

/// Fold one classified event into the wait: authoritative terminals return
/// immediately; an inference is held (first one wins) for the grace window;
/// our cell showing life cancels a held inference — the browser evidently
/// kept us past the failure, so the stale-cache guess was wrong.
fn apply_signal(
    held: &mut Option<serde_json::Value>,
    signal: RunSignal,
) -> Option<serde_json::Value> {
    match signal {
        RunSignal::Terminal(v) => Some(v),
        RunSignal::Inferred(v) => {
            if held.is_none() {
                *held = Some(v);
            }
            None
        }
        RunSignal::OursAlive => {
            *held = None;
            None
        }
        RunSignal::None => None,
    }
}

/// Does `target_id` transitively depend on `failed_id` in this notebook —
/// the same shared recipe the browser's queue policy runs
/// (`ironpad_common::cell_deps`), so daemon and UI classify a failure
/// identically. Unknown ids degrade to `true` (terminal, never a hang).
fn target_depends_on(notebook: &IronpadNotebook, target_id: &str, failed_id: &str) -> bool {
    let target = notebook.cells.iter().position(|c| c.id == target_id);
    let failed = notebook.cells.iter().position(|c| c.id == failed_id);
    let (Some(target), Some(failed)) = (target, failed) else {
        return true;
    };
    let graph: Vec<ironpad_common::cell_deps::DepCell<'_>> = notebook
        .cells
        .iter()
        .map(|c| ironpad_common::cell_deps::DepCell {
            runnable: c.is_runnable(),
            source: Some(&c.source),
        })
        .collect();
    ironpad_common::cell_deps::transitive_dep_indices(&graph, target).contains(&failed)
}

/// Classification of an incoming event relative to a waited-on run.
#[derive(Debug)]
enum RunSignal {
    /// Authoritative terminal outcome — return it immediately.
    Terminal(serde_json::Value),
    /// Terminal only by DEPENDENCY INFERENCE against the daemon's cached
    /// notebook — held for a grace window (see [`serve_cells_run`]) rather
    /// than returned, because the cache can trail the browser's live Monaco
    /// buffers by the save debounce and guess wrong in both directions.
    Inferred(serde_json::Value),
    /// Our cell is visibly alive (compiling, or compiled cleanly): the
    /// browser kept it past whatever failed, refuting any held inference.
    OursAlive,
    /// Nothing to conclude; keep waiting.
    None,
}

/// Maps an incoming event to a [`RunSignal`] for a run of `cell_id`.
///
/// Authoritative terminals:
/// - `CellExecuted` for our cell — done, successfully or not.
/// - `CellCompiled { success: false }` for our cell — compile failure.
/// - `CellBlocked` for our cell — the browser's report that our queued run
///   was dropped because a dependency failed (protocol v6). The browser
///   decides drops against sources that include live Monaco buffers this
///   daemon's cache never sees, so its verdict beats any local inference.
/// - `RunCancelled` naming our cell — the user terminated the run and every
///   queued cell was dropped, dependents and independents alike.
/// - `CellDeleted` for our cell — the target is gone mid-run and will never
///   emit a compile/execute event; without this arm the wait could only end
///   at the timeout.
///
/// Fallback inference ([`RunSignal::Inferred`], grace-held): a compile OR
/// runtime failure of a cell our cell transitively DEPENDS ON (PRD-0060) —
/// `prerequisite_failed`, payload naming the cell. Kept for browsers
/// predating `CellBlocked` (a cached bundle can lag a deploy). The
/// dependency test runs the same shared recipe as the browser (`cell_deps`)
/// against the daemon's cached notebook; with no cached notebook every
/// other-cell failure is inferred-terminal (conservative — never a hang).
///
/// An UNRELATED cell's failure concludes nothing: the queue continues past
/// it and our run still executes (continue-past-failures).
fn run_signal(
    cell_id: &str,
    event: &protocol::Event,
    notebook: Option<&IronpadNotebook>,
) -> RunSignal {
    match event {
        protocol::Event::CellExecuted {
            cell_id: id,
            display_text,
            type_tag,
            execution_time_ms,
            success,
        } if id == cell_id => RunSignal::Terminal(serde_json::json!({
            "status": if *success { "executed" } else { "execution_error" },
            "cell_id": id,
            "display_text": display_text,
            "type_tag": type_tag,
            "execution_time_ms": execution_time_ms,
        })),
        protocol::Event::CellExecuted {
            cell_id: id,
            display_text,
            success: false,
            ..
        } => {
            if notebook.is_some_and(|nb| !target_depends_on(nb, cell_id, id)) {
                return RunSignal::None; // independent failure: the queue continues
            }
            RunSignal::Inferred(serde_json::json!({
                "status": "prerequisite_failed",
                "cell_id": id,
                "display_text": display_text,
            }))
        }
        protocol::Event::CellCompiled {
            cell_id: id,
            diagnostics,
            success: false,
        } => {
            if id == cell_id {
                return RunSignal::Terminal(serde_json::json!({
                    "status": "compile_error",
                    "cell_id": id,
                    "diagnostics": diagnostics,
                }));
            }
            if notebook.is_some_and(|nb| !target_depends_on(nb, cell_id, id)) {
                return RunSignal::None; // independent failure: the queue continues
            }
            RunSignal::Inferred(serde_json::json!({
                "status": "prerequisite_failed",
                "cell_id": id,
                "diagnostics": diagnostics,
            }))
        }
        protocol::Event::CellCompiling { cell_id: id }
        | protocol::Event::CellCompiled {
            cell_id: id,
            success: true,
            ..
        } if id == cell_id => RunSignal::OursAlive,
        protocol::Event::CellBlocked {
            cell_id: id,
            blocked_by,
        } if id == cell_id => RunSignal::Terminal(serde_json::json!({
            "status": "prerequisite_failed",
            "cell_id": blocked_by,
            "blocked_cell_id": id,
        })),
        protocol::Event::RunCancelled { cell_ids } if cell_ids.iter().any(|id| id == cell_id) => {
            RunSignal::Terminal(serde_json::json!({
                "status": "cancelled",
                "cell_id": cell_id,
            }))
        }
        protocol::Event::CellDeleted { cell_id: id } if id == cell_id => {
            RunSignal::Terminal(serde_json::json!({
                "status": "cell_deleted",
                "cell_id": id,
            }))
        }
        _ => RunSignal::None,
    }
}

/// Forward an IPC command to the server via the WebSocket and wait for the response.
async fn forward_to_server(req: &IpcRequest, state: &DaemonState) -> IpcResponse {
    // Check that the WebSocket is connected.
    let ws_tx = {
        let guard = state.ws_tx.read().await;
        match guard.as_ref() {
            Some(tx) => tx.clone(),
            None => {
                return IpcResponse::error_with_code(
                    "WebSocket not yet connected",
                    "connection_error",
                )
            }
        }
    };

    // Translate IPC command → protocol message.
    let msg_id = uuid::Uuid::new_v4().to_string();
    let kind = match translate_command(req) {
        Ok(kind) => kind,
        Err(e) => return IpcResponse::error(e),
    };

    let msg = protocol::Message {
        id: msg_id.clone(),
        kind,
    };

    let json = match serde_json::to_string(&msg) {
        Ok(j) => {
            tracing::debug!(ws_send = %j, "forwarding to server");
            j
        }
        Err(e) => return IpcResponse::error(format!("serialization error: {e}")),
    };

    // Set up response channel.
    let (tx, rx) = oneshot::channel::<String>();
    state.pending.write().await.insert(msg_id.clone(), tx);

    // Send over WebSocket.
    if ws_tx.send(json).is_err() {
        state.pending.write().await.remove(&msg_id);
        return IpcResponse::error_with_code("WebSocket disconnected", "connection_error");
    }

    // Wait for response (10s timeout).
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), rx).await;

    // Clean up pending entry if not already resolved.
    state.pending.write().await.remove(&msg_id);

    match result {
        Ok(Ok(response_text)) => match serde_json::from_str::<protocol::Message>(&response_text) {
            Ok(response_msg) => translate_response(response_msg),
            Err(e) => IpcResponse::error(format!("invalid response: {e}")),
        },
        Ok(Err(_)) => IpcResponse::error_with_code("response channel closed", "connection_error"),
        Err(_) => IpcResponse::error_with_code("request timed out", "connection_error"),
    }
}

/// Translate an IPC command name + args into a protocol `MessageKind`.
fn translate_command(req: &IpcRequest) -> Result<MessageKind, String> {
    match req.command.as_str() {
        "cells.add" => {
            let source = req
                .args
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cell_type = match req.args.get("type").and_then(|v| v.as_str()) {
                Some("markdown") => ironpad_common::CellType::Markdown,
                _ => ironpad_common::CellType::Code,
            };
            let label = req
                .args
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or(protocol::DEFAULT_CELL_LABEL)
                .to_string();
            let after_cell_id = req
                .args
                .get("after_cell_id")
                .and_then(|v| v.as_str())
                .map(String::from);
            let cargo_toml = req
                .args
                .get("cargo_toml")
                .and_then(|v| v.as_str())
                .map(String::from);

            let shared = req
                .args
                .get("shared")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            Ok(MessageKind::Mutation(protocol::Mutation::CellAdd {
                cell: protocol::NewCell {
                    source,
                    cell_type,
                    label,
                    cargo_toml,
                    shared,
                },
                after_cell_id,
            }))
        }
        "cells.update" => {
            let cell_id = req
                .args
                .get("cell_id")
                .and_then(|v| v.as_str())
                .ok_or("missing cell_id")?
                .to_string();
            let source = req
                .args
                .get("source")
                .and_then(|v| v.as_str())
                .map(String::from);
            let cargo_toml = req
                .args
                .get("cargo_toml")
                .and_then(|v| v.as_str())
                .map(|s| Some(s.to_string()));
            let label = req
                .args
                .get("label")
                .and_then(|v| v.as_str())
                .map(String::from);
            let shared = req.args.get("shared").and_then(serde_json::Value::as_bool);
            let collapsed = req
                .args
                .get("collapsed")
                .and_then(serde_json::Value::as_bool);
            let output_collapsed = req
                .args
                .get("output_collapsed")
                .and_then(serde_json::Value::as_bool);
            let version = req
                .args
                .get("version")
                .and_then(serde_json::Value::as_u64)
                .ok_or("missing version (required for optimistic concurrency)")?;

            Ok(MessageKind::Mutation(protocol::Mutation::CellUpdate {
                cell_id,
                source,
                cargo_toml,
                label,
                shared,
                collapsed,
                output_collapsed,
                version,
            }))
        }
        "cells.delete" => {
            let cell_id = req
                .args
                .get("cell_id")
                .and_then(|v| v.as_str())
                .ok_or("missing cell_id")?
                .to_string();
            let version = req
                .args
                .get("version")
                .and_then(serde_json::Value::as_u64)
                .ok_or("missing version (required for optimistic concurrency)")?;

            Ok(MessageKind::Mutation(protocol::Mutation::CellDelete {
                cell_id,
                version,
            }))
        }
        "notebook.update" => {
            // The args carry a `NotebookMetaPatch` verbatim; deserializing it
            // (rather than plucking fields) keeps the CLI's surface in
            // lockstep with the protocol — a field added to the patch is
            // immediately sendable. `explicit_null_is_a_clear` turns the
            // JSON's absent/null/value into untouched/clear/set.
            let meta_value = req.args.get("meta").cloned().ok_or("missing meta")?;
            let meta: protocol::NotebookMetaPatch = serde_json::from_value(meta_value)
                .map_err(|e| format!("invalid meta patch: {e}"))?;
            Ok(MessageKind::Mutation(
                protocol::Mutation::NotebookUpdateMeta { meta },
            ))
        }
        "cells.reorder" => {
            let cell_ids: Vec<String> = req
                .args
                .get("cell_ids")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            Ok(MessageKind::Mutation(protocol::Mutation::CellReorder {
                cell_ids,
            }))
        }
        "cells.run" => {
            let cell_id = req
                .args
                .get("cell_id")
                .and_then(|v| v.as_str())
                .ok_or("missing cell_id")?
                .to_string();
            Ok(MessageKind::Mutation(protocol::Mutation::CellRun {
                cell_id,
            }))
        }
        cmd => Err(format!("unknown command: {cmd}")),
    }
}

/// Translate a protocol response message into an IPC response.
fn translate_response(msg: protocol::Message) -> IpcResponse {
    match msg.kind {
        MessageKind::Response(response) => match response {
            protocol::Response::Notebook { notebook } => IpcResponse::success(
                serde_json::to_value(notebook).expect("notebook serialization"),
            ),
            protocol::Response::Cell { cell } => {
                IpcResponse::success(serde_json::to_value(cell).expect("cell serialization"))
            }
            protocol::Response::CellsList { cells } => {
                IpcResponse::success(serde_json::to_value(cells).expect("cells serialization"))
            }
            protocol::Response::MutationOk { detail } => {
                IpcResponse::success(serde_json::to_value(detail).expect("detail serialization"))
            }
            protocol::Response::Error { code, message } => {
                IpcResponse::error_with_code(message, format!("{code:?}"))
            }
            protocol::Response::Unknown => {
                IpcResponse::error("unrecognized response from a newer server")
            }
        },
        MessageKind::Event(envelope) => {
            IpcResponse::success(serde_json::to_value(envelope).expect("envelope serialization"))
        }
        _ => IpcResponse::error("unexpected response type"),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ironpad_common::protocol::{
        ClientId, ErrorCode, Event, EventEnvelope, MutationResult, Response,
    };
    use ironpad_common::{CellType, IronpadCell, IronpadNotebook};
    use serde_json::json;

    // ── helpers ─────────────────────────────────────────────────────────

    fn ipc(command: &str, args: serde_json::Value) -> IpcRequest {
        IpcRequest {
            command: command.to_string(),
            args,
        }
    }

    fn make_cell(id: &str, order: u32) -> IronpadCell {
        IronpadCell {
            id: id.to_string(),
            order,
            label: format!("Cell {id}"),
            cell_type: CellType::Code,
            source: String::new(),
            cargo_toml: None,
            shared: false,
            collapsed: false,
            output_collapsed: false,
            version: 1,
            saved_output: None,
        }
    }

    fn test_state() -> DaemonState {
        DaemonState {
            connected: RwLock::new(true),
            notebook: RwLock::new(None),
            pending: RwLock::new(HashMap::new()),
            ws_tx: RwLock::new(None),
            events: broadcast::channel(16).0,
            init_query_id: RwLock::new(None),
        }
    }

    /// Regression for the startup race: an event arriving right behind the
    /// initial `NotebookGet` response used to hit a still-`None` cache (the
    /// main task cached the snapshot only after its oneshot resolved) and
    /// was silently dropped — a permanently stale snapshot. Caching on the
    /// recv path makes response-then-event ordering deterministic.
    #[tokio::test]
    async fn init_response_and_trailing_event_apply_in_stream_order() {
        let state = test_state();
        *state.init_query_id.write().await = Some("init-1".to_string());

        let mut nb = IronpadNotebook::new("Race");
        nb.cells = vec![make_cell("a", 0)];
        let response = serde_json::to_string(&protocol::Message {
            id: "init-1".to_string(),
            kind: MessageKind::Response(protocol::Response::Notebook { notebook: nb }),
        })
        .unwrap();
        let event = serde_json::to_string(&protocol::Message {
            id: String::new(),
            kind: MessageKind::Event(protocol::EventEnvelope {
                event: protocol::Event::CellAdded {
                    cell: make_cell("b", 1),
                    after_cell_id: Some("a".to_string()),
                },
                by: protocol::ClientId::browser(),
            }),
        })
        .unwrap();

        // The exact wire order that used to lose the event.
        handle_ws_message(&response, &state).await;
        handle_ws_message(&event, &state).await;

        let cached = state.notebook.read().await;
        let cells: Vec<&str> = cached
            .as_ref()
            .expect("snapshot cached on the recv path")
            .cells
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(cells, ["a", "b"], "the trailing event must not be lost");
        assert!(
            state.init_query_id.read().await.is_none(),
            "init id cleared after use"
        );
    }

    // ── cells.run (PRD-0052) ─────────────────────────────────────────────

    #[test]
    fn translate_cells_run() {
        let req = ipc("cells.run", json!({ "cell_id": "cell-3" }));
        let kind = translate_command(&req).unwrap();
        match kind {
            MessageKind::Mutation(protocol::Mutation::CellRun { cell_id }) => {
                assert_eq!(cell_id, "cell-3");
            }
            other => panic!("expected CellRun, got {other:?}"),
        }
        assert!(translate_command(&ipc("cells.run", json!({}))).is_err());
    }

    /// Unwrap helpers for the [`RunSignal`] shape.
    fn terminal(sig: RunSignal) -> serde_json::Value {
        match sig {
            RunSignal::Terminal(v) => v,
            other => panic!("expected Terminal, got {other:?}"),
        }
    }
    fn inferred(sig: RunSignal) -> serde_json::Value {
        match sig {
            RunSignal::Inferred(v) => v,
            other => panic!("expected Inferred, got {other:?}"),
        }
    }

    #[test]
    fn run_signal_terminal_and_alive_shapes() {
        // Our own activity is OursAlive (it refutes a held inference), and
        // someone else's success concludes nothing.
        assert!(matches!(
            run_signal(
                "mine",
                &Event::CellCompiling {
                    cell_id: "mine".into()
                },
                None,
            ),
            RunSignal::OursAlive
        ));
        assert!(matches!(
            run_signal(
                "mine",
                &Event::CellCompiled {
                    cell_id: "mine".into(),
                    diagnostics: vec![],
                    success: true,
                },
                None,
            ),
            RunSignal::OursAlive
        ));
        assert!(matches!(
            run_signal(
                "mine",
                &Event::CellExecuted {
                    cell_id: "other".into(),
                    display_text: None,
                    type_tag: None,
                    execution_time_ms: 1.0,
                    success: true,
                },
                None,
            ),
            RunSignal::None
        ));

        // Terminal: our execution, either way.
        let ok = terminal(run_signal(
            "mine",
            &Event::CellExecuted {
                cell_id: "mine".into(),
                display_text: Some("42".into()),
                type_tag: Some("u32".into()),
                execution_time_ms: 3.5,
                success: true,
            },
            None,
        ));
        assert_eq!(ok["status"], "executed");
        assert_eq!(ok["display_text"], "42");

        let failed = terminal(run_signal(
            "mine",
            &Event::CellExecuted {
                cell_id: "mine".into(),
                display_text: Some("boom".into()),
                type_tag: None,
                execution_time_ms: 0.0,
                success: false,
            },
            None,
        ));
        assert_eq!(failed["status"], "execution_error");

        // Terminal: our compile failure. Another cell's failure with NO
        // cached notebook is INFERRED (conservative) — held for the grace
        // window rather than returned outright, never a hang.
        let compile_err = terminal(run_signal(
            "mine",
            &Event::CellCompiled {
                cell_id: "mine".into(),
                diagnostics: vec![],
                success: false,
            },
            None,
        ));
        assert_eq!(compile_err["status"], "compile_error");

        let prereq = inferred(run_signal(
            "mine",
            &Event::CellCompiled {
                cell_id: "upstream".into(),
                diagnostics: vec![],
                success: false,
            },
            None,
        ));
        assert_eq!(prereq["status"], "prerequisite_failed");
        assert_eq!(prereq["cell_id"], "upstream");

        let runtime_prereq = inferred(run_signal(
            "mine",
            &Event::CellExecuted {
                cell_id: "upstream".into(),
                display_text: Some("panicked at 'boom'".into()),
                type_tag: None,
                execution_time_ms: 0.0,
                success: false,
            },
            None,
        ));
        assert_eq!(runtime_prereq["status"], "prerequisite_failed");
        assert_eq!(runtime_prereq["display_text"], "panicked at 'boom'");
    }

    #[test]
    fn run_signal_blocked_cancelled_deleted_are_terminal_for_the_target_only() {
        // The browser's authoritative drop report (protocol v6): terminal
        // even with no cached notebook — no inference needed.
        let blocked = terminal(run_signal(
            "mine",
            &Event::CellBlocked {
                cell_id: "mine".into(),
                blocked_by: "dep".into(),
            },
            None,
        ));
        assert_eq!(blocked["status"], "prerequisite_failed");
        assert_eq!(blocked["cell_id"], "dep");
        assert_eq!(blocked["blocked_cell_id"], "mine");

        // Another cell's drop keeps our wait open: the queue continues.
        assert!(matches!(
            run_signal(
                "mine",
                &Event::CellBlocked {
                    cell_id: "other".into(),
                    blocked_by: "dep".into(),
                },
                None,
            ),
            RunSignal::None
        ));

        // User-terminate drops the whole queue and reports it: terminal for
        // every named cell (dependent or independent), no one waits out the
        // timeout.
        let cancelled = terminal(run_signal(
            "mine",
            &Event::RunCancelled {
                cell_ids: vec!["other".into(), "mine".into()],
            },
            None,
        ));
        assert_eq!(cancelled["status"], "cancelled");
        assert!(matches!(
            run_signal(
                "mine",
                &Event::RunCancelled {
                    cell_ids: vec!["other".into()],
                },
                None,
            ),
            RunSignal::None
        ));

        // Target deleted mid-run: no compile/execute event will ever come,
        // so this must be terminal rather than a guaranteed timeout.
        let deleted = terminal(run_signal(
            "mine",
            &Event::CellDeleted {
                cell_id: "mine".into(),
            },
            None,
        ));
        assert_eq!(deleted["status"], "cell_deleted");
        assert!(matches!(
            run_signal(
                "mine",
                &Event::CellDeleted {
                    cell_id: "other".into(),
                },
                None,
            ),
            RunSignal::None
        ));
    }

    #[test]
    fn run_signal_is_dependency_aware_with_a_cached_notebook() {
        use ironpad_common::IronpadCell;

        fn code(id: &str, source: &str, order: u32) -> IronpadCell {
            IronpadCell {
                id: id.to_string(),
                order,
                label: id.to_string(),
                cell_type: ironpad_common::CellType::Code,
                source: source.to_string(),
                cargo_toml: None,
                shared: false,
                collapsed: false,
                output_collapsed: false,
                version: 0,
                saved_output: None,
            }
        }

        // mine consumes slot 0 ("dep"); "indep" is independent of everything.
        let mut nb = IronpadNotebook::new("t");
        nb.cells = vec![
            code("dep", "1 + 1", 0),
            code("indep", "42", 1),
            code("mine", "cell0 * 2", 2),
        ];

        let failure = |id: &str| Event::CellCompiled {
            cell_id: id.into(),
            diagnostics: vec![],
            success: false,
        };

        // An INDEPENDENT cell's failure concludes nothing: the queue
        // continues past it (PRD-0060) and our run still executes.
        assert!(matches!(
            run_signal("mine", &failure("indep"), Some(&nb)),
            RunSignal::None
        ));

        // A cell we depend on failing is INFERRED terminal (grace-held).
        let out = inferred(run_signal("mine", &failure("dep"), Some(&nb)));
        assert_eq!(out["status"], "prerequisite_failed");
        assert_eq!(out["cell_id"], "dep");

        // Runtime failures classify the same way.
        let runtime = |id: &str| Event::CellExecuted {
            cell_id: id.into(),
            display_text: Some("boom".into()),
            type_tag: None,
            execution_time_ms: 0.0,
            success: false,
        };
        assert!(matches!(
            run_signal("mine", &runtime("indep"), Some(&nb)),
            RunSignal::None
        ));
        assert!(matches!(
            run_signal("mine", &runtime("dep"), Some(&nb)),
            RunSignal::Inferred(_)
        ));

        // Unknown failed id degrades to inferred-terminal (never a hang).
        assert!(matches!(
            run_signal("mine", &failure("ghost"), Some(&nb)),
            RunSignal::Inferred(_)
        ));
    }

    #[test]
    fn apply_signal_holds_inference_until_refuted_or_confirmed() {
        let prereq = serde_json::json!({ "status": "prerequisite_failed" });
        let executed = serde_json::json!({ "status": "executed" });

        // Inference is held, not returned; a second inference doesn't
        // overwrite the first.
        let mut held = None;
        assert!(apply_signal(&mut held, RunSignal::Inferred(prereq.clone())).is_none());
        assert_eq!(held.as_ref().unwrap()["status"], "prerequisite_failed");
        assert!(apply_signal(
            &mut held,
            RunSignal::Inferred(serde_json::json!({ "status": "second" }))
        )
        .is_none());
        assert_eq!(held.as_ref().unwrap()["status"], "prerequisite_failed");

        // Our cell showing life refutes the held guess entirely...
        assert!(apply_signal(&mut held, RunSignal::OursAlive).is_none());
        assert!(held.is_none());

        // ...and the authoritative terminal wins immediately, held or not.
        assert!(apply_signal(&mut held, RunSignal::Inferred(prereq)).is_none());
        let out = apply_signal(&mut held, RunSignal::Terminal(executed)).unwrap();
        assert_eq!(out["status"], "executed");
    }

    // ── translate_command ────────────────────────────────────────────────

    #[test]
    fn translate_cells_add_defaults() {
        let req = ipc("cells.add", json!({}));
        let kind = translate_command(&req).unwrap();

        match kind {
            MessageKind::Mutation(protocol::Mutation::CellAdd {
                cell,
                after_cell_id,
            }) => {
                assert_eq!(cell.source, "");
                assert!(matches!(cell.cell_type, CellType::Code));
                assert_eq!(cell.label, "New Cell");
                assert!(cell.cargo_toml.is_none());
                assert!(after_cell_id.is_none());
            }
            other => panic!("expected CellAdd, got {other:?}"),
        }
    }

    #[test]
    fn translate_cells_add_full() {
        let req = ipc(
            "cells.add",
            json!({
                "source": "let x = 1;",
                "type": "markdown",
                "label": "My Cell",
                "after_cell_id": "cell-0",
                "cargo_toml": "[dependencies]",
            }),
        );
        let kind = translate_command(&req).unwrap();

        match kind {
            MessageKind::Mutation(protocol::Mutation::CellAdd {
                cell,
                after_cell_id,
            }) => {
                assert_eq!(cell.source, "let x = 1;");
                assert!(matches!(cell.cell_type, CellType::Markdown));
                assert_eq!(cell.label, "My Cell");
                assert_eq!(cell.cargo_toml.as_deref(), Some("[dependencies]"));
                assert_eq!(after_cell_id.as_deref(), Some("cell-0"));
            }
            other => panic!("expected CellAdd, got {other:?}"),
        }
    }

    #[test]
    fn translate_cells_update() {
        let req = ipc(
            "cells.update",
            json!({
                "cell_id": "c1",
                "source": "new code",
                "version": 3,
            }),
        );
        let kind = translate_command(&req).unwrap();

        match kind {
            MessageKind::Mutation(protocol::Mutation::CellUpdate {
                cell_id,
                source,
                cargo_toml,
                label,
                shared,
                collapsed,
                output_collapsed,
                version,
            }) => {
                assert_eq!(cell_id, "c1");
                assert_eq!(source.as_deref(), Some("new code"));
                assert!(cargo_toml.is_none());
                assert!(label.is_none());
                assert!(shared.is_none());
                assert!(collapsed.is_none());
                assert!(output_collapsed.is_none());
                assert_eq!(version, 3);
            }
            other => panic!("expected CellUpdate, got {other:?}"),
        }
    }

    #[test]
    fn translate_cells_update_missing_cell_id() {
        let req = ipc("cells.update", json!({"version": 1}));
        assert!(translate_command(&req).is_err());
    }

    #[test]
    fn translate_cells_update_missing_version() {
        let req = ipc("cells.update", json!({"cell_id": "c1"}));
        assert!(translate_command(&req).is_err());
    }

    #[test]
    fn translate_cells_delete() {
        let req = ipc("cells.delete", json!({"cell_id": "c1", "version": 5}));
        let kind = translate_command(&req).unwrap();

        match kind {
            MessageKind::Mutation(protocol::Mutation::CellDelete { cell_id, version }) => {
                assert_eq!(cell_id, "c1");
                assert_eq!(version, 5);
            }
            other => panic!("expected CellDelete, got {other:?}"),
        }
    }

    #[test]
    fn translate_cells_delete_missing_cell_id() {
        let req = ipc("cells.delete", json!({"version": 1}));
        assert!(translate_command(&req).is_err());
    }

    #[test]
    fn translate_cells_delete_missing_version() {
        let req = ipc("cells.delete", json!({"cell_id": "c1"}));
        assert!(translate_command(&req).is_err());
    }

    #[test]
    fn translate_notebook_update_tri_states() {
        // One request exercising all three wire states: set (title, shared
        // source), clear (explicit null on shared cargo), and untouched
        // (everything absent, e.g. description).
        let req = ipc(
            "notebook.update",
            json!({
                "meta": {
                    "title": "Renamed",
                    "shared_source": "pub fn from_cli() -> i32 { 9 }",
                    "shared_cargo_toml": null,
                }
            }),
        );
        let kind = translate_command(&req).unwrap();

        match kind {
            MessageKind::Mutation(protocol::Mutation::NotebookUpdateMeta { meta }) => {
                assert_eq!(meta.title.as_deref(), Some("Renamed"));
                assert_eq!(
                    meta.shared_source,
                    Some(Some("pub fn from_cli() -> i32 { 9 }".to_string()))
                );
                // Explicit null is a clear, not "unchanged".
                assert_eq!(meta.shared_cargo_toml, Some(None));
                // Absent fields stay untouched.
                assert!(meta.description.is_none());
                assert!(meta.tags.is_none());
            }
            other => panic!("expected NotebookUpdateMeta, got {other:?}"),
        }

        // No meta at all is a hard error, not an empty patch.
        let bad = ipc("notebook.update", json!({}));
        assert!(translate_command(&bad).is_err());
    }

    #[test]
    fn translate_cells_reorder() {
        let req = ipc("cells.reorder", json!({"cell_ids": ["c", "a", "b"]}));
        let kind = translate_command(&req).unwrap();

        match kind {
            MessageKind::Mutation(protocol::Mutation::CellReorder { cell_ids }) => {
                assert_eq!(cell_ids, vec!["c", "a", "b"]);
            }
            other => panic!("expected CellReorder, got {other:?}"),
        }
    }

    #[test]
    fn translate_cells_reorder_missing_ids_defaults_empty() {
        let req = ipc("cells.reorder", json!({}));
        let kind = translate_command(&req).unwrap();

        match kind {
            MessageKind::Mutation(protocol::Mutation::CellReorder { cell_ids }) => {
                assert!(cell_ids.is_empty());
            }
            other => panic!("expected CellReorder, got {other:?}"),
        }
    }

    #[test]
    fn translate_unknown_command() {
        let req = ipc("nope.nope", json!({}));
        let err = translate_command(&req).unwrap_err();
        assert!(err.contains("unknown command"), "got: {err}");
    }

    // ── translate_response ──────────────────────────────────────────────

    fn wrap(kind: MessageKind) -> protocol::Message {
        protocol::Message {
            id: "resp-1".into(),
            kind,
        }
    }

    #[test]
    fn response_notebook() {
        let nb: IronpadNotebook = serde_json::from_value(json!({
            "version": 1,
            "id": "00000000-0000-0000-0000-000000000000",
            "title": "Test",
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z",
            "cells": [],
        }))
        .unwrap();
        let resp = translate_response(wrap(MessageKind::Response(Response::Notebook {
            notebook: nb,
        })));
        assert!(resp.ok);
        assert_eq!(resp.data.as_ref().unwrap()["title"], "Test");
    }

    #[test]
    fn response_cell() {
        let cell = make_cell("c1", 0);
        let resp = translate_response(wrap(MessageKind::Response(Response::Cell {
            cell: cell.clone(),
        })));
        assert!(resp.ok);
        assert_eq!(resp.data.as_ref().unwrap()["id"], "c1");
    }

    #[test]
    fn response_cells_list() {
        let cells = vec![ironpad_common::types::CellManifest {
            id: "c1".into(),
            order: 0,
            label: "First".into(),
            cell_type: CellType::Code,
            shared: false,
            collapsed: false,
            output_collapsed: false,
        }];
        let resp = translate_response(wrap(MessageKind::Response(Response::CellsList { cells })));
        assert!(resp.ok);
        let arr = resp.data.unwrap();
        assert_eq!(arr.as_array().unwrap().len(), 1);
    }

    #[test]
    fn response_mutation_ok() {
        let resp = translate_response(wrap(MessageKind::Response(Response::MutationOk {
            detail: MutationResult::CellAdded {
                cell_id: "c1".into(),
                version: 1,
            },
        })));
        assert!(resp.ok);
        let data = resp.data.unwrap();
        assert_eq!(data["cell_id"], "c1");
    }

    #[test]
    fn response_error() {
        let resp = translate_response(wrap(MessageKind::Response(Response::Error {
            code: ErrorCode::PermissionDenied,
            message: "denied".into(),
        })));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_deref(), Some("denied"));
        assert_eq!(resp.code.as_deref(), Some("PermissionDenied"));
    }

    #[test]
    fn response_event_passthrough() {
        let resp = translate_response(wrap(MessageKind::Event(EventEnvelope {
            by: ClientId::browser(),
            event: Event::CellDeleted {
                cell_id: "c1".into(),
            },
        })));
        assert!(resp.ok);
    }

    #[test]
    fn response_unexpected_type() {
        let resp = translate_response(wrap(MessageKind::Query(Query::NotebookGet)));
        assert!(!resp.ok);
        assert!(resp.error.as_deref().unwrap().contains("unexpected"));
    }

    // ── shutdown cleanup (SIGTERM/SIGINT stop path) ──────────────────────

    #[tokio::test]
    async fn cleanup_removes_socket_and_pidfile() {
        // Exercises the exact routine the SIGTERM/SIGINT arms now invoke:
        // both the socket and the pidfile must be gone afterward.
        let base = std::env::temp_dir().join(format!("ironpad-cli-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&base).await.unwrap();
        let sock = base.join("daemon.sock");
        let pid = base.join("daemon.pid");
        tokio::fs::write(&sock, b"x").await.unwrap();
        tokio::fs::write(&pid, b"123").await.unwrap();
        assert!(sock.exists() && pid.exists());

        cleanup(&sock, &pid).await;

        assert!(!sock.exists(), "socket should be removed on cleanup");
        assert!(!pid.exists(), "pidfile should be removed on cleanup");
        let _ = tokio::fs::remove_dir_all(&base).await;
    }

    #[tokio::test]
    async fn shutdown_signal_resolves_on_sigterm() {
        use std::time::Duration;
        use tokio::signal::unix::{signal, SignalKind};

        // Delivering a real SIGTERM is only safe under nextest's
        // process-per-test isolation; under single-process `cargo test` it
        // would disturb sibling tests, so skip there. nextest sets `NEXTEST`.
        if std::env::var_os("NEXTEST").is_none() {
            return;
        }

        // Pre-install a SIGTERM handler so raising the signal never triggers the
        // default (process-terminating) disposition — it's merely delivered to
        // the installed handlers. Drain it in the background.
        let mut guard = signal(SignalKind::terminate()).expect("install guard handler");
        tokio::spawn(async move { while guard.recv().await.is_some() {} });

        let shutdown = tokio::spawn(shutdown_signal());

        let sig = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                // Safe: a SIGTERM handler is installed (guard above), so this is
                // a delivered signal, not a terminate. Repeat until the shutdown
                // future's own handler is installed and observes one.
                #[allow(unsafe_code)]
                unsafe {
                    libc::raise(libc::SIGTERM);
                }
                if shutdown.is_finished() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            shutdown.await.expect("shutdown task panicked")
        })
        .await
        .expect("shutdown_signal did not resolve after SIGTERM");

        assert_eq!(sig, "SIGTERM");
    }
}
