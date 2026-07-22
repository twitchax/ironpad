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

/// Load this notebook's host secret from `localStorage`, generating and storing a
/// fresh one on first use (PRD-0038 T-014). Kept in `localStorage` — separate
/// from the `IndexedDB` notebook record — so export/share of the notebook never
/// carries the secret. Entropy comes from two v4 UUIDs (getrandom → the
/// browser's CSPRNG on wasm), which is unguessable and avoids a web-sys Crypto
/// feature dependency.
fn host_secret(notebook_id: &str) -> String {
    let key = format!("ironpad:host-secret:{notebook_id}");
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten());
    if let Some(existing) = storage
        .as_ref()
        .and_then(|s| s.get_item(&key).ok().flatten())
        .filter(|v| !v.is_empty())
    {
        return existing;
    }
    let secret = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    if let Some(s) = storage.as_ref() {
        let _ = s.set_item(&key, &secret);
    }
    secret
}

// ── Session socket ──────────────────────────────────────────────────────────

/// The live WebSocket plus everything that must die with it: the event
/// closures and the heartbeat interval. These closures capture the page's
/// model signals; they were previously `forget()`-leaked, so a message
/// arriving after the page was disposed (navigate away with the session
/// still running) read disposed signals and panicked the whole reactive
/// runtime — leaving every subsequent page render-only.
pub(crate) struct SessionSocket {
    ws: web_sys::WebSocket,
    heartbeat_interval_id: Option<i32>,
    _on_open: Closure<dyn FnMut()>,
    _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _on_close: Closure<dyn FnMut(web_sys::CloseEvent)>,
    _on_error: Closure<dyn FnMut(web_sys::ErrorEvent)>,
    _heartbeat: Closure<dyn FnMut()>,
}

