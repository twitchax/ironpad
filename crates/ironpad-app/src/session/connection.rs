//! WebSocket connection management (hydrate/browser only).
//!
//! Wired into the UI by the session panel (0007).

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use ironpad_common::protocol::{
    self, ClientId, ControlMessage, MessageKind, Permissions, Response,
};

use crate::model::NotebookModel;

use super::{ConnectionStatus, SessionState};

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Serialize a protocol message and send it over the WebSocket.
/// Logs to the browser console on serialization or send failure.
fn ws_send(ws: &web_sys::WebSocket, msg: &protocol::Message) {
    match serde_json::to_string(msg) {
        Ok(json) => {
            if let Err(e) = ws.send_with_str(&json) {
                web_sys::console::error_1(&format!("WebSocket send failed: {e:?}").into());
            }
        }
        Err(e) => {
            web_sys::console::error_1(&format!("failed to serialize protocol message: {e}").into());
        }
    }
}

// ── Start session ───────────────────────────────────────────────────────────

/// Open a WebSocket to the server and start an agent session.
///
/// Sets up message handlers that bridge agent messages to the `NotebookModel`.
/// Returns `Ok(())` if the WebSocket opens; session details arrive asynchronously
/// via `SessionState` signals.
pub(crate) fn start_session(
    notebook_id: &str,
    permissions: Permissions,
    model: NotebookModel,
    state: SessionState,
) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let location = window.location();
    let protocol = location.protocol().unwrap_or_default();
    let host = location.host().unwrap_or_default();

    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
    let url = format!("{ws_protocol}//{host}/ws/host?notebook_id={notebook_id}");

    let ws = web_sys::WebSocket::new(&url).map_err(|e| format!("{e:?}"))?;

    state.connection_status.set(ConnectionStatus::Connecting);
    state.active.set(true);

    // ── On open: send CreateSession ─────────────────────────────────────

    let ws_clone = ws.clone();
    let perms = permissions;
    let on_open = Closure::<dyn FnMut()>::new(move || {
        state.connection_status.set(ConnectionStatus::Connected);

        ws_send(
            &ws_clone,
            &protocol::Message {
                id: "create-session".to_string(),
                kind: MessageKind::Control(ControlMessage::CreateSession {
                    permissions: perms.clone(),
                }),
            },
        );
    });
    ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    on_open.forget();

    // ── On message: dispatch to model ───────────────────────────────────

    let ws_for_msg = ws.clone();
    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            let Some(text) = event.data().as_string() else {
                return;
            };
            handle_incoming(&text, &model, &state, &ws_for_msg);
        });
    ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();

    // ── On close ────────────────────────────────────────────────────────

    let on_close =
        Closure::<dyn FnMut(web_sys::CloseEvent)>::new(move |_event: web_sys::CloseEvent| {
            state.connection_status.set(ConnectionStatus::Disconnected);
            // Don't reset session state here — end_session handles that explicitly.
            // An unexpected close (network drop) leaves state.active == true so the
            // UI can show "disconnected" rather than silently reverting.
        });
    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    on_close.forget();

    // ── On error ────────────────────────────────────────────────────────

    let on_error =
        Closure::<dyn FnMut(web_sys::ErrorEvent)>::new(move |_event: web_sys::ErrorEvent| {
            // WebSocket errors are followed by a close event, so we just log here.
            web_sys::console::warn_1(&"WebSocket error".into());
        });
    ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    on_error.forget();

    // ── Event bridge: model events → WebSocket ──────────────────────────

    // Store the WS reference for the event bridge Effect.
    let ws_for_events = ws.clone();
    Effect::new(move || {
        // Track the generation counter — this triggers the effect.
        let _gen = model.event_generation().get();

        // Drain events (untracked — won't re-trigger).
        let events = model.drain_events();
        for event in events {
            // Only forward events from the browser (not echoes from agents).
            if event.by == ClientId::browser() {
                ws_send(
                    &ws_for_events,
                    &protocol::Message {
                        id: String::new(),
                        kind: MessageKind::Event(event),
                    },
                );
            }
        }
    });

    // Store the WebSocket on window so end_session can close it.
    if js_sys::Reflect::set(&window, &JsValue::from_str("__ironpad_session_ws"), &ws).is_err() {
        web_sys::console::error_1(&"failed to store WebSocket on window".into());
    }

    Ok(())
}

