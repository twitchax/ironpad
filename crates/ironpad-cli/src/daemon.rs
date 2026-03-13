//! Long-lived daemon process.
//!
//! Maintains a WebSocket connection to the ironpad server and exposes
//! a Unix socket for fast CLI IPC. Caches notebook state locally so
//! read queries don't require a WebSocket round-trip.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio_tungstenite::tungstenite;

use ironpad_common::protocol::{self, MessageKind, Query};
use ironpad_common::IronpadNotebook;

use crate::ipc::{IpcRequest, IpcResponse};

// ── Paths ───────────────────────────────────────────────────────────────────

/// Directory for daemon runtime files (`~/.ironpad`).
pub fn daemon_dir() -> PathBuf {
    home_dir().join(".ironpad")
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .expect("HOME environment variable must be set")
}

pub fn socket_path() -> PathBuf {
    daemon_dir().join("daemon.sock")
}

pub fn pid_path() -> PathBuf {
    daemon_dir().join("daemon.pid")
}

// ── Daemon state ────────────────────────────────────────────────────────────

struct DaemonState {
    /// Cached notebook (populated on connect via NotebookGet query).
    notebook: RwLock<Option<IronpadNotebook>>,
    /// Pending request-response pairs keyed by message ID.
    pending: RwLock<HashMap<String, oneshot::Sender<String>>>,
    /// Channel to send messages over the WebSocket.
    ws_tx: mpsc::UnboundedSender<String>,
}

// ── Run daemon ──────────────────────────────────────────────────────────────

