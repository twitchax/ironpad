//! Shared server state for the Axum application.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::FromRef;
use leptos::config::LeptosOptions;
use tokio::sync::{mpsc, RwLock};

use ironpad_common::AppConfig;

use crate::sessions::SessionStore;

// ── App state ───────────────────────────────────────────────────────────────

/// Combined state shared across all Axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub leptos_options: LeptosOptions,
    pub config: AppConfig,
    pub ws: WsState,
}

/// Leptos needs to extract `LeptosOptions` from state for SSR + file serving.
impl FromRef<AppState> for LeptosOptions {
    fn from_ref(state: &AppState) -> Self {
        state.leptos_options.clone()
    }
}

// ── WebSocket relay state ───────────────────────────────────────────────────

/// Manages WebSocket connections between browser hosts and CLI guests.
#[derive(Clone, Default)]
pub struct WsState {
    pub sessions: SessionStore,
    /// notebook_id → host channel sender.
    hosts: Arc<RwLock<HashMap<String, HostHandle>>>,
    /// session_id → guest channel senders.
    guests: Arc<RwLock<HashMap<String, Vec<GuestHandle>>>>,
    /// Pending query message_id → guest client_id (for routing responses).
    pending_queries: Arc<RwLock<HashMap<String, String>>>,
}

/// Channel handle for a connected browser host.
#[derive(Clone)]
struct HostHandle {
    connection_id: String,
    sender: mpsc::UnboundedSender<String>,
}

/// Channel handle for a connected CLI guest.
#[derive(Clone)]
struct GuestHandle {
    client_id: String,
    sender: mpsc::UnboundedSender<String>,
}

impl WsState {
    // ── Host management ─────────────────────────────────────────────────

    /// Register a browser as the host for a notebook.
    ///
    /// If a host is already registered for this notebook, it is replaced
    /// (the old host's channel is dropped, closing its WebSocket).
    pub async fn register_host(
        &self,
        notebook_id: &str,
        connection_id: &str,
        sender: mpsc::UnboundedSender<String>,
    ) {
        let prev = self.hosts.write().await.insert(
            notebook_id.to_string(),
            HostHandle {
                connection_id: connection_id.to_string(),
                sender,
            },
        );
        if prev.is_some() {
            tracing::warn!(
                notebook_id = %notebook_id,
                "replacing existing host connection for notebook"
            );
        }
    }

    /// Remove the host for a notebook. Returns the connection_id if found.
    pub async fn unregister_host(&self, notebook_id: &str) -> Option<String> {
        self.hosts
            .write()
            .await
            .remove(notebook_id)
            .map(|h| h.connection_id)
    }

    /// Send a JSON message to the host of a notebook.
    pub async fn send_to_host(&self, notebook_id: &str, message: &str) -> bool {
        let hosts = self.hosts.read().await;
        if let Some(host) = hosts.get(notebook_id) {
            host.sender.send(message.to_string()).is_ok()
        } else {
            false
        }
    }

    // ── Guest management ────────────────────────────────────────────────

    /// Register a CLI agent as a guest on a session.
    pub async fn register_guest(
        &self,
        session_id: &str,
        client_id: &str,
        sender: mpsc::UnboundedSender<String>,
    ) {
        self.guests
            .write()
            .await
            .entry(session_id.to_string())
            .or_default()
            .push(GuestHandle {
                client_id: client_id.to_string(),
                sender,
            });
    }

    /// Remove a specific guest from a session.
    pub async fn unregister_guest(&self, session_id: &str, client_id: &str) {
        let mut guests = self.guests.write().await;
        if let Some(list) = guests.get_mut(session_id) {
            list.retain(|g| g.client_id != client_id);
            if list.is_empty() {
                guests.remove(session_id);
            }
        }
    }

    /// Send a JSON message to all guests on a session.
    pub async fn broadcast_to_guests(&self, session_id: &str, message: &str) {
        let guests = self.guests.read().await;
        if let Some(list) = guests.get(session_id) {
            for guest in list {
                let _ = guest.sender.send(message.to_string());
            }
        }
    }

    /// Send a JSON message to all guests on all sessions for a notebook.
    pub async fn broadcast_to_notebook_guests(&self, notebook_id: &str, message: &str) {
        let sessions = self.sessions_for_notebook(notebook_id).await;
        let guests = self.guests.read().await;
        for session_id in &sessions {
            if let Some(list) = guests.get(session_id) {
                for guest in list {
                    let _ = guest.sender.send(message.to_string());
                }
            }
        }
    }

    /// Send a JSON message to a specific guest by client_id.
    pub async fn send_to_guest(&self, client_id: &str, message: &str) -> bool {
        let guests = self.guests.read().await;
        for list in guests.values() {
            if let Some(guest) = list.iter().find(|g| g.client_id == client_id) {
                return guest.sender.send(message.to_string()).is_ok();
            }
        }
        false
    }

    /// Disconnect all guests on a session, sending them a close reason.
    pub async fn disconnect_guests(&self, session_id: &str) {
        self.guests.write().await.remove(session_id);
        // Dropping the senders closes the channels, which causes the
        // send tasks to exit and the WebSocket connections to close.
    }

    // ── Query tracking ──────────────────────────────────────────────────

    /// Track a pending query so the response can be routed back.
    pub async fn track_query(&self, message_id: &str, client_id: &str) {
        self.pending_queries
            .write()
            .await
            .insert(message_id.to_string(), client_id.to_string());
    }

    /// Resolve a pending query, returning the client_id that sent it.
    pub async fn resolve_query(&self, message_id: &str) -> Option<String> {
        self.pending_queries.write().await.remove(message_id)
    }

    // ── Helpers ─────────────────────────────────────────────────────────

    /// Get all session IDs for a notebook.
    async fn sessions_for_notebook(&self, notebook_id: &str) -> Vec<String> {
        // Read all sessions and filter by notebook_id.
        // This is O(n) over sessions — fine for the expected scale.
        let sessions = self.sessions.all_sessions().await;
        sessions
            .into_iter()
            .filter(|s| s.notebook_id == notebook_id)
            .map(|s| s.id)
            .collect()
    }
}
