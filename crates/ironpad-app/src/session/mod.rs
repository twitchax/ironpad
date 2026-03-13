//! Browser-side WebSocket session for agent collaboration.
//!
//! When a session is active, this module manages the WebSocket connection
//! to the server, bridges incoming agent messages to the [`NotebookModel`],
//! and forwards outgoing model events to the server for relay to agents.
//!
//! This module is only active in the `hydrate` (browser) build.

#[cfg(feature = "hydrate")]
mod connection;

#[cfg(feature = "hydrate")]
pub(crate) use connection::*;

/// Session state signals — available in both SSR and hydrate builds
/// so components can reference the types without feature-gating.
use leptos::prelude::*;

/// Reactive state for an active agent session, provided as Leptos context.
#[derive(Clone, Copy)]
pub(crate) struct SessionState {
    /// Whether a session is currently active.
    pub active: RwSignal<bool>,
    /// The session ID (set after CreateSession succeeds).
    pub session_id: RwSignal<Option<String>>,
    /// The plaintext token to give to the agent (shown once).
    pub token: RwSignal<Option<String>>,
    /// Client IDs of currently connected guests.
    pub connected_guests: RwSignal<Vec<String>>,
    /// Current connection status.
    pub connection_status: RwSignal<ConnectionStatus>,
}

/// WebSocket connection status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    /// Reserved for future reconnection logic.
    #[allow(dead_code)]
    Reconnecting,
}

impl SessionState {
    pub fn new() -> Self {
        Self {
            active: RwSignal::new(false),
            session_id: RwSignal::new(None),
            token: RwSignal::new(None),
            connected_guests: RwSignal::new(Vec::new()),
            connection_status: RwSignal::new(ConnectionStatus::Disconnected),
        }
    }

    pub fn reset(&self) {
        self.active.set(false);
        self.session_id.set(None);
        self.token.set(None);
        self.connected_guests.set(Vec::new());
        self.connection_status.set(ConnectionStatus::Disconnected);
    }
}
