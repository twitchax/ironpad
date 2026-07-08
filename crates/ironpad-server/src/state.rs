//! Shared server state for the Axum application.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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

/// Default global cap on concurrent guest connections across all sessions.
/// Bounds the memory/task cost of a connection flood; generous for legitimate
/// multi-agent use on a single-author deployment.
const DEFAULT_MAX_GUESTS: usize = 512;

/// Default idle timeout for a guest connection. Guests (CLI daemons) don't send
/// heartbeats and don't auto-reconnect, so this is deliberately generous: it
/// exists to reap a half-open connection (a network drop with no FIN), not to
/// police an agent's normal think-time. Any inbound frame resets the window.
const DEFAULT_GUEST_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Manages WebSocket connections between browser hosts and CLI guests.
#[derive(Clone)]
pub struct WsState {
    pub sessions: SessionStore,
    /// `notebook_id` → host channel sender.
    hosts: Arc<RwLock<HashMap<String, HostHandle>>>,
    /// `notebook_id` → blake3(host secret). Bound on first claim (TOFU),
    /// required to match on later claims, and forgotten when the notebook goes
    /// idle (see [`WsState::claim_host`] / [`WsState::forget_secret_if_idle`]).
    /// In-memory only — after a restart there is no active host/session to
    /// hijack, and TOFU re-establishes on the next claim (PRD-0038 T-014).
    notebook_secrets: Arc<RwLock<HashMap<String, blake3::Hash>>>,
    /// `session_id` → guest channel senders.
    guests: Arc<RwLock<HashMap<String, Vec<GuestHandle>>>>,
    /// Pending query `message_id` → guest `client_id` (for routing responses).
    pending_queries: Arc<RwLock<HashMap<String, String>>>,
    /// Idle timeout applied to guest read loops (see [`DEFAULT_GUEST_IDLE_TIMEOUT`]).
    guest_idle_timeout: Duration,
    /// Global cap on concurrent guest connections (see [`DEFAULT_MAX_GUESTS`]).
    max_guests: usize,
}

impl Default for WsState {
    fn default() -> Self {
        Self {
            sessions: SessionStore::default(),
            hosts: Arc::default(),
            notebook_secrets: Arc::default(),
            guests: Arc::default(),
            pending_queries: Arc::default(),
            guest_idle_timeout: DEFAULT_GUEST_IDLE_TIMEOUT,
            max_guests: DEFAULT_MAX_GUESTS,
        }
    }
}

/// Channel handle for a connected browser host.
#[derive(Clone)]
struct HostHandle {
    connection_id: String,
    sender: mpsc::Sender<String>,
}

/// Channel handle for a connected CLI guest.
#[derive(Clone)]
struct GuestHandle {
    client_id: String,
    sender: mpsc::Sender<String>,
}

/// Outcome of a host claim against a notebook's bound secret (PRD-0038 T-014).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// The claim is authorized. `tofu` is true when this claim *established* the
    /// binding (trust-on-first-use), false when it matched an existing binding.
    Accepted { tofu: bool },
    /// The presented secret does not match the notebook's bound secret — reject.
    Rejected,
}

impl WsState {
    // ── Configuration (builders + accessors) ────────────────────────────

    /// Override the guest idle timeout. A legitimate tuning knob, and also lets
    /// tests exercise the reaper without waiting out the production timeout.
    #[must_use]
    pub fn with_guest_idle_timeout(mut self, timeout: Duration) -> Self {
        self.guest_idle_timeout = timeout;
        self
    }

    /// Override the global cap on concurrent guest connections.
    #[must_use]
    pub fn with_max_guests(mut self, max: usize) -> Self {
        self.max_guests = max;
        self
    }

    /// Idle timeout applied to guest read loops.
    #[must_use]
    pub fn guest_idle_timeout(&self) -> Duration {
        self.guest_idle_timeout
    }

    /// Total number of registered guest connections across all sessions.
    pub async fn guest_count(&self) -> usize {
        self.guests.read().await.values().map(Vec::len).sum()
    }

    /// Whether another guest connection may be admitted under the global cap.
    pub async fn guest_slot_available(&self) -> bool {
        self.guest_count().await < self.max_guests
    }

    // ── Host management ─────────────────────────────────────────────────