// ── End session ─────────────────────────────────────────────────────────────

/// Close the WebSocket and reset session state.
pub(crate) fn end_session(state: &SessionState) {
    // Send EndSession if we have a session_id.
    if let (Some(session_id), Some(ws)) = (state.session_id.get_untracked(), get_stored_ws()) {
        ws_send(
            &ws,
            &protocol::Message {
                id: "end-session".to_string(),
                kind: MessageKind::Control(ControlMessage::EndSession { session_id }),
            },
        );
        let _ = ws.close();
    }

    // Clear the stored WebSocket.
    if let Some(window) = web_sys::window() {
        let _ =
            js_sys::Reflect::delete_property(&window, &JsValue::from_str("__ironpad_session_ws"));
    }

    state.reset();
}

/// Retrieve the stored WebSocket from window.
fn get_stored_ws() -> Option<web_sys::WebSocket> {
    let window = web_sys::window()?;
    let val = js_sys::Reflect::get(&window, &JsValue::from_str("__ironpad_session_ws")).ok()?;
    val.dyn_into::<web_sys::WebSocket>().ok()
}

// ── Incoming message handler ────────────────────────────────────────────────

fn handle_incoming(
    text: &str,
    model: &NotebookModel,
    state: &SessionState,
    ws: &web_sys::WebSocket,
) {
    let Ok(msg) = serde_json::from_str::<protocol::Message>(text) else {
        web_sys::console::warn_1(&format!("invalid message from server: {text}").into());
        return;
    };

    match msg.kind {
        // Agent sent a mutation → apply via model (same path as UI edits).
        MessageKind::Mutation(mutation) => {
            // The server relay forwards the raw message; extract a client
            // identifier from the message ID prefix when available, otherwise
            // fall back to a generic agent identity.
            let client_id = ClientId::agent("remote");
            match model.apply(mutation, client_id) {
                Ok((_result, envelope)) => {
                    ws_send(
                        ws,
                        &protocol::Message {
                            id: msg.id,
                            kind: MessageKind::Event(envelope),
                        },
                    );
                    // Persist the change.
                    let nb_state = leptos::prelude::expect_context::<
                        crate::pages::notebook_editor::state::NotebookState,
                    >();
                    crate::pages::notebook_editor::state::persist_notebook(&nb_state);
                }
                Err(err) => {
                    ws_send(
                        ws,
                        &protocol::Message {
                            id: msg.id,
                            kind: MessageKind::Response(Response::Error {
                                code: err.code,
                                message: err.message,
                            }),
                        },
                    );
                }
            }
        }

        // Agent sent a query → answer from model.
        MessageKind::Query(query) => {
            let response_kind = match model.query(query) {
                Ok(response) => MessageKind::Response(response),
                Err(err) => MessageKind::Response(Response::Error {
                    code: err.code,
                    message: err.message,
                }),
            };
            ws_send(
                ws,
                &protocol::Message {
                    id: msg.id,
                    kind: response_kind,
                },
            );
        }

        // Control messages from server (session lifecycle).
        MessageKind::Control(control) => match control {
            ControlMessage::SessionCreated { session_id, token } => {
                state.session_id.set(Some(session_id));
                state.token.set(Some(token));
            }
            ControlMessage::SessionEnded { .. } => {
                state.reset();
            }
            ControlMessage::GuestConnected { client_id } => {
                state
                    .connected_guests
                    .update(|guests| guests.push(client_id.0));
            }
            ControlMessage::GuestDisconnected { client_id } => {
                state
                    .connected_guests
                    .update(|guests| guests.retain(|g| g != &client_id.0));
            }
            ControlMessage::CreateSession { .. } | ControlMessage::EndSession { .. } => {
                // Not expected from server on the host path.
            }
        },

        // Events and responses from server are not expected on the host path.
        MessageKind::Event(_) | MessageKind::Response(_) => {
            web_sys::console::warn_1(&"unexpected Event/Response on host WebSocket".into());
        }
    }
}