impl SessionSocket {
    /// Detach the handlers, stop the heartbeat, and close the socket.
    /// Handlers are detached BEFORE the closures drop (when `self` does), so
    /// a straggling event can't invoke a dropped closure.
    fn teardown(self) {
        self.ws.set_onopen(None);
        self.ws.set_onmessage(None);
        self.ws.set_onclose(None);
        self.ws.set_onerror(None);
        if let (Some(id), Some(window)) = (self.heartbeat_interval_id, web_sys::window()) {
            window.clear_interval_with_handle(id);
        }
        let _ = self.ws.close();
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

    // Replace any existing socket. The panel gates Start on !active, but a
    // leaked previous socket must never survive regardless.
    if let Some(old) = state.socket.try_update_value(Option::take).flatten() {
        old.teardown();
    }

    state.connection_status.set(ConnectionStatus::Connecting);
    state.active.set(true);

    // ── On open: claim the host role, then create the session ───────────

    let ws_clone = ws.clone();
    let perms = permissions;
    let secret = host_secret(notebook_id);
    let on_open = Closure::<dyn FnMut()>::new(move || {
        state.connection_status.set(ConnectionStatus::Connected);

        // First frame: prove we hold this notebook's host secret (PRD-0038
        // T-014). The relay validates this before registering us as host and
        // closes 4403 on a mismatch (handled in on_close below).
        ws_send(
            &ws_clone,
            &protocol::Message {
                id: "claim-host".to_string(),
                kind: MessageKind::Control(ControlMessage::ClaimHost {
                    secret: secret.clone(),
                }),
            },
        );

        ws_send(
            &ws_clone,
            &protocol::Message {
                id: "create-session".to_string(),
                kind: MessageKind::Control(ControlMessage::CreateSession {
                    permissions: perms.clone(),
                }),
            },
        );

        // Flush any browser events buffered before the socket opened. The event
        // bridge Effect skips draining while the socket is still CONNECTING (a
        // send on a non-OPEN socket throws InvalidStateError and the event would
        // be lost), so those edits are still queued in the model — send them now.
        for event in model.drain_events() {
            if event.by == ClientId::browser() {
                ws_send(
                    &ws_clone,
                    &protocol::Message {
                        id: String::new(),
                        kind: MessageKind::Event(event),
                    },
                );
            }
        }
    });
    ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));

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

    // ── On close ────────────────────────────────────────────────────────

    let on_close =
        Closure::<dyn FnMut(web_sys::CloseEvent)>::new(move |event: web_sys::CloseEvent| {
            state.connection_status.set(ConnectionStatus::Disconnected);
            // 4403 = the relay rejected our host claim (PRD-0038 T-014): the
            // notebook's stored host secret doesn't match the server's binding
            // (e.g. it's already hosted in another session). Surface it clearly
            // rather than silently retrying into a rejection loop.
            if event.code() == 4403 {
                web_sys::console::error_1(
                    &"host claim rejected: this notebook's host credential doesn't match — \
                      it may already be open in another session"
                        .into(),
                );
            }
            // Don't reset session state here — end_session handles that explicitly.
            // An unexpected close (network drop) leaves state.active == true so the
            // UI can show "disconnected" rather than silently reverting.
        });
    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));

    // ── On error ────────────────────────────────────────────────────────

    let on_error =
        Closure::<dyn FnMut(web_sys::ErrorEvent)>::new(move |_event: web_sys::ErrorEvent| {
            // WebSocket errors are followed by a close event, so we just log here.
            web_sys::console::warn_1(&"WebSocket error".into());
        });
    ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));

    // ── Event bridge: model events → WebSocket ──────────────────────────

    // Store the WS reference for the event bridge Effect.
    let ws_for_events = ws.clone();
    Effect::new(move || {
        // Track the generation counter — this triggers the effect.
        let _gen = model.event_generation().get();

        // Don't drain while the socket isn't OPEN — a send on a CONNECTING
        // socket throws InvalidStateError and the drained event would be lost.
        // Events stay buffered in the model; on_open flushes the pre-open
        // backlog, and later generation bumps re-run this effect once OPEN.
        if ws_for_events.ready_state() != web_sys::WebSocket::OPEN {
            return;
        }

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

    // ── Heartbeat: keep the connection alive + let the server reap dead ones ─
    //
    // The browser WebSocket API can't send protocol-level Ping frames, so send an
    // application-level Heartbeat every 30s. The server's read loop tears down a
    // connection after 120s of silence (a half-open network drop).
    let ws_for_hb = ws.clone();
    let heartbeat = Closure::<dyn FnMut()>::new(move || {
        if ws_for_hb.ready_state() == web_sys::WebSocket::OPEN {
            ws_send(
                &ws_for_hb,
                &protocol::Message {
                    id: String::new(),
                    kind: MessageKind::Control(ControlMessage::Heartbeat),
                },
            );
        }
    });
    let heartbeat_interval_id = window
        .set_interval_with_callback_and_timeout_and_arguments_0(
            heartbeat.as_ref().unchecked_ref(),
            30_000,
        )
        .ok();

    // Hand everything to the page-scoped holder: end_session (Stop button or
    // page cleanup) takes it back and tears it down.
    state.socket.set_value(Some(SessionSocket {
        ws,
        heartbeat_interval_id,
        _on_open: on_open,
        _on_message: on_message,
        _on_close: on_close,
        _on_error: on_error,
        _heartbeat: heartbeat,
    }));

    Ok(())
}

// ── End session ─────────────────────────────────────────────────────────────

/// Close the WebSocket and reset session state.
///
/// Runs from the panel's Stop button AND from the notebook page's
/// `on_cleanup` (the session must never outlive the page whose model it
/// serves), so signal reads use `try_` variants.
pub(crate) fn end_session(state: &SessionState) {
    if let Some(socket) = state.socket.try_update_value(Option::take).flatten() {
        // Polite goodbye so the server releases the session and guests see
        // SessionEnded instead of a silent stall.
        if let Some(session_id) = state.session_id.try_get_untracked().flatten() {
            ws_send(
                &socket.ws,
                &protocol::Message {
                    id: "end-session".to_string(),
                    kind: MessageKind::Control(ControlMessage::EndSession { session_id }),
                },
            );
        }
        socket.teardown();
    }

    state.reset();
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
            ControlMessage::CreateSession { .. }
            | ControlMessage::EndSession { .. }
            | ControlMessage::Heartbeat
            | ControlMessage::ClaimHost { .. } => {
                // Not sent by the server to the host (Heartbeat and ClaimHost are
                // host → server).
            }
            ControlMessage::Unknown => {
                web_sys::console::warn_1(&"ignoring unknown control message from server".into());
            }
        },

        // Events and responses from server are not expected on the host path.
        MessageKind::Event(_) | MessageKind::Response(_) => {
            web_sys::console::warn_1(&"unexpected Event/Response on host WebSocket".into());
        }
    }
}