/// Start the daemon: connect to the server, listen on Unix socket, run forever.
pub async fn run(host: &str, token: &str) -> anyhow::Result<()> {
    // Ensure daemon directory exists.
    let dir = daemon_dir();
    tokio::fs::create_dir_all(&dir).await?;

    // Clean up stale socket.
    let sock = socket_path();
    if sock.exists() {
        tokio::fs::remove_file(&sock).await?;
    }

    // Write pidfile.
    let pid = pid_path();
    tokio::fs::write(&pid, std::process::id().to_string()).await?;

    // Connect WebSocket.
    let ws_url = format!("{host}/ws/connect?token={token}");
    tracing::info!(url = %ws_url, "connecting to server");

    let (ws_stream, _response) = tokio_tungstenite::connect_async(&ws_url).await?;
    tracing::info!("WebSocket connected");

    let (mut ws_sink, mut ws_source) = ws_stream.split();
    let (ws_tx, mut ws_rx) = mpsc::unbounded_channel::<String>();

    let state = Arc::new(DaemonState {
        notebook: RwLock::new(None),
        pending: RwLock::new(HashMap::new()),
        ws_tx: ws_tx.clone(),
    });

    // Request initial notebook state.
    let init_id = uuid::Uuid::new_v4().to_string();
    let init_msg = protocol::Message {
        id: init_id.clone(),
        kind: MessageKind::Query(Query::NotebookGet),
    };
    let (init_tx, init_rx) = oneshot::channel::<String>();
    state.pending.write().await.insert(init_id, init_tx);
    let _ = ws_tx.send(serde_json::to_string(&init_msg)?);

    // ── Task: forward channel → WebSocket ───────────────────────────────

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

    // ── Task: read WebSocket → dispatch ─────────────────────────────────

    let state_ws = Arc::clone(&state);
    let ws_recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_source.next().await {
            if let tungstenite::Message::Text(text) = msg {
                handle_ws_message(&text, &state_ws).await;
            }
        }
        tracing::info!("WebSocket closed");
    });

    // Wait for initial notebook fetch.
    if let Ok(response_text) =
        tokio::time::timeout(std::time::Duration::from_secs(10), init_rx).await
    {
        if let Ok(text) = response_text {
            if let Ok(msg) = serde_json::from_str::<protocol::Message>(&text) {
                if let MessageKind::Response(protocol::Response::Notebook { notebook }) = msg.kind {
                    *state.notebook.write().await = Some(notebook);
                    tracing::info!("notebook state cached");
                }
            }
        }
    } else {
        tracing::warn!("timed out waiting for initial notebook state");
    }

    // ── Task: Unix socket listener ──────────────────────────────────────

    let listener = UnixListener::bind(&sock)?;
    tracing::info!(path = %sock.display(), "listening on Unix socket");

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

    // ── Wait for shutdown ───────────────────────────────────────────────

    tokio::select! {
        _ = ws_send_task => tracing::info!("WS send task ended"),
        _ = ws_recv_task => tracing::info!("WS recv task ended"),
        _ = ipc_task => tracing::info!("IPC task ended"),
        _ = tokio::signal::ctrl_c() => tracing::info!("received SIGINT"),
    }

    // Cleanup.
    cleanup(&sock, &pid).await;
    Ok(())
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
        MessageKind::Response(_) => {
            let mut pending = state.pending.write().await;
            if let Some(tx) = pending.remove(&msg.id) {
                let _ = tx.send(text.to_string());
            }
        }

        // Event from the host browser — update local cache.
        MessageKind::Event(envelope) => {
            update_cache_from_event(&envelope.event, state).await;
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
        } => {
            if let Some(after_id) = after_cell_id {
                if let Some(idx) = nb.cells.iter().position(|c| c.id == *after_id) {
                    nb.cells.insert(idx + 1, cell.clone());
                } else {
                    nb.cells.push(cell.clone());
                }
            } else {
                nb.cells.insert(0, cell.clone());
            }
            renumber(&mut nb.cells);
        }
        protocol::Event::CellUpdated {
            cell_id,
            source,
            cargo_toml,
            label,
            version,
        } => {
            if let Some(cell) = nb.cells.iter_mut().find(|c| c.id == *cell_id) {
                if let Some(src) = source {
                    cell.source = src.clone();
                }
                if let Some(ct) = cargo_toml {
                    cell.cargo_toml = ct.clone();
                }
                if let Some(lbl) = label {
                    cell.label = lbl.clone();
                }
                cell.version = *version;
            }
        }
        protocol::Event::CellDeleted { cell_id } => {
            nb.cells.retain(|c| c.id != *cell_id);
            renumber(&mut nb.cells);
        }
        protocol::Event::CellReordered { cell_ids } => {
            let mut reordered = Vec::with_capacity(cell_ids.len());
            for id in cell_ids {
                if let Some(pos) = nb.cells.iter().position(|c| c.id == *id) {
                    reordered.push(nb.cells.remove(pos));
                }
            }
            reordered.append(&mut nb.cells);
            nb.cells = reordered;
            renumber(&mut nb.cells);
        }
        protocol::Event::NotebookMetaUpdated {
            title,
            shared_cargo_toml,
            shared_source,
        } => {
            if let Some(t) = title {
                nb.title = t.clone();
            }
            if let Some(sct) = shared_cargo_toml {
                nb.shared_cargo_toml = sct.clone();
            }
            if let Some(ss) = shared_source {
                nb.shared_source = ss.clone();
            }
        }
        // Compilation/execution events don't affect the notebook structure.
        protocol::Event::CellCompiling { .. }
        | protocol::Event::CellCompiled { .. }
        | protocol::Event::CellExecuted { .. }
        | protocol::Event::Error { .. } => {}
    }
}

fn renumber(cells: &mut [ironpad_common::IronpadCell]) {
    for (i, cell) in cells.iter_mut().enumerate() {
        cell.order = i as u32;
    }
}

// ── IPC connection handling ─────────────────────────────────────────────────

