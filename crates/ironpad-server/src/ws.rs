//! WebSocket relay handlers.
//!
//! The server is a dumb relay — it routes messages between browser hosts
//! and CLI guests, enforcing session/token validation and permissions.
//! It never interprets notebook state.

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;

use ironpad_common::protocol::{
    self, ClientId, ControlMessage, ErrorCode, MessageKind, Permissions, Response,
};

use crate::sessions::{check_permission, ValidateError};
use crate::state::{AppState, ClaimOutcome, HostDelivery};

/// Serialize a protocol message to JSON for the wire.
fn wire_msg(id: &str, kind: MessageKind) -> String {
    serde_json::to_string(&protocol::Message {
        id: id.to_string(),
        kind,
    })
    .expect("protocol message serialization should never fail")
}

/// Cap on a single WebSocket message so a malicious peer can't send a giant
/// frame that balloons server memory. Generous for protocol messages (a single
/// mutation/event carries at most one cell's source + metadata).
const MAX_WS_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// Per-connection outbound queue depth. Bounding it (vs. an unbounded channel)
/// caps memory when a peer stops reading: once full, `try_send` drops the
/// message rather than buffering without limit (the idle timeout eventually
/// reaps a stuck connection).
const WS_CHANNEL_BOUND: usize = 1024;

/// How long to wait for a host's first frame (its `ClaimHost`). A client that
/// connects and never speaks is reaped so it can't hold the socket open
/// (PRD-0038 T-014).
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

// ── Query parameters ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct HostParams {
    pub notebook_id: String,
}

#[derive(Deserialize)]
pub struct GuestParams {
    pub token: String,
}

// ── Host handler ────────────────────────────────────────────────────────────

/// WebSocket upgrade for browser hosts: `GET /ws/host?notebook_id=<id>`.
#[allow(clippy::unused_async)] // Axum handler signature requires async.
pub async fn ws_host_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<HostParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.max_message_size(MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| handle_host(socket, params.notebook_id, state))
}

async fn handle_host(socket: WebSocket, notebook_id: String, state: AppState) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let connection_id = uuid::Uuid::new_v4().to_string();

    // ── Handshake (PRD-0038 T-014) ──────────────────────────────────────────
    // The first frame MUST be a `ClaimHost` proving the browser holds this
    // notebook's host secret — validated BEFORE we register as host. A missing
    // or malformed first frame closes 4400; a secret mismatch closes 4403 and
    // leaves the incumbent host (and its sessions) untouched. A client that
    // connects and never speaks is reaped by the handshake timeout.
    let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, ws_receiver.next()).await;
    let claim = first
        .ok()
        .flatten()
        .and_then(Result::ok)
        .and_then(|m| match m {
            Message::Text(t) => parse_claim_secret(&t),
            _ => None,
        });
    let Some(secret) = claim else {
        close_host(&mut ws_sender, 4400, "expected ClaimHost first frame").await;
        return;
    };
    match state.ws.claim_host(&notebook_id, &secret).await {
        ClaimOutcome::Accepted { tofu } => {
            tracing::info!(notebook_id = %notebook_id, connection_id = %connection_id, tofu, "host claim accepted");
        }
        ClaimOutcome::Rejected => {
            tracing::warn!(notebook_id = %notebook_id, "host claim rejected — secret mismatch");
            close_host(&mut ws_sender, 4403, "host claim rejected").await;
            return;
        }
    }

    let (tx, mut rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
    tracing::info!(
        notebook_id = %notebook_id,
        connection_id = %connection_id,
        "host connected"
    );

    state
        .ws
        .register_host(&notebook_id, &connection_id, tx)
        .await;

    // Forward channel → WebSocket.
    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // Read WebSocket → process.
    let nb_id = notebook_id.clone();
    let conn_id = connection_id.clone();
    let st = state.clone();
    let mut recv_task = tokio::spawn(async move {
        // Idle timeout: the browser host sends a Heartbeat every ~30s, so 120s
        // of total silence means a half-open connection (a network drop without
        // a FIN) — tear it down so the host doesn't stay registered forever.
        const HOST_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
        loop {
            let next = tokio::time::timeout(HOST_IDLE_TIMEOUT, ws_receiver.next()).await;
            // Stream end (`Ok(None)`), a WS error (`Ok(Some(Err))`), or the idle
            // timeout (`Err`) all mean we're done reading from this connection.
            let Ok(Some(Ok(msg))) = next else {
                if next.is_err() {
                    tracing::warn!(
                        notebook_id = %nb_id,
                        "host idle timeout — closing half-open connection"
                    );
                }
                break;
            };
            match msg {
                Message::Text(text) => {
                    handle_host_message(&text, &nb_id, &conn_id, &st).await;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // Wait for either side to finish.
    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }

    // Cleanup: unregister host (only if we're still the registered connection —
    // a reconnect may have replaced us), invalidate sessions, disconnect guests.
    tracing::info!(notebook_id = %notebook_id, "host disconnected");
    state.ws.unregister_host(&notebook_id, &connection_id).await;

    let removed = state
        .ws
        .sessions
        .invalidate_by_connection(&connection_id)
        .await;
    for session_id in &removed {
        let close_msg = wire_msg(
            "",
            MessageKind::Control(ControlMessage::SessionEnded {
                session_id: session_id.clone(),
            }),
        );
        state.ws.broadcast_to_guests(session_id, &close_msg).await;
        state.ws.disconnect_guests(session_id).await;
    }

    // Forget the host secret if the notebook is now idle (no host, no sessions),
    // bounding the map and letting a fresh TOFU claim take over later.
    state.ws.forget_secret_if_idle(&notebook_id).await;
}

/// Parse a first-frame `ClaimHost`, returning its secret if that's what it is.
fn parse_claim_secret(text: &str) -> Option<String> {
    match serde_json::from_str::<protocol::Message>(text).ok()?.kind {
        MessageKind::Control(ControlMessage::ClaimHost { secret }) => Some(secret),
        _ => None,
    }
}

/// Close a host socket with an application close code + reason before dropping it.
async fn close_host(
    ws_sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    code: u16,
    reason: &str,
) {
    use axum::extract::ws::CloseFrame;
    let _ = ws_sender
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.to_string().into(),
        })))
        .await;
}

