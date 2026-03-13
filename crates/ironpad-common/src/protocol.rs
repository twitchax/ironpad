//! Unified message protocol for notebook collaboration.
//!
//! All interaction with the notebook model — whether from the browser UI,
//! a CLI agent, or any future client — uses these types. Mutations, queries,
//! events, and responses share a common envelope so they can travel over
//! the same WebSocket channel.

use serde::{Deserialize, Serialize};

use crate::types::{CellManifest, CellType, Diagnostic, IronpadCell, IronpadNotebook};

// ── Envelope ────────────────────────────────────────────────────────────────

/// Top-level message envelope. Every frame on the wire is one of these.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    /// Correlation ID. Responses and events reference the mutation/query that
    /// caused them so clients can match request → response.
    pub id: String,
    /// The payload.
    #[serde(flatten)]
    pub kind: MessageKind,
}

/// Discriminated union of all message types.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum MessageKind {
    Mutation(Mutation),
    Query(Query),
    Event(EventEnvelope),
    Response(Response),
    Control(ControlMessage),
}

// ── Client Identity ─────────────────────────────────────────────────────────

/// Opaque identifier for the source of a mutation.
///
/// The protocol doesn't ascribe meaning to this — the UI can use it to
/// distinguish "my edit" from "agent's edit" for display purposes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ClientId(pub String);

impl ClientId {
    pub fn browser() -> Self {
        Self("browser".to_string())
    }

    pub fn agent(token_prefix: &str) -> Self {
        Self(format!("agent:{token_prefix}"))
    }
}

// ── Mutations (client → model) ──────────────────────────────────────────────

/// A request to change notebook state. Any client can send these
/// (subject to permissions).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum Mutation {
    CellAdd {
        cell: NewCell,
        /// Insert after this cell. `None` = insert at beginning.
        after_cell_id: Option<String>,
    },
    CellUpdate {
        cell_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cargo_toml: Option<Option<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// Expected current version (optimistic concurrency control).
        version: u64,
    },
    CellDelete {
        cell_id: String,
        /// Expected current version.
        version: u64,
    },
    CellReorder {
        /// Complete ordered list of all cell IDs.
        cell_ids: Vec<String>,
    },
    CellCompile {
        cell_id: String,
    },
    CellExecute {
        cell_id: String,
    },
    NotebookUpdateMeta {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shared_cargo_toml: Option<Option<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shared_source: Option<Option<String>>,
    },
}

/// Data for creating a new cell. The model assigns `id`, `order`, and `version`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewCell {
    pub source: String,
    #[serde(default)]
    pub cell_type: CellType,
    #[serde(default = "default_cell_label")]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cargo_toml: Option<String>,
}

fn default_cell_label() -> String {
    "New Cell".to_string()
}

// ── Queries (client → model) ────────────────────────────────────────────────

/// Read-only requests. Responses are sent only to the requesting client.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "query")]
pub enum Query {
    NotebookGet,
    CellGet { cell_id: String },
    CellsList,
    SessionStatus,
}

// ── Events (model → all clients) ────────────────────────────────────────────

/// Wraps an event with its origin and correlation ID.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventEnvelope {
    /// Who caused this event.
    pub by: ClientId,
    /// The event payload.
    pub event: Event,
}

/// Broadcast to every connected client when notebook state changes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum Event {
    CellAdded {
        cell: IronpadCell,
        after_cell_id: Option<String>,
        version: u64,
    },
    CellUpdated {
        cell_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cargo_toml: Option<Option<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        version: u64,
    },
    CellDeleted {
        cell_id: String,
    },
    CellReordered {
        cell_ids: Vec<String>,
    },
    CellCompiling {
        cell_id: String,
    },
    CellCompiled {
        cell_id: String,
        diagnostics: Vec<Diagnostic>,
        success: bool,
    },
    CellExecuted {
        cell_id: String,
        display_text: Option<String>,
        type_tag: Option<String>,
        execution_time_ms: f64,
    },
    NotebookMetaUpdated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shared_cargo_toml: Option<Option<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shared_source: Option<Option<String>>,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

// ── Responses (model → requesting client) ───────────────────────────────────