async fn handle_ipc_connection(stream: tokio::net::UnixStream, state: Arc<DaemonState>) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
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
        // ── Read commands (served from cache) ───────────────────────────
        "notebook.get" => {
            let nb = state.notebook.read().await;
            match nb.as_ref() {
                Some(notebook) => IpcResponse::success(serde_json::to_value(notebook).unwrap()),
                None => IpcResponse::error("no notebook cached"),
            }
        }
        "cells.list" => {
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
        "cells.get" => {
            let cell_id = req.args.get("cell_id").and_then(|v| v.as_str());
            let Some(cell_id) = cell_id else {
                return IpcResponse::error("missing cell_id argument");
            };
            let nb = state.notebook.read().await;
            match nb
                .as_ref()
                .and_then(|nb| nb.cells.iter().find(|c| c.id == cell_id))
            {
                Some(cell) => IpcResponse::success(serde_json::to_value(cell).unwrap()),
                None => IpcResponse::error_with_code("cell not found", "CellNotFound"),
            }
        }
        "status" => IpcResponse::success(serde_json::json!({
            "connected": true,
            "cached": state.notebook.read().await.is_some(),
        })),

        // ── Write commands (forwarded via WebSocket) ────────────────────
        _ => forward_to_server(&req, state).await,
    }
}

/// Forward an IPC command to the server via the WebSocket and wait for the response.
async fn forward_to_server(req: &IpcRequest, state: &DaemonState) -> IpcResponse {
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
        Ok(j) => j,
        Err(e) => return IpcResponse::error(format!("serialization error: {e}")),
    };

    // Set up response channel.
    let (tx, rx) = oneshot::channel::<String>();
    state.pending.write().await.insert(msg_id.clone(), tx);

    // Send over WebSocket.
    if state.ws_tx.send(json).is_err() {
        state.pending.write().await.remove(&msg_id);
        return IpcResponse::error("WebSocket disconnected");
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
        Ok(Err(_)) => IpcResponse::error("response channel closed"),
        Err(_) => IpcResponse::error("request timed out"),
    }
}

/// Translate an IPC command name + args into a protocol MessageKind.
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
                .unwrap_or("New Cell")
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

            Ok(MessageKind::Mutation(protocol::Mutation::CellAdd {
                cell: protocol::NewCell {
                    source,
                    cell_type,
                    label,
                    cargo_toml,
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
            let version = req
                .args
                .get("version")
                .and_then(|v| v.as_u64())
                .ok_or("missing version (required for optimistic concurrency)")?;

            Ok(MessageKind::Mutation(protocol::Mutation::CellUpdate {
                cell_id,
                source,
                cargo_toml,
                label,
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
                .and_then(|v| v.as_u64())
                .ok_or("missing version (required for optimistic concurrency)")?;

            Ok(MessageKind::Mutation(protocol::Mutation::CellDelete {
                cell_id,
                version,
            }))
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
        cmd => Err(format!("unknown command: {cmd}")),
    }
}

/// Translate a protocol response message into an IPC response.
fn translate_response(msg: protocol::Message) -> IpcResponse {
    match msg.kind {
        MessageKind::Response(response) => match response {
            protocol::Response::Notebook { notebook } => {
                IpcResponse::success(serde_json::to_value(notebook).unwrap())
            }
            protocol::Response::Cell { cell } => {
                IpcResponse::success(serde_json::to_value(cell).unwrap())
            }
            protocol::Response::CellsList { cells } => {
                IpcResponse::success(serde_json::to_value(cells).unwrap())
            }
            protocol::Response::SessionStatus {
                session_id,
                connected_clients,
            } => IpcResponse::success(serde_json::json!({
                "session_id": session_id,
                "connected_clients": connected_clients,
            })),
            protocol::Response::MutationOk { detail } => {
                IpcResponse::success(serde_json::to_value(detail).unwrap())
            }
            protocol::Response::Error { code, message } => {
                IpcResponse::error_with_code(message, format!("{code:?}"))
            }
        },
        MessageKind::Event(envelope) => {
            IpcResponse::success(serde_json::to_value(envelope).unwrap())
        }
        _ => IpcResponse::error("unexpected response type"),
    }
}
