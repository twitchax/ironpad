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
use crate::state::AppState;

/// Serialize a protocol message to JSON for the wire.
fn wire_msg(id: &str, kind: MessageKind) -> String {
    serde_json::to_string(&protocol::Message {
        id: id.to_string(),
        kind,
    })
    .expect("protocol message serialization should never fail")
}

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
pub async fn ws_host_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<HostParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_host(socket, params.notebook_id, state))
}

async fn handle_host(socket: WebSocket, notebook_id: String, state: AppState) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let connection_id = uuid::Uuid::new_v4().to_string();

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
        while let Some(Ok(msg)) = ws_receiver.next().await {
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

    // Cleanup: unregister host, invalidate sessions, disconnect guests.
    tracing::info!(notebook_id = %notebook_id, "host disconnected");
    state.ws.unregister_host(&notebook_id).await;

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
}

/// Process a message from the host browser.
async fn handle_host_message(text: &str, notebook_id: &str, connection_id: &str, state: &AppState) {
    let Ok(msg) = serde_json::from_str::<ironpad_common::protocol::Message>(text) else {
        tracing::warn!("host sent invalid JSON");
        return;
    };

    match &msg.kind {
        // Host sends events → broadcast to all guests for this notebook.
        MessageKind::Event(_) => {
            state
                .ws
                .broadcast_to_notebook_guests(notebook_id, text)
                .await;
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

        // These control messages are server-originated, not host-sent.
        ControlMessage::SessionCreated { .. }
        | ControlMessage::SessionEnded { .. }
        | ControlMessage::GuestConnected { .. }
        | ControlMessage::GuestDisconnected { .. } => {}
    }
}

// ── Guest handler ───────────────────────────────────────────────────────────

/// WebSocket upgrade for CLI guests: `GET /ws/connect?token=<token>`.
///
/// Validates the token before upgrading. Returns HTTP 401 if invalid,
/// HTTP 410 if expired.
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

    let session_id = session.id.clone();
    let notebook_id = session.notebook_id.clone();
    let permissions = session.permissions.clone();
    let token_prefix = params.token.get(..8).unwrap_or(&params.token);
    let client_id = ClientId::agent(token_prefix).0;

    Ok(ws.on_upgrade(move |socket| {
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
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

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
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
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
        // Guest sends mutations → forward to host.
        MessageKind::Mutation(_) => {
            if !state.ws.send_to_host(notebook_id, text).await {
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

        // Guest sends queries → forward to host, track for response routing.
        MessageKind::Query(_) => {
            state.ws.track_query(&msg.id, client_id).await;
            if !state.ws.send_to_host(notebook_id, text).await {
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

        // Guests don't send events, responses, or control messages.
        MessageKind::Event(_) | MessageKind::Response(_) | MessageKind::Control(_) => {
            tracing::debug!(client_id = %client_id, "guest sent unexpected message type");
        }
    }
}