/// Process a message from the host browser.
async fn handle_host_message(text: &str, notebook_id: &str, connection_id: &str, state: &AppState) {
    let Ok(msg) = serde_json::from_str::<ironpad_common::protocol::Message>(text) else {
        tracing::warn!("host sent invalid JSON");
        return;
    };

    match &msg.kind {
        // Host sends events → broadcast to all *readable* guests for this
        // notebook. A guest-originated mutation acks via its Event (which carries
        // the mutation id), so resolving that id here clears the tracked entry
        // (the error Response counterpart won't fire) AND names the originating
        // guest. Browser-originated events carry an empty id and are never
        // tracked, so skip the write-lock acquisition for them (review L4).
        MessageKind::Event(_) => {
            let origin = if msg.id.is_empty() {
                None
            } else {
                state.ws.resolve_query(&msg.id).await
            };

            state
                .ws
                .broadcast_to_notebook_guests(notebook_id, text)
                .await;

            // The read gate above skips a write-only (read: false) originator, so
            // its mutation's only ack would be swallowed and its client would
            // strand on the 10s request timeout despite the mutation succeeding.
            // Deliver the ack to such an originator directly (review M2). This is
            // not a content leak: the event reflects only the mutation this guest
            // itself just sent — plus the model-assigned id/version it needs for
            // follow-ups — never another client's edits, which stay gated.
            if let Some(origin_client) = origin {
                if !state.ws.guest_can_read(&origin_client).await {
                    state.ws.send_to_guest(&origin_client, text).await;
                }
            }
        }

        // Host sends a response → route to the guest that sent the query.
        MessageKind::Response(_) => {
            if let Some(client_id) = state.ws.resolve_query(&msg.id).await {
                state.ws.send_to_guest(&client_id, text).await;
            }
        }

        // Host sends control messages → handle session lifecycle.
        MessageKind::Control(control) => {
            handle_host_control(control, notebook_id, connection_id, &msg.id, state).await;
        }

        // Hosts don't send mutations or queries through this path.
        MessageKind::Mutation(_) | MessageKind::Query(_) => {
            tracing::debug!("host sent unexpected mutation/query, ignoring");
        }
    }
}

