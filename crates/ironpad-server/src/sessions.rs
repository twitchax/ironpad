//! In-memory session store for agent collaboration.
//!
//! Sessions are ephemeral — a server restart invalidates all sessions.
//! This is correct by design: the browser (model server) also loses its
//! WebSocket connection on restart, so agents must reconnect regardless.
//!

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use ironpad_common::protocol::{MessageKind, Permissions};
use tokio::sync::RwLock;

// ── Errors ──────────────────────────────────────────────────────────────────

/// Why token validation failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidateError {
    /// No session matches the provided token.
    InvalidToken,
    /// Token matched a session, but it has expired.
    SessionExpired,
}

// ── Session ─────────────────────────────────────────────────────────────────

/// A live collaboration session between a browser host and one or more agents.
#[derive(Clone, Debug)]
pub struct Session {
    /// Unique session identifier.
    pub id: String,
    /// The notebook this session is scoped to.
    pub notebook_id: String,
    /// Blake3 hash of the token (plaintext is never stored).
    pub token_hash: String,
    /// What connected agents are allowed to do.
    pub permissions: Permissions,
    /// When this session was created.
    pub created_at: DateTime<Utc>,
    /// When this session expires (regardless of activity).
    pub expires_at: DateTime<Utc>,
    /// Identifier of the browser WebSocket connection that owns this session.
    /// When this connection drops, the session is invalidated.
    pub browser_connection_id: String,
}

/// Default session TTL: 24 hours.
const DEFAULT_TTL_HOURS: i64 = 24;

// ── Session store ───────────────────────────────────────────────────────────

/// Thread-safe, in-memory session store.
#[derive(Clone, Debug, Default)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

/// Result of creating a new session. The plaintext token is returned exactly
/// once — the store only keeps the hash.
#[derive(Debug)]
pub struct CreateSessionResult {
    pub session_id: String,
    /// Plaintext token to give to the agent. Shown once, never stored.
    pub token: String,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new session for a notebook, returning the session ID and
    /// plaintext token.
    pub async fn create_session(
        &self,
        notebook_id: String,
        browser_connection_id: String,
        permissions: Permissions,
    ) -> CreateSessionResult {
        let session_id = uuid::Uuid::new_v4().to_string();
        let token = generate_token();
        let token_hash = hash_token(&token);
        let now = Utc::now();

        let session = Session {
            id: session_id.clone(),
            notebook_id,
            token_hash,
            permissions,
            created_at: now,
            expires_at: now + Duration::hours(DEFAULT_TTL_HOURS),
            browser_connection_id,
        };

        self.sessions
            .write()
            .await
            .insert(session_id.clone(), session);

        CreateSessionResult { session_id, token }
    }

    /// Validate a plaintext token and return the session if valid.
    pub async fn validate_token(&self, token: &str) -> Result<Session, ValidateError> {
        let token_hash = hash_token(token);
        let sessions = self.sessions.read().await;
        let session = sessions
            .values()
            .find(|s| s.token_hash == token_hash)
            .ok_or(ValidateError::InvalidToken)?;

        if session.expires_at < Utc::now() {
            return Err(ValidateError::SessionExpired);
        }

        Ok(session.clone())
    }

    /// Look up a session by ID.
    pub async fn get_session(&self, session_id: &str) -> Option<Session> {
        let sessions = self.sessions.read().await;
        let session = sessions.get(session_id)?;

        if session.expires_at < Utc::now() {
            return None;
        }

        Some(session.clone())
    }

    /// Remove a specific session.
    pub async fn invalidate_session(&self, session_id: &str) -> bool {
        self.sessions.write().await.remove(session_id).is_some()
    }

