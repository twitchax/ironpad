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
    /// `notebook_id` → host channel sender.
    hosts: Arc<RwLock<HashMap<String, HostHandle>>>,
    /// `session_id` → guest channel senders.
    guests: Arc<RwLock<HashMap<String, Vec<GuestHandle>>>>,
    /// Pending query `message_id` → guest `client_id` (for routing responses).
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

    /// Remove the host for a notebook, but only if the stored connection matches
    /// `connection_id`. This prevents an old (replaced) connection's cleanup from
    /// evicting the *new* host after a reconnect or a second tab of the same
    /// notebook. Returns true if a host was removed.
    pub async fn unregister_host(&self, notebook_id: &str, connection_id: &str) -> bool {
        let mut hosts = self.hosts.write().await;
        if hosts
            .get(notebook_id)
            .is_some_and(|h| h.connection_id == connection_id)
        {
            hosts.remove(notebook_id);
            true
        } else {
            false
        }
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
        {
            let mut guests = self.guests.write().await;
            if let Some(list) = guests.get_mut(session_id) {
                list.retain(|g| g.client_id != client_id);
                if list.is_empty() {
                    guests.remove(session_id);
                }
            }
        }
        // Drop any queries/mutations this guest was still awaiting — with the
        // guest gone, nobody will deliver those responses, so don't leak the
        // pending_queries entries.
        self.pending_queries
            .write()
            .await
            .retain(|_, cid| cid != client_id);
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

    /// Send a JSON message to a specific guest by `client_id`.
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

    /// Resolve a pending query, returning the `client_id` that sent it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use ironpad_common::protocol::Permissions;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn register_and_unregister_host() {
        let ws = WsState::default();
        let (tx, _rx) = mpsc::unbounded_channel();

        ws.register_host("nb-1", "conn-1", tx).await;
        assert!(ws.hosts.read().await.contains_key("nb-1"));

        assert!(ws.unregister_host("nb-1", "conn-1").await);
        assert!(!ws.hosts.read().await.contains_key("nb-1"));
    }

    #[tokio::test]
    async fn unregister_host_returns_false_for_unknown() {
        let ws = WsState::default();
        assert!(!ws.unregister_host("no-such-nb", "conn-x").await);
    }

    #[tokio::test]
    async fn register_host_replaces_existing() {
        let ws = WsState::default();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();

        ws.register_host("nb-1", "conn-1", tx1).await;
        ws.register_host("nb-1", "conn-2", tx2).await;

        // The old connection's cleanup must NOT evict the new host (reconnect race).
        assert!(!ws.unregister_host("nb-1", "conn-1").await);
        assert!(ws.hosts.read().await.contains_key("nb-1"));
        // The current connection can unregister itself.
        assert!(ws.unregister_host("nb-1", "conn-2").await);
        assert!(!ws.hosts.read().await.contains_key("nb-1"));
    }

    #[tokio::test]
    async fn send_to_host_delivers_message() {
        let ws = WsState::default();
        let (tx, mut rx) = mpsc::unbounded_channel();

        ws.register_host("nb-1", "conn-1", tx).await;
        assert!(ws.send_to_host("nb-1", "hello").await);
        assert_eq!(rx.recv().await.unwrap(), "hello");
    }

    #[tokio::test]
    async fn send_to_host_returns_false_for_unknown() {
        let ws = WsState::default();
        assert!(!ws.send_to_host("no-such-nb", "hello").await);
    }

    #[tokio::test]
    async fn register_and_unregister_guest() {
        let ws = WsState::default();
        let (tx, _rx) = mpsc::unbounded_channel();

        ws.register_guest("sess-1", "client-1", tx).await;
        assert!(ws.guests.read().await.contains_key("sess-1"));

        ws.unregister_guest("sess-1", "client-1").await;
        // Session entry removed when last guest leaves.
        assert!(!ws.guests.read().await.contains_key("sess-1"));
    }

    #[tokio::test]
    async fn unregister_guest_leaves_others() {
        let ws = WsState::default();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();

        ws.register_guest("sess-1", "client-1", tx1).await;
        ws.register_guest("sess-1", "client-2", tx2).await;

        ws.unregister_guest("sess-1", "client-1").await;

        let guests = ws.guests.read().await;
        let list = guests.get("sess-1").expect("session still has a guest");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].client_id, "client-2");
    }

    #[tokio::test]
    async fn unregister_guest_purges_pending_queries() {
        let ws = WsState::default();
        let (tx, _rx) = mpsc::unbounded_channel();
        ws.register_guest("sess-1", "client-1", tx).await;

        // Two in-flight queries/mutations from this guest, one from another.
        ws.track_query("m-1", "client-1").await;
        ws.track_query("m-2", "client-1").await;
        ws.track_query("q-9", "client-2").await;

        ws.unregister_guest("sess-1", "client-1").await;

        // The departing guest's pending entries are dropped; others survive.
        assert_eq!(ws.resolve_query("m-1").await, None);
        assert_eq!(ws.resolve_query("m-2").await, None);
        assert_eq!(ws.resolve_query("q-9").await.as_deref(), Some("client-2"));
    }

    #[tokio::test]
    async fn broadcast_to_guests_delivers_to_all() {
        let ws = WsState::default();
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();

        ws.register_guest("sess-1", "client-1", tx1).await;
        ws.register_guest("sess-1", "client-2", tx2).await;

        ws.broadcast_to_guests("sess-1", "update").await;

        assert_eq!(rx1.recv().await.unwrap(), "update");
        assert_eq!(rx2.recv().await.unwrap(), "update");
    }

    #[tokio::test]
    async fn broadcast_to_guests_noop_for_unknown_session() {
        let ws = WsState::default();
        // Should not panic.
        ws.broadcast_to_guests("no-such-sess", "msg").await;
    }

    #[tokio::test]
    async fn broadcast_to_notebook_guests_across_sessions() {
        let ws = WsState::default();

        // Create two sessions for the same notebook.
        let r1 = ws
            .sessions
            .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
            .await;
        let r2 = ws
            .sessions
            .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
            .await;

        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();

        ws.register_guest(&r1.session_id, "client-1", tx1).await;
        ws.register_guest(&r2.session_id, "client-2", tx2).await;

        ws.broadcast_to_notebook_guests("nb-1", "nb-update").await;

        assert_eq!(rx1.recv().await.unwrap(), "nb-update");
        assert_eq!(rx2.recv().await.unwrap(), "nb-update");
    }

    #[tokio::test]
    async fn broadcast_to_notebook_guests_ignores_other_notebooks() {
        let ws = WsState::default();

        let r1 = ws
            .sessions
            .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
            .await;
        let r2 = ws
            .sessions
            .create_session("nb-2".into(), "conn-1".into(), Permissions::default())
            .await;

        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();

        ws.register_guest(&r1.session_id, "client-1", tx1).await;
        ws.register_guest(&r2.session_id, "client-2", tx2).await;

        ws.broadcast_to_notebook_guests("nb-1", "only-nb1").await;

        assert_eq!(rx1.recv().await.unwrap(), "only-nb1");
        // rx2 should have nothing.
        assert!(rx2.try_recv().is_err());
    }

    #[tokio::test]
    async fn send_to_guest_delivers_to_specific_client() {
        let ws = WsState::default();
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();

        ws.register_guest("sess-1", "client-1", tx1).await;
        ws.register_guest("sess-1", "client-2", tx2).await;

        assert!(ws.send_to_guest("client-2", "targeted").await);

        assert!(rx1.try_recv().is_err());
        assert_eq!(rx2.recv().await.unwrap(), "targeted");
    }

    #[tokio::test]
    async fn send_to_guest_returns_false_for_unknown() {
        let ws = WsState::default();
        assert!(!ws.send_to_guest("no-such-client", "msg").await);
    }

    #[tokio::test]
    async fn disconnect_guests_drops_all_channels() {
        let ws = WsState::default();
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();

        ws.register_guest("sess-1", "client-1", tx1).await;
        ws.register_guest("sess-1", "client-2", tx2).await;

        ws.disconnect_guests("sess-1").await;

        // Session removed from map.
        assert!(!ws.guests.read().await.contains_key("sess-1"));
        // Channels closed — recv returns None.
        assert!(rx1.recv().await.is_none());
        assert!(rx2.recv().await.is_none());
    }

    #[tokio::test]
    async fn track_and_resolve_query() {
        let ws = WsState::default();

        ws.track_query("msg-42", "client-7").await;
        let resolved = ws.resolve_query("msg-42").await;
        assert_eq!(resolved.as_deref(), Some("client-7"));

        // Second resolve returns None (consumed).
        assert_eq!(ws.resolve_query("msg-42").await, None);
    }

    #[tokio::test]
    async fn resolve_query_returns_none_for_unknown() {
        let ws = WsState::default();
        assert_eq!(ws.resolve_query("no-such-msg").await, None);
    }
}