    /// Register a browser as the host for a notebook.
    ///
    /// If a host is already registered for this notebook, it is replaced
    /// (the old host's channel is dropped, closing its WebSocket).
    pub async fn register_host(
        &self,
        notebook_id: &str,
        connection_id: &str,
        sender: mpsc::Sender<String>,
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

    /// Authorize a host claim for `notebook_id` against its bound secret.
    ///
    /// Trust-on-first-use: the first claim for a notebook binds the secret and
    /// is accepted; later claims must present the same secret. A mismatch is
    /// rejected — this is what stops anyone who merely knows a `notebook_id`
    /// from evicting the real host and receiving its guest traffic.
    pub async fn claim_host(&self, notebook_id: &str, secret: &str) -> ClaimOutcome {
        let presented = blake3::hash(secret.as_bytes());
        let mut secrets = self.notebook_secrets.write().await;
        match secrets.get(notebook_id) {
            None => {
                secrets.insert(notebook_id.to_string(), presented);
                ClaimOutcome::Accepted { tofu: true }
            }
            // `blake3::Hash` equality is constant-time.
            Some(bound) if *bound == presented => ClaimOutcome::Accepted { tofu: false },
            Some(_) => ClaimOutcome::Rejected,
        }
    }

    /// Forget a notebook's host-secret binding once it is idle — no host
    /// connection AND no sessions. This bounds the map and lets a fresh TOFU
    /// claim take over an abandoned notebook, where there is nothing to hijack.
    pub async fn forget_secret_if_idle(&self, notebook_id: &str) {
        if self.hosts.read().await.contains_key(notebook_id) {
            return;
        }
        let has_session = self
            .sessions
            .all_sessions()
            .await
            .iter()
            .any(|s| s.notebook_id == notebook_id);
        if !has_session {
            self.notebook_secrets.write().await.remove(notebook_id);
        }
    }

    /// Send a JSON message to the host of a notebook.
    pub async fn send_to_host(&self, notebook_id: &str, message: &str) -> bool {
        let hosts = self.hosts.read().await;
        if let Some(host) = hosts.get(notebook_id) {
            host.sender.try_send(message.to_string()).is_ok()
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
        sender: mpsc::Sender<String>,
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
                let _ = guest.sender.try_send(message.to_string());
            }
        }
    }

    /// Send a JSON message to all guests on all *readable* sessions for a
    /// notebook.
    ///
    /// Events carry full cell content, so a session created with `read: false`
    /// (a write-only / "blind" agent) is intentionally skipped — this is the
    /// confidentiality boundary for the `read` permission. Session lifecycle
    /// control messages (`SessionEnded`) use [`broadcast_to_guests`], which is
    /// *not* gated so a revoked guest still learns its session ended.
    pub async fn broadcast_to_notebook_guests(&self, notebook_id: &str, message: &str) {
        let sessions = self.readable_sessions_for_notebook(notebook_id).await;
        let guests = self.guests.read().await;
        for session_id in &sessions {
            if let Some(list) = guests.get(session_id) {
                for guest in list {
                    let _ = guest.sender.try_send(message.to_string());
                }
            }
        }
    }