    /// Force a session to be expired *without* removing it, so a subsequent
    /// [`validate_token`](Self::validate_token) returns
    /// [`ValidateError::SessionExpired`] (HTTP 410) rather than
    /// [`ValidateError::InvalidToken`] (HTTP 401). Unlike
    /// [`invalidate_session`](Self::invalidate_session), the record is retained
    /// (the token still matches a session), which is what distinguishes "this
    /// session ended" from "no such token". Returns `true` if the session
    /// existed.
    pub async fn expire_session(&self, session_id: &str) -> bool {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.expires_at = Utc::now() - Duration::seconds(1);
            true
        } else {
            false
        }
    }

    /// Remove all sessions owned by a specific browser connection.
    ///
    /// Called when the browser's WebSocket disconnects — all sessions for
    /// that connection become invalid since the model server is gone.
    pub async fn invalidate_by_connection(&self, browser_connection_id: &str) -> Vec<String> {
        let mut sessions = self.sessions.write().await;
        let to_remove: Vec<String> = sessions
            .values()
            .filter(|s| s.browser_connection_id == browser_connection_id)
            .map(|s| s.id.clone())
            .collect();

        for id in &to_remove {
            sessions.remove(id);
        }

        to_remove
    }

    /// Return all non-expired sessions.
    pub async fn all_sessions(&self) -> Vec<Session> {
        let sessions = self.sessions.read().await;
        let now = Utc::now();
        sessions
            .values()
            .filter(|s| s.expires_at > now)
            .cloned()
            .collect()
    }

    /// IDs of non-expired sessions for `notebook_id` whose permissions grant
    /// `read`. Filters under the read lock and clones only the ids — this runs
    /// on the hot event-broadcast path, where cloning whole [`Session`]s (as
    /// [`all_sessions`](Self::all_sessions) does) per event would be wasteful.
    pub async fn readable_session_ids(&self, notebook_id: &str) -> Vec<String> {
        let sessions = self.sessions.read().await;
        let now = Utc::now();
        sessions
            .values()
            .filter(|s| s.notebook_id == notebook_id && s.permissions.read && s.expires_at > now)
            .map(|s| s.id.clone())
            .collect()
    }

    /// Remove expired sessions, returning their ids so the caller can also
    /// disconnect their guests. Call periodically to prevent unbounded
    /// growth — and to actually END expired sessions: permissions are
    /// checked against the value captured at connect time, so a
    /// still-connected guest outlives its session unless someone drops it
    /// (see [`WsState::sweep_expired_sessions`](crate::state::WsState)).
    pub async fn sweep_expired(&self) -> Vec<String> {
        let mut sessions = self.sessions.write().await;
        let now = Utc::now();
        let mut swept = Vec::new();
        sessions.retain(|id, s| {
            let keep = s.expires_at > now;
            if !keep {
                swept.push(id.clone());
            }
            keep
        });
        swept
    }
}

// ── Permission checking ─────────────────────────────────────────────────────

/// Check whether a session's permissions allow the given message.
pub fn check_permission(permissions: &Permissions, message: &MessageKind) -> bool {
    match message {
        MessageKind::Query(_) => permissions.read,
        MessageKind::Mutation(_) => permissions.write,
        // Events, responses, and control messages are always allowed — they
        // flow from the model/relay to clients and are not permission-gated.
        MessageKind::Event(_) | MessageKind::Response(_) | MessageKind::Control(_) => true,
    }
}

// ── Token helpers ───────────────────────────────────────────────────────────

/// Generate a cryptographically random 64-character hex token.
fn generate_token() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    // blake3 gives us hex encoding for free.
    blake3::Hash::from_bytes(bytes).to_hex().to_string()
}