/// Handle control messages from the host.
async fn handle_host_control(
    control: &ControlMessage,
    notebook_id: &str,
    connection_id: &str,
    msg_id: &str,
    state: &AppState,
) {
    match control {
        ControlMessage::CreateSession { permissions } => {
            let result = state
                .ws
                .sessions
                .create_session(
                    notebook_id.to_string(),
                    connection_id.to_string(),
                    permissions.clone(),
                )
                .await;

            tracing::info!(
                session_id = %result.session_id,
                notebook_id = %notebook_id,
                "session created"
            );

            let response = wire_msg(
                msg_id,
                MessageKind::Control(ControlMessage::SessionCreated {
                    session_id: result.session_id,
                    token: result.token,
                }),
            );
            state.ws.send_to_host(notebook_id, &response).await;
        }

        ControlMessage::EndSession { session_id } => {
            tracing::info!(session_id = %session_id, "session ended by host");
            state.ws.sessions.invalidate_session(session_id).await;

            let close_msg = wire_msg(
                msg_id,
                MessageKind::Control(ControlMessage::SessionEnded {
                    session_id: session_id.clone(),
                }),
            );
            state.ws.broadcast_to_guests(session_id, &close_msg).await;
            state.ws.disconnect_guests(session_id).await;
        }

        // Host keep-alive (Heartbeat) is a no-op here — simply receiving it
        // resets the read-loop idle timer that reaps half-open connections. The
        // remaining variants are server-originated and never host-sent.
        // ClaimHost is only meaningful as the first frame (consumed by the
        // handshake in handle_host); a later one is ignored.
        ControlMessage::Heartbeat
        | ControlMessage::SessionCreated { .. }
        | ControlMessage::SessionEnded { .. }
        | ControlMessage::GuestConnected { .. }
        | ControlMessage::GuestDisconnected { .. }
        | ControlMessage::ClaimHost { .. }
        | ControlMessage::Unknown => {}
    }
}

// ── Guest handler ───────────────────────────────────────────────────────────

/// WebSocket upgrade for CLI guests: `GET /ws/connect?token=<token>`.
///
/// Validates the token before upgrading. Returns HTTP 401 if invalid, HTTP 410
/// if expired, and HTTP 503 if the global guest connection cap is reached.
pub async fn ws_connect_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<GuestParams>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    let session = state
        .ws
        .sessions
        .validate_token(&params.token)
        .await
        .map_err(|e| match e {
            ValidateError::InvalidToken => StatusCode::UNAUTHORIZED,
            ValidateError::SessionExpired => StatusCode::GONE,
        })?;

    // Enforce the global guest connection cap before upgrading, so a connection
    // flood can't exhaust server memory/tasks. A small TOCTOU overshoot under
    // concurrent connects is acceptable — the cap bounds steady-state, not a
    // single simultaneous burst.
    if !state.ws.guest_slot_available().await {
        tracing::warn!(session_id = %session.id, "guest connection cap reached — rejecting");
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let session_id = session.id.clone();
    let notebook_id = session.notebook_id.clone();
    let permissions = session.permissions.clone();
    // Make the client id unique per connection (not just the token prefix) so two
    // connections on the same token don't collide — a collision misroutes query
    // responses (delivered to the first match) and lets either disconnect strand
    // the other (retain removes both handles).
    let token_prefix = params.token.get(..8).unwrap_or(&params.token);
    let client_id = format!(
        "{}-{}",
        ClientId::agent(token_prefix).0,
        uuid::Uuid::new_v4()
    );

    Ok(ws
        .max_message_size(MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| {
            handle_guest(
                socket,
                session_id,
                notebook_id,
                client_id,
                permissions,
                state,
            )
        }))
}