    /// Send a JSON message to a specific guest by `client_id`.
    pub async fn send_to_guest(&self, client_id: &str, message: &str) -> bool {
        let guests = self.guests.read().await;
        for list in guests.values() {
            if let Some(guest) = list.iter().find(|g| g.client_id == client_id) {
                return guest.sender.try_send(message.to_string()).is_ok();
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

    /// Session IDs for a notebook whose permissions grant `read`. Used to gate
    /// content-bearing event broadcasts (see [`broadcast_to_notebook_guests`]),
    /// which is why sessions with `read: false` are filtered out here.
    ///
    /// O(n) over sessions — fine for the expected scale.
    async fn readable_sessions_for_notebook(&self, notebook_id: &str) -> Vec<String> {
        let sessions = self.sessions.all_sessions().await;
        sessions
            .into_iter()
            .filter(|s| s.notebook_id == notebook_id && s.permissions.read)
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
        let (tx, _rx) = mpsc::channel(64);

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

    // ── Host-secret claim (PRD-0038 T-014) ──────────────────────────────

    #[tokio::test]
    async fn claim_host_tofu_then_match_then_reject() {
        let ws = WsState::default();
        // First claim of a fresh notebook binds the secret (trust-on-first-use).
        assert!(matches!(
            ws.claim_host("nb-1", "s3cr3t").await,
            ClaimOutcome::Accepted { tofu: true }
        ));
        // The same secret is accepted again (reconnect / take-over), not TOFU.
        assert!(matches!(
            ws.claim_host("nb-1", "s3cr3t").await,
            ClaimOutcome::Accepted { tofu: false }
        ));
        // A different secret is rejected — the hole this closes.
        assert!(matches!(
            ws.claim_host("nb-1", "wrong").await,
            ClaimOutcome::Rejected
        ));
        // A different notebook binds independently.
        assert!(matches!(
            ws.claim_host("nb-2", "other").await,
            ClaimOutcome::Accepted { tofu: true }
        ));
    }

    #[tokio::test]
    async fn forget_secret_if_idle_drops_binding_when_no_host_no_sessions() {
        let ws = WsState::default();
        assert!(matches!(
            ws.claim_host("nb-1", "a").await,
            ClaimOutcome::Accepted { tofu: true }
        ));
        // No host connection, no sessions → forget the binding.
        ws.forget_secret_if_idle("nb-1").await;
        // A brand-new secret is now TOFU-accepted (the binding was dropped).
        assert!(matches!(
            ws.claim_host("nb-1", "b").await,
            ClaimOutcome::Accepted { tofu: true }
        ));
    }

    #[tokio::test]
    async fn forget_secret_if_idle_keeps_binding_while_host_registered() {
        let ws = WsState::default();
        assert!(matches!(
            ws.claim_host("nb-1", "a").await,
            ClaimOutcome::Accepted { tofu: true }
        ));
        let (tx, _rx) = mpsc::channel(4);
        ws.register_host("nb-1", "conn-1", tx).await;
        // Host still connected → the binding must survive.
        ws.forget_secret_if_idle("nb-1").await;
        assert!(matches!(
            ws.claim_host("nb-1", "b").await,
            ClaimOutcome::Rejected
        ));
    }

    #[tokio::test]
    async fn forget_secret_if_idle_keeps_binding_while_session_active() {
        let ws = WsState::default();
        assert!(matches!(
            ws.claim_host("nb-1", "a").await,
            ClaimOutcome::Accepted { tofu: true }
        ));
        // An active session for the notebook keeps the binding even with no host.
        ws.sessions
            .create_session(
                "nb-1".to_string(),
                "conn-1".to_string(),
                Permissions::default(),
            )
            .await;
        ws.forget_secret_if_idle("nb-1").await;
        assert!(matches!(
            ws.claim_host("nb-1", "b").await,
            ClaimOutcome::Rejected
        ));
    }

    #[tokio::test]
    async fn register_host_replaces_existing() {
        let ws = WsState::default();
        let (tx1, _rx1) = mpsc::channel(64);
        let (tx2, _rx2) = mpsc::channel(64);

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
        let (tx, mut rx) = mpsc::channel(64);

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
        let (tx, _rx) = mpsc::channel(64);

        ws.register_guest("sess-1", "client-1", tx).await;
        assert!(ws.guests.read().await.contains_key("sess-1"));

        ws.unregister_guest("sess-1", "client-1").await;
        // Session entry removed when last guest leaves.
        assert!(!ws.guests.read().await.contains_key("sess-1"));
    }

    #[tokio::test]
    async fn unregister_guest_leaves_others() {
        let ws = WsState::default();
        let (tx1, _rx1) = mpsc::channel(64);
        let (tx2, _rx2) = mpsc::channel(64);

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
        let (tx, _rx) = mpsc::channel(64);
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
        let (tx1, mut rx1) = mpsc::channel(64);
        let (tx2, mut rx2) = mpsc::channel(64);

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

        let (tx1, mut rx1) = mpsc::channel(64);
        let (tx2, mut rx2) = mpsc::channel(64);

        ws.register_guest(&r1.session_id, "client-1", tx1).await;
        ws.register_guest(&r2.session_id, "client-2", tx2).await;

        ws.broadcast_to_notebook_guests("nb-1", "nb-update").await;

        assert_eq!(rx1.recv().await.unwrap(), "nb-update");
        assert_eq!(rx2.recv().await.unwrap(), "nb-update");
    }

    #[tokio::test]
    async fn broadcast_to_notebook_guests_skips_read_denied_sessions() {
        let ws = WsState::default();

        // Two sessions on the same notebook: one readable, one write-only.
        let reader = ws
            .sessions
            .create_session(
                "nb-1".into(),
                "conn-1".into(),
                Permissions {
                    read: true,
                    write: true,
                },
            )
            .await;
        let blind = ws
            .sessions
            .create_session(
                "nb-1".into(),
                "conn-1".into(),
                Permissions {
                    read: false,
                    write: true,
                },
            )
            .await;

        let (tx_r, mut rx_r) = mpsc::channel(64);
        let (tx_b, mut rx_b) = mpsc::channel(64);
        ws.register_guest(&reader.session_id, "reader", tx_r).await;
        ws.register_guest(&blind.session_id, "writer", tx_b).await;

        ws.broadcast_to_notebook_guests("nb-1", "cell-source").await;

        // The readable session's guest receives the content event...
        assert_eq!(rx_r.recv().await.unwrap(), "cell-source");
        // ...but the read:false (write-only) guest must not — the read
        // permission is a confidentiality boundary for content-bearing events.
        assert!(
            rx_b.try_recv().is_err(),
            "a read:false guest must not receive content-bearing events"
        );
    }

    #[tokio::test]
    async fn guest_count_and_cap() {
        let ws = WsState::default().with_max_guests(2);
        assert_eq!(ws.guest_count().await, 0);
        assert!(ws.guest_slot_available().await);

        let (tx1, _r1) = mpsc::channel(8);
        let (tx2, _r2) = mpsc::channel(8);
        ws.register_guest("sess-1", "c1", tx1).await;
        ws.register_guest("sess-1", "c2", tx2).await;

        assert_eq!(ws.guest_count().await, 2);
        assert!(
            !ws.guest_slot_available().await,
            "no slot should be available once the cap of 2 is reached"
        );
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

        let (tx1, mut rx1) = mpsc::channel(64);
        let (tx2, mut rx2) = mpsc::channel(64);

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
        let (tx1, mut rx1) = mpsc::channel(64);
        let (tx2, mut rx2) = mpsc::channel(64);

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
        let (tx1, mut rx1) = mpsc::channel(64);
        let (tx2, mut rx2) = mpsc::channel(64);

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