/// Direct response to a [`Query`]. Sent only to the client that asked.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "response")]
pub enum Response {
    Notebook {
        notebook: IronpadNotebook,
    },
    Cell {
        cell: IronpadCell,
        version: u64,
    },
    CellsList {
        cells: Vec<CellManifest>,
    },
    SessionStatus {
        session_id: String,
        connected_clients: u32,
    },
    MutationOk {
        /// Echoed back so the client knows which mutation succeeded.
        detail: MutationResult,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

/// Specific result data for a successful mutation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "result")]
pub enum MutationResult {
    CellAdded { cell_id: String, version: u64 },
    CellUpdated { cell_id: String, version: u64 },
    CellDeleted { cell_id: String },
    CellReordered,
    NotebookMetaUpdated,
}

// ── Control Messages (session management) ───────────────────────────────────

/// Session lifecycle messages between the browser/CLI and the server.
/// These are not part of the notebook protocol — they manage the transport.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "control")]
pub enum ControlMessage {
    /// Host → Server: create a new agent session for this notebook.
    CreateSession {
        #[serde(default)]
        permissions: Permissions,
    },
    /// Server → Host: session created, here's the token.
    SessionCreated { session_id: String, token: String },
    /// Host → Server: end a session, disconnect all guests.
    EndSession { session_id: String },
    /// Server → Host/Guests: session has ended.
    SessionEnded { session_id: String },
    /// Server → Host: a guest connected.
    GuestConnected { client_id: ClientId },
    /// Server → Host: a guest disconnected.
    GuestDisconnected { client_id: ClientId },
}

// ── Permissions ─────────────────────────────────────────────────────────────

/// What a session token authorizes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Permissions {
    /// Can query cells and notebook state.
    pub read: bool,
    /// Can mutate cells (add, update, delete, reorder, update metadata).
    pub write: bool,
    /// Can trigger compilation and execution.
    pub execute: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            read: true,
            write: true,
            execute: false,
        }
    }
}

// ── Error Codes ─────────────────────────────────────────────────────────────