async fn handle_guest(
    socket: WebSocket,
    session_id: String,
    notebook_id: String,
    client_id: String,
    permissions: Permissions,
    state: AppState,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);

    tracing::info!(
        session_id = %session_id,
        client_id = %client_id,
        "guest connected"
    );

    state.ws.register_guest(&session_id, &client_id, tx).await;

    // Notify host that a guest connected.
    let connected_msg = wire_msg(
        "",
        MessageKind::Control(ControlMessage::GuestConnected {
            client_id: ClientId(client_id.clone()),
        }),
    );
    state.ws.send_to_host(&notebook_id, &connected_msg).await;

    // Forward channel → WebSocket.
    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // Read WebSocket → process.
    let cid = client_id.clone();
    let sid = session_id.clone();
    let nid = notebook_id.clone();
    let perms = permissions;
    let st = state.clone();
    let idle_timeout = state.ws.guest_idle_timeout();
    let mut recv_task = tokio::spawn(async move {
        // Idle timeout: reap a half-open guest (a network drop with no FIN).
        // Guests don't heartbeat, so this window is generous (see `WsState`);
        // any inbound frame resets it.
        loop {
            let next = tokio::time::timeout(idle_timeout, ws_receiver.next()).await;
            // Stream end (`Ok(None)`), a WS error (`Ok(Some(Err))`), or the idle
            // timeout (`Err`) all mean we're done reading this connection.
            let Ok(Some(Ok(msg))) = next else {
                if next.is_err() {
                    tracing::warn!(
                        client_id = %cid,
                        "guest idle timeout — closing half-open connection"
                    );
                }
                break;
            };
            match msg {
                Message::Text(text) => {
                    handle_guest_message(&text, &nid, &sid, &cid, &perms, &st).await;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }

    // Cleanup.
    tracing::info!(client_id = %client_id, "guest disconnected");
    state.ws.unregister_guest(&session_id, &client_id).await;

    // Notify host.
    let disconnected_msg = wire_msg(
        "",
        MessageKind::Control(ControlMessage::GuestDisconnected {
            client_id: ClientId(client_id),
        }),
    );
    state.ws.send_to_host(&notebook_id, &disconnected_msg).await;
}

/// Process a message from a CLI guest.
async fn handle_guest_message(
    text: &str,
    notebook_id: &str,
    _session_id: &str,
    client_id: &str,
    permissions: &Permissions,
    state: &AppState,
) {
    let Ok(msg) = serde_json::from_str::<ironpad_common::protocol::Message>(text) else {
        tracing::warn!(client_id = %client_id, "guest sent invalid JSON");
        return;
    };

    // Permission check.
    if !check_permission(permissions, &msg.kind) {
        let error = wire_msg(
            &msg.id,
            MessageKind::Response(Response::Error {
                code: ErrorCode::PermissionDenied,
                message: "Insufficient permissions for this operation".into(),
            }),
        );
        state.ws.send_to_guest(client_id, &error).await;
        return;
    }

    match &msg.kind {
        // Guest sends a mutation or query → forward to host and track the id so
        // the host's response routes back to this guest. For a mutation the
        // tracked entry routes a Response::Error (e.g. a version conflict) and is
        // cleared when the success Event is broadcast (see handle_host_message);
        // for a query it routes the Response.
        MessageKind::Mutation(_) | MessageKind::Query(_) => {
            state.ws.track_query(&msg.id, client_id).await;
            match state.ws.send_to_host(notebook_id, text).await {
                HostDelivery::Delivered => {}
                // Host alive but its outbound queue is saturated; the frame was
                // dropped. Don't misreport this live host as gone (review L2):
                // untrack and let the guest's own request timeout surface it
                // honestly as a timeout rather than a fabricated disconnect.
                HostDelivery::Full => {
                    tracing::warn!(
                        client_id = %client_id,
                        "host channel full — dropping guest frame under backpressure"
                    );
                    state.ws.resolve_query(&msg.id).await;
                }
                // No host is connected — fail fast so the guest isn't left
                // waiting on the request timeout.
                HostDelivery::Disconnected => {
                    state.ws.resolve_query(&msg.id).await;
                    let err = wire_msg(
                        &msg.id,
                        MessageKind::Response(Response::Error {
                            code: ErrorCode::SessionNotFound,
                            message: "host disconnected".into(),
                        }),
                    );
                    state.ws.send_to_guest(client_id, &err).await;
                }
            }
        }

        // Guests don't send events, responses, or control messages.
        MessageKind::Event(_) | MessageKind::Response(_) | MessageKind::Control(_) => {
            tracing::debug!(client_id = %client_id, "guest sent unexpected message type");
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ironpad_common::protocol::{
        self, ClientId, ControlMessage, ErrorCode, Event, EventEnvelope, MessageKind, Mutation,
        NewCell, Permissions, Query, Response,
    };
    use ironpad_common::types::CellType;
    use ironpad_common::AppConfig;
    use leptos::config::LeptosOptions;
    use tokio::sync::mpsc;

    use crate::state::{AppState, WsState};

    use super::{handle_guest_message, handle_host_message, wire_msg, WS_CHANNEL_BOUND};

    /// Build a minimal `AppState` suitable for WS handler tests.
    fn test_state() -> AppState {
        AppState {
            leptos_options: LeptosOptions::builder().output_name("ironpad-test").build(),
            config: AppConfig {
                data_dir: PathBuf::from("/tmp"),
                cache_dir: PathBuf::from("/tmp"),
                port: 0,
                ironpad_cell_path: PathBuf::from("/tmp"),
                compilation_proxy: None,
            },
            ws: WsState::default(),
        }
    }

    // ── Host message tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn host_event_broadcasts_to_guests() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        // Register host.
        let (host_tx, _host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        // Create a session so `broadcast_to_notebook_guests` can find guests.
        let session = state
            .ws
            .sessions
            .create_session(nb.into(), conn.into(), Permissions::default())
            .await;

        // Register a guest on that session.
        let (guest_tx, mut guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        // Host sends an event.
        let event_json = wire_msg(
            "evt-1",
            MessageKind::Event(EventEnvelope {
                by: ClientId::browser(),
                event: Event::CellDeleted {
                    cell_id: "c1".into(),
                },
            }),
        );

        handle_host_message(&event_json, nb, conn, &state).await;

        // Guest should receive the broadcast.
        let received = guest_rx.try_recv().expect("guest should receive event");
        let msg: protocol::Message = serde_json::from_str(&received).unwrap();
        assert!(matches!(msg.kind, MessageKind::Event(_)));
    }

    #[tokio::test]
    async fn host_response_routes_to_querying_guest() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        let (host_tx, _host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        let session = state
            .ws
            .sessions
            .create_session(nb.into(), conn.into(), Permissions::default())
            .await;

        let (guest_tx, mut guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        // Simulate a tracked query from guest-1 with message id "q-42".
        state.ws.track_query("q-42", "guest-1").await;

        // Host sends a response with matching id.
        let response_json = wire_msg(
            "q-42",
            MessageKind::Response(Response::CellsList { cells: vec![] }),
        );

        handle_host_message(&response_json, nb, conn, &state).await;

        // Guest should receive the routed response.
        let received = guest_rx.try_recv().expect("guest should receive response");
        let msg: protocol::Message = serde_json::from_str(&received).unwrap();
        assert_eq!(msg.id, "q-42");
        assert!(matches!(
            msg.kind,
            MessageKind::Response(Response::CellsList { .. })
        ));
    }

    #[tokio::test]
    async fn host_response_untracked_is_dropped() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        let (host_tx, _host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        let session = state
            .ws
            .sessions
            .create_session(nb.into(), conn.into(), Permissions::default())
            .await;

        let (guest_tx, mut guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        // No tracked query — response with unknown id should be dropped.
        let response_json = wire_msg(
            "unknown-id",
            MessageKind::Response(Response::CellsList { cells: vec![] }),
        );

        handle_host_message(&response_json, nb, conn, &state).await;

        assert!(
            guest_rx.try_recv().is_err(),
            "guest should NOT receive untracked response"
        );
    }

    #[tokio::test]
    async fn host_mutation_is_ignored() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        let (host_tx, _host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        let session = state
            .ws
            .sessions
            .create_session(nb.into(), conn.into(), Permissions::default())
            .await;

        let (guest_tx, mut guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        // Hosts should not send mutations; they should be silently ignored.
        let mutation_json = wire_msg(
            "m-1",
            MessageKind::Mutation(Mutation::CellReorder { cell_ids: vec![] }),
        );

        handle_host_message(&mutation_json, nb, conn, &state).await;

        assert!(
            guest_rx.try_recv().is_err(),
            "unexpected mutation from host should not be forwarded"
        );
    }

    // ── Guest mutation routing (C2) ─────────────────────────────────────

    #[tokio::test]
    async fn guest_mutation_error_routes_back_to_guest() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        let (host_tx, mut host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        let session = state
            .ws
            .sessions
            .create_session(nb.into(), conn.into(), Permissions::default())
            .await;

        let (guest_tx, mut guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        // Guest sends a mutation → forwarded to host and tracked for routing.
        let mutation_json = wire_msg(
            "m-1",
            MessageKind::Mutation(Mutation::CellReorder { cell_ids: vec![] }),
        );
        handle_guest_message(
            &mutation_json,
            nb,
            &session.session_id,
            "guest-1",
            &Permissions::default(),
            &state,
        )
        .await;
        assert!(
            host_rx.try_recv().is_ok(),
            "host should receive the mutation"
        );

        // Host replies with an error for that mutation id (e.g. a version conflict).
        let error_json = wire_msg(
            "m-1",
            MessageKind::Response(Response::Error {
                code: ErrorCode::VersionConflict,
                message: "version conflict".into(),
            }),
        );
        handle_host_message(&error_json, nb, conn, &state).await;

        // The error must route back to the originating guest (dropped before C2).
        let received = guest_rx.try_recv().expect("guest should receive the error");
        let msg: protocol::Message = serde_json::from_str(&received).unwrap();
        assert_eq!(msg.id, "m-1");
        assert!(matches!(
            msg.kind,
            MessageKind::Response(Response::Error { .. })
        ));
    }

    #[tokio::test]
    async fn host_event_clears_tracked_mutation() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        let (host_tx, _host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        let session = state
            .ws
            .sessions
            .create_session(nb.into(), conn.into(), Permissions::default())
            .await;
        let (guest_tx, _guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        // A mutation is in flight (tracked).
        state.ws.track_query("m-1", "guest-1").await;

        // The host broadcasts the mutation's success Event (carrying its id).
        let event_json = wire_msg(
            "m-1",
            MessageKind::Event(EventEnvelope {
                by: ClientId::agent("guest-1"),
                event: Event::CellDeleted {
                    cell_id: "c1".into(),
                },
            }),
        );
        handle_host_message(&event_json, nb, conn, &state).await;

        // The tracked entry must have been cleared (no leak on the success path).
        assert_eq!(state.ws.resolve_query("m-1").await, None);
    }

    #[tokio::test]
    async fn write_only_originator_receives_mutation_ack_but_not_foreign_events() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        let (host_tx, _host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        // A write-only (read: false) "blind" agent session.
        let session = state
            .ws
            .sessions
            .create_session(
                nb.into(),
                conn.into(),
                Permissions {
                    read: false,
                    write: true,
                },
            )
            .await;
        let (guest_tx, mut guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        // The guest's mutation "m-1" is in flight (tracked for routing).
        state.ws.track_query("m-1", "guest-1").await;

        // The host echoes the mutation's success Event, carrying its id.
        let ack = wire_msg(
            "m-1",
            MessageKind::Event(EventEnvelope {
                by: ClientId::agent("guest-1"),
                event: Event::CellDeleted {
                    cell_id: "c1".into(),
                },
            }),
        );
        handle_host_message(&ack, nb, conn, &state).await;

        // The write-only originator receives its ack despite the read gate —
        // otherwise its client would strand on the 10s request timeout. (The
        // read-gated broadcast alone would have skipped this session.)
        let received = guest_rx
            .try_recv()
            .expect("write-only originator should receive its own mutation ack");
        let msg: protocol::Message = serde_json::from_str(&received).unwrap();
        assert_eq!(msg.id, "m-1");
        assert!(matches!(msg.kind, MessageKind::Event(_)));

        // A foreign, browser-originated content event (empty id, untracked) must
        // NOT reach the write-only guest — the confidentiality boundary holds for
        // everything it did not itself originate.
        let foreign = wire_msg(
            "",
            MessageKind::Event(EventEnvelope {
                by: ClientId::browser(),
                event: Event::CellDeleted {
                    cell_id: "c2".into(),
                },
            }),
        );
        handle_host_message(&foreign, nb, conn, &state).await;
        assert!(
            guest_rx.try_recv().is_err(),
            "write-only guest must not receive events it did not originate"
        );
    }

    // ── Control message tests ───────────────────────────────────────────

    #[tokio::test]
    async fn host_create_session_sends_session_created() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        let (host_tx, mut host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        let create_msg = wire_msg(
            "ctrl-1",
            MessageKind::Control(ControlMessage::CreateSession {
                permissions: Permissions::default(),
            }),
        );

        handle_host_message(&create_msg, nb, conn, &state).await;

        // Host should receive a SessionCreated response.
        let received = host_rx
            .try_recv()
            .expect("host should receive SessionCreated");
        let msg: protocol::Message = serde_json::from_str(&received).unwrap();
        assert_eq!(msg.id, "ctrl-1");
        assert!(matches!(
            msg.kind,
            MessageKind::Control(ControlMessage::SessionCreated { .. })
        ));

        // Session should exist in the store.
        let sessions = state.ws.sessions.all_sessions().await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].notebook_id, nb);
    }

    #[tokio::test]
    async fn host_end_session_broadcasts_and_disconnects() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        let (host_tx, _host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        let session = state
            .ws
            .sessions
            .create_session(nb.into(), conn.into(), Permissions::default())
            .await;

        let (guest_tx, mut guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        // Host ends the session.
        let end_msg = wire_msg(
            "ctrl-2",
            MessageKind::Control(ControlMessage::EndSession {
                session_id: session.session_id.clone(),
            }),
        );

        handle_host_message(&end_msg, nb, conn, &state).await;

        // Guest should receive SessionEnded.
        let received = guest_rx
            .try_recv()
            .expect("guest should receive SessionEnded");
        let msg: protocol::Message = serde_json::from_str(&received).unwrap();
        assert!(matches!(
            msg.kind,
            MessageKind::Control(ControlMessage::SessionEnded { .. })
        ));

        // Session should be invalidated.
        assert!(state
            .ws
            .sessions
            .get_session(&session.session_id)
            .await
            .is_none());
    }

    // ── Guest message tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn guest_mutation_forwarded_to_host() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        let (host_tx, mut host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        let session = state
            .ws
            .sessions
            .create_session(nb.into(), conn.into(), Permissions::default())
            .await;

        let (guest_tx, _guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        let perms = Permissions {
            read: true,
            write: true,
        };

        let mutation_json = wire_msg(
            "m-1",
            MessageKind::Mutation(Mutation::CellAdd {
                cell: NewCell {
                    source: "let x = 1;".into(),
                    cell_type: CellType::Code,
                    label: "Test".into(),
                    cargo_toml: None,
                },
                after_cell_id: None,
            }),
        );

        handle_guest_message(
            &mutation_json,
            nb,
            &session.session_id,
            "guest-1",
            &perms,
            &state,
        )
        .await;

        // Host should receive the forwarded mutation.
        let received = host_rx.try_recv().expect("host should receive mutation");
        let msg: protocol::Message = serde_json::from_str(&received).unwrap();
        assert_eq!(msg.id, "m-1");
        assert!(matches!(
            msg.kind,
            MessageKind::Mutation(Mutation::CellAdd { .. })
        ));
    }

    #[tokio::test]
    async fn guest_query_forwarded_and_tracked() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        let (host_tx, mut host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        let session = state
            .ws
            .sessions
            .create_session(nb.into(), conn.into(), Permissions::default())
            .await;

        let (guest_tx, _guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        let perms = Permissions {
            read: true,
            write: false,
        };

        let query_json = wire_msg("q-1", MessageKind::Query(Query::CellsList));

        handle_guest_message(
            &query_json,
            nb,
            &session.session_id,
            "guest-1",
            &perms,
            &state,
        )
        .await;

        // Host should receive the forwarded query.
        let received = host_rx.try_recv().expect("host should receive query");
        let msg: protocol::Message = serde_json::from_str(&received).unwrap();
        assert_eq!(msg.id, "q-1");
        assert!(matches!(msg.kind, MessageKind::Query(Query::CellsList)));

        // Query should be tracked for response routing.
        let tracked = state.ws.resolve_query("q-1").await;
        assert_eq!(tracked.as_deref(), Some("guest-1"));
    }

    #[tokio::test]
    async fn guest_mutation_denied_without_write() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        let (host_tx, mut host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        let session = state
            .ws
            .sessions
            .create_session(nb.into(), conn.into(), Permissions::default())
            .await;

        let (guest_tx, mut guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        // Read-only permissions — no write.
        let perms = Permissions {
            read: true,
            write: false,
        };

        let mutation_json = wire_msg(
            "m-1",
            MessageKind::Mutation(Mutation::CellReorder { cell_ids: vec![] }),
        );

        handle_guest_message(
            &mutation_json,
            nb,
            &session.session_id,
            "guest-1",
            &perms,
            &state,
        )
        .await;

        // Host should NOT receive it.
        assert!(
            host_rx.try_recv().is_err(),
            "host should NOT receive permission-denied mutation"
        );

        // Guest should receive a PermissionDenied error.
        let received = guest_rx
            .try_recv()
            .expect("guest should receive error response");
        let msg: protocol::Message = serde_json::from_str(&received).unwrap();
        assert_eq!(msg.id, "m-1");
        assert!(matches!(
            msg.kind,
            MessageKind::Response(Response::Error {
                code: ironpad_common::protocol::ErrorCode::PermissionDenied,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn guest_query_denied_without_read() {
        let state = test_state();
        let nb = "nb-1";

        let (host_tx, mut host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, "conn-1", host_tx).await;

        let session = state
            .ws
            .sessions
            .create_session(nb.into(), "conn-1".into(), Permissions::default())
            .await;

        let (guest_tx, mut guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        let perms = Permissions {
            read: false,
            write: true,
        };

        let query_json = wire_msg("q-1", MessageKind::Query(Query::NotebookGet));

        handle_guest_message(
            &query_json,
            nb,
            &session.session_id,
            "guest-1",
            &perms,
            &state,
        )
        .await;

        assert!(host_rx.try_recv().is_err(), "host should not receive query");

        let received = guest_rx.try_recv().expect("guest should receive error");
        let msg: protocol::Message = serde_json::from_str(&received).unwrap();
        assert!(matches!(
            msg.kind,
            MessageKind::Response(Response::Error {
                code: ironpad_common::protocol::ErrorCode::PermissionDenied,
                ..
            })
        ));
    }

    // ── Invalid JSON handling ───────────────────────────────────────────

    #[tokio::test]
    async fn host_invalid_json_does_not_crash() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        let (host_tx, _host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        // Malformed JSON — should be silently ignored.
        handle_host_message("{not valid json", nb, conn, &state).await;
    }

    #[tokio::test]
    async fn guest_invalid_json_does_not_crash() {
        let state = test_state();

        let perms = Permissions::default();

        // Malformed JSON — should be silently ignored.
        handle_guest_message("{bad", "nb-1", "sess-1", "guest-1", &perms, &state).await;
    }

    #[tokio::test]
    async fn host_structurally_valid_but_wrong_schema_does_not_crash() {
        let state = test_state();
        let nb = "nb-1";
        let conn = "conn-1";

        let (host_tx, _host_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state.ws.register_host(nb, conn, host_tx).await;

        // Valid JSON but missing required protocol fields.
        handle_host_message(r#"{"foo": "bar"}"#, nb, conn, &state).await;
    }

    // ── Edge cases ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn guest_mutation_when_host_disconnected() {
        let state = test_state();
        let nb = "nb-1";

        // No host registered — guest sends mutation.
        let session = state
            .ws
            .sessions
            .create_session(nb.into(), "conn-1".into(), Permissions::default())
            .await;

        let (guest_tx, mut guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        let perms = Permissions {
            read: true,
            write: true,
        };

        let mutation_json = wire_msg(
            "m-1",
            MessageKind::Mutation(Mutation::CellReorder { cell_ids: vec![] }),
        );

        handle_guest_message(
            &mutation_json,
            nb,
            &session.session_id,
            "guest-1",
            &perms,
            &state,
        )
        .await;

        // Guest should receive a SessionNotFound error.
        let received = guest_rx
            .try_recv()
            .expect("guest should receive error when host is disconnected");
        let msg: protocol::Message = serde_json::from_str(&received).unwrap();
        assert!(matches!(
            msg.kind,
            MessageKind::Response(Response::Error {
                code: ironpad_common::protocol::ErrorCode::SessionNotFound,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn guest_query_when_host_disconnected() {
        let state = test_state();
        let nb = "nb-1";

        let session = state
            .ws
            .sessions
            .create_session(nb.into(), "conn-1".into(), Permissions::default())
            .await;

        let (guest_tx, mut guest_rx) = mpsc::channel::<String>(WS_CHANNEL_BOUND);
        state
            .ws
            .register_guest(&session.session_id, "guest-1", guest_tx)
            .await;

        let perms = Permissions {
            read: true,
            write: false,
        };

        let query_json = wire_msg("q-1", MessageKind::Query(Query::CellsList));

        handle_guest_message(
            &query_json,
            nb,
            &session.session_id,
            "guest-1",
            &perms,
            &state,
        )
        .await;

        // Guest should receive a SessionNotFound error.
        let received = guest_rx.try_recv().expect("guest should receive error");
        let msg: protocol::Message = serde_json::from_str(&received).unwrap();
        assert!(matches!(
            msg.kind,
            MessageKind::Response(Response::Error {
                code: ironpad_common::protocol::ErrorCode::SessionNotFound,
                ..
            })
        ));

        // Query should NOT remain tracked (cleaned up on failure).
        assert!(state.ws.resolve_query("q-1").await.is_none());
    }
}