/// Hash a plaintext token with blake3 (the store never keeps plaintext).
fn hash_token(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ironpad_common::protocol::Mutation;

    #[test]
    fn token_is_64_hex_chars() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn token_is_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_deterministic() {
        let token = "abc123";
        assert_eq!(hash_token(token), hash_token(token));
    }

    #[test]
    fn hash_differs_for_different_tokens() {
        assert_ne!(hash_token("token_a"), hash_token("token_b"));
    }

    #[tokio::test]
    async fn create_and_validate() {
        let store = SessionStore::new();
        let result = store
            .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
            .await;

        assert_eq!(result.token.len(), 64);

        let session = store
            .validate_token(&result.token)
            .await
            .expect("valid token");
        assert_eq!(session.id, result.session_id);
        assert_eq!(session.notebook_id, "nb-1");
        assert!(session.permissions.read);
        assert!(session.permissions.write);
    }

    #[tokio::test]
    async fn invalid_token_returns_none() {
        let store = SessionStore::new();
        store
            .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
            .await;

        assert_eq!(
            store.validate_token("wrong_token").await.unwrap_err(),
            ValidateError::InvalidToken
        );
    }

    #[tokio::test]
    async fn invalidate_session_removes_it() {
        let store = SessionStore::new();
        let result = store
            .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
            .await;

        assert!(store.invalidate_session(&result.session_id).await);
        assert!(store.validate_token(&result.token).await.is_err());
    }

    #[tokio::test]
    async fn invalidate_by_connection() {
        let store = SessionStore::new();

        // Two sessions on the same connection.
        let r1 = store
            .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
            .await;
        let r2 = store
            .create_session("nb-2".into(), "conn-1".into(), Permissions::default())
            .await;
        // One session on a different connection.
        let r3 = store
            .create_session("nb-3".into(), "conn-2".into(), Permissions::default())
            .await;

        let removed = store.invalidate_by_connection("conn-1").await;
        assert_eq!(removed.len(), 2);

        assert!(store.validate_token(&r1.token).await.is_err());
        assert!(store.validate_token(&r2.token).await.is_err());
        assert!(store.validate_token(&r3.token).await.is_ok());
    }

    #[tokio::test]
    async fn expired_session_is_rejected() {
        let store = SessionStore::new();
        let result = store
            .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
            .await;

        // Manually expire the session.
        {
            let mut sessions = store.sessions.write().await;
            if let Some(s) = sessions.get_mut(&result.session_id) {
                s.expires_at = Utc::now() - Duration::hours(1);
            }
        }

        assert_eq!(
            store.validate_token(&result.token).await.unwrap_err(),
            ValidateError::SessionExpired
        );
    }

    #[tokio::test]
    async fn expire_session_forces_expired_validation() {
        let store = SessionStore::new();
        let result = store
            .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
            .await;

        assert!(store.expire_session(&result.session_id).await);

        // The token still matches a session, but it now reports as expired
        // (→ HTTP 410) rather than invalid (→ HTTP 401).
        assert_eq!(
            store.validate_token(&result.token).await.unwrap_err(),
            ValidateError::SessionExpired
        );

        // An unknown session id is a no-op.
        assert!(!store.expire_session("no-such-session").await);
    }

    #[tokio::test]
    async fn readable_session_ids_filters_by_notebook_read_and_expiry() {
        let store = SessionStore::new();

        // Readable session on the target notebook — included.
        let readable = store
            .create_session(
                "nb-1".into(),
                "conn-1".into(),
                Permissions {
                    read: true,
                    write: true,
                },
            )
            .await;
        // Write-only session on the target notebook — excluded (read: false).
        store
            .create_session(
                "nb-1".into(),
                "conn-1".into(),
                Permissions {
                    read: false,
                    write: true,
                },
            )
            .await;
        // Readable session on a different notebook — excluded (wrong notebook).
        store
            .create_session("nb-2".into(), "conn-1".into(), Permissions::default())
            .await;
        // Readable-but-expired session on the target notebook — excluded.
        let expired = store
            .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
            .await;
        assert!(store.expire_session(&expired.session_id).await);

        let ids = store.readable_session_ids("nb-1").await;
        assert_eq!(
            ids,
            vec![readable.session_id],
            "only the non-expired, readable session for nb-1 should be returned"
        );
    }

    #[tokio::test]
    async fn sweep_expired_removes_only_expired() {
        let store = SessionStore::new();

        let r1 = store
            .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
            .await;
        store
            .create_session("nb-2".into(), "conn-1".into(), Permissions::default())
            .await;

        // Expire one session.
        {
            let mut sessions = store.sessions.write().await;
            if let Some(s) = sessions.get_mut(&r1.session_id) {
                s.expires_at = Utc::now() - Duration::hours(1);
            }
        }

        let swept = store.sweep_expired().await;
        assert_eq!(swept, vec![r1.session_id]);

        // One session should remain.
        let sessions = store.sessions.read().await;
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn permission_read_allows_queries() {
        let perms = Permissions {
            read: true,
            write: false,
        };
        let msg = MessageKind::Query(ironpad_common::protocol::Query::CellsList);
        assert!(check_permission(&perms, &msg));
    }

    #[test]
    fn permission_write_allows_mutations() {
        let perms = Permissions {
            read: false,
            write: true,
        };
        let msg = MessageKind::Mutation(Mutation::CellReorder { cell_ids: vec![] });
        assert!(check_permission(&perms, &msg));
    }

    #[test]
    fn permission_no_write_denies_mutations() {
        let perms = Permissions {
            read: true,
            write: false,
        };
        let msg = MessageKind::Mutation(Mutation::CellReorder { cell_ids: vec![] });
        assert!(!check_permission(&perms, &msg));
    }

    #[test]
    fn permission_no_read_denies_queries() {
        let perms = Permissions {
            read: false,
            write: true,
        };
        let msg = MessageKind::Query(ironpad_common::protocol::Query::NotebookGet);
        assert!(!check_permission(&perms, &msg));
    }
}