/// Structured error codes for protocol-level failures.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCode {
    /// OCC: the client's version is stale.
    VersionConflict,
    CellNotFound,
    NotebookNotFound,
    PermissionDenied,
    InvalidMessage,
    SessionNotFound,
    SessionExpired,
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: round-trip a value through JSON and assert equality.
    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + std::fmt::Debug>(value: &T) {
        let json = serde_json::to_string(value).expect("serialize");
        let back: T = serde_json::from_str(&json).expect("deserialize");
        // Re-serialize to compare (since we don't require PartialEq on all types).
        let json2 = serde_json::to_string(&back).expect("re-serialize");
        assert_eq!(json, json2, "round-trip mismatch");
    }

    #[test]
    fn mutation_cell_add() {
        let msg = Message {
            id: "req-1".into(),
            kind: MessageKind::Mutation(Mutation::CellAdd {
                cell: NewCell {
                    source: "let x = 42;".into(),
                    cell_type: CellType::Code,
                    label: "My Cell".into(),
                    cargo_toml: None,
                },
                after_cell_id: Some("cell-0".into()),
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn mutation_cell_update() {
        let msg = Message {
            id: "req-2".into(),
            kind: MessageKind::Mutation(Mutation::CellUpdate {
                cell_id: "cell-1".into(),
                source: Some("let x = 99;".into()),
                cargo_toml: None,
                label: None,
                version: 3,
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn mutation_cell_delete() {
        let msg = Message {
            id: "req-3".into(),
            kind: MessageKind::Mutation(Mutation::CellDelete {
                cell_id: "cell-1".into(),
                version: 5,
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn mutation_cell_reorder() {
        let msg = Message {
            id: "req-4".into(),
            kind: MessageKind::Mutation(Mutation::CellReorder {
                cell_ids: vec!["c".into(), "a".into(), "b".into()],
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn mutation_notebook_update_meta() {
        let msg = Message {
            id: "req-5".into(),
            kind: MessageKind::Mutation(Mutation::NotebookUpdateMeta {
                title: Some("New Title".into()),
                shared_cargo_toml: Some(Some("toml content".into())),
                shared_source: None,
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn query_notebook_get() {
        let msg = Message {
            id: "req-6".into(),
            kind: MessageKind::Query(Query::NotebookGet),
        };
        round_trip(&msg);
    }

    #[test]
    fn query_cell_get() {
        let msg = Message {
            id: "req-7".into(),
            kind: MessageKind::Query(Query::CellGet {
                cell_id: "cell-1".into(),
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn event_cell_updated() {
        let msg = Message {
            id: "req-2".into(),
            kind: MessageKind::Event(EventEnvelope {
                by: ClientId::agent("abc123"),
                event: Event::CellUpdated {
                    cell_id: "cell-1".into(),
                    source: Some("let x = 99;".into()),
                    cargo_toml: None,
                    label: None,
                    version: 4,
                },
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn event_cell_compiled() {
        let msg = Message {
            id: "req-8".into(),
            kind: MessageKind::Event(EventEnvelope {
                by: ClientId::browser(),
                event: Event::CellCompiled {
                    cell_id: "cell-1".into(),
                    diagnostics: vec![],
                    success: true,
                },
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn event_cell_executed() {
        let msg = Message {
            id: "req-9".into(),
            kind: MessageKind::Event(EventEnvelope {
                by: ClientId::browser(),
                event: Event::CellExecuted {
                    cell_id: "cell-1".into(),
                    display_text: Some("42".into()),
                    type_tag: Some("u32".into()),
                    execution_time_ms: 1.5,
                },
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn event_error() {
        let msg = Message {
            id: "req-2".into(),
            kind: MessageKind::Event(EventEnvelope {
                by: ClientId::browser(),
                event: Event::Error {
                    code: ErrorCode::VersionConflict,
                    message: "Expected version 3, actual 5".into(),
                },
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn response_cells_list() {
        let msg = Message {
            id: "req-7".into(),
            kind: MessageKind::Response(Response::CellsList {
                cells: vec![CellManifest {
                    id: "cell-1".into(),
                    order: 0,
                    label: "First".into(),
                    cell_type: CellType::Code,
                }],
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn response_mutation_ok() {
        let msg = Message {
            id: "req-2".into(),
            kind: MessageKind::Response(Response::MutationOk {
                detail: MutationResult::CellUpdated {
                    cell_id: "cell-1".into(),
                    version: 4,
                },
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn response_error() {
        let msg = Message {
            id: "req-10".into(),
            kind: MessageKind::Response(Response::Error {
                code: ErrorCode::PermissionDenied,
                message: "Token does not allow execute".into(),
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn control_create_session() {
        let msg = Message {
            id: "ctrl-1".into(),
            kind: MessageKind::Control(ControlMessage::CreateSession {
                permissions: Permissions::default(),
            }),
        };
        round_trip(&msg);

        // Verify default permissions.
        let perms = Permissions::default();
        assert!(perms.read);
        assert!(perms.write);
        assert!(!perms.execute);
    }

    #[test]
    fn control_session_created() {
        let msg = Message {
            id: "ctrl-1".into(),
            kind: MessageKind::Control(ControlMessage::SessionCreated {
                session_id: "sess-1".into(),
                token: "a1b2c3d4".into(),
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn control_guest_connected() {
        let msg = Message {
            id: "ctrl-2".into(),
            kind: MessageKind::Control(ControlMessage::GuestConnected {
                client_id: ClientId::agent("abc"),
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn client_id_constructors() {
        assert_eq!(ClientId::browser().0, "browser");
        assert_eq!(ClientId::agent("abc123").0, "agent:abc123");
    }

    #[test]
    fn permissions_custom() {
        let perms = Permissions {
            read: true,
            write: false,
            execute: true,
        };
        round_trip(&perms);
    }

    #[test]
    fn new_cell_defaults() {
        let json = r#"{"source":"hello"}"#;
        let cell: NewCell = serde_json::from_str(json).unwrap();
        assert_eq!(cell.cell_type, CellType::Code);
        assert_eq!(cell.label, "New Cell");
        assert!(cell.cargo_toml.is_none());
    }
}
