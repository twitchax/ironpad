//! Unified message protocol for notebook collaboration.
//!
//! All interaction with the notebook model — whether from the browser UI,
//! a CLI agent, or any future client — uses these types. Mutations, queries,
//! events, and responses share a common envelope so they can travel over
//! the same WebSocket channel.

use serde::{Deserialize, Serialize};

use crate::types::{CellManifest, CellType, Diagnostic, IronpadCell, IronpadNotebook};

// ── Envelope ────────────────────────────────────────────────────────────────

/// Current wire protocol version.
///
/// Advisory only. It exists for observability and future capability
/// negotiation — peers do **not** hard-reject on mismatch, since that would
/// break interop between adjacent versions. Bump it whenever the message
/// schema changes in a way a peer should be able to notice (a new payload
/// variant, an added field, …). Decode sites (in the server/CLI crates) may
/// log a warning when a received version differs from this constant.
pub const PROTOCOL_VERSION: u32 = 1;

/// Top-level message envelope. Every frame on the wire is one of these.
///
/// # Forward / backward compatibility
///
/// The envelope carries no `#[serde(deny_unknown_fields)]`, so a frame minted
/// by a newer peer that carries envelope keys an older peer doesn't know — a
/// future top-level `version`, or any other added field — still deserializes
/// (unknown keys are ignored, not rejected). Symmetrically, a frame from a peer
/// that predates a field simply omits it. This tolerance is regression-locked
/// by `envelope_tolerates_unknown_future_fields` and
/// `envelope_without_version_still_parses`, and is what lets [`PROTOCOL_VERSION`]
/// be introduced on the wire later without a flag day.
///
/// Added *enum variants* in the payload sub-enums ([`Mutation`], [`Query`],
/// [`Event`], [`Response`], [`ControlMessage`]) are tolerated too: each carries
/// a `#[serde(other)] Unknown` arm, so a new `action`/`event`/`query`/`response`/
/// `control` tag from a newer peer decodes to that enum's `Unknown` instead of
/// failing the whole `Message` (regression-locked by
/// `unknown_payload_variant_decodes_to_unknown`). Consumers handle `Unknown` by
/// dropping the frame with a warning rather than acting on it. This is the case
/// that matters: a new *variant within an existing category* is the normal way
/// the protocol grows, and it is the one that could otherwise stall a correlated
/// request (e.g. a `Response::Unknown` that never resolves its oneshot).
///
/// A wholly-unknown top-level `type` (a new message *category* beyond the five
/// here) is **not** decoded to a variant — [`MessageKind`] is adjacently tagged
/// (`type`/`payload`), where serde's `#[serde(other)]` can't consume the
/// `payload`. Such a frame fails to parse and is dropped-with-a-warning at the
/// decode site, which is safe: a new top-level category is never a correlated
/// `Response`, so it can't stall anything. The five categories are
/// architecturally stable, so this is a documented limitation, not a gap.
///
/// A deliberate `#[non_exhaustive]` is *not* applied to any of these — every
/// variant is defined in-repo, so keeping the `match`es exhaustive means adding
/// a real variant still forces every consumer to decide how to handle it (T-016).
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
    NotebookUpdateMeta {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shared_cargo_toml: Option<Option<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shared_source: Option<Option<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reactive_mode: Option<bool>,
    },
    /// An unrecognised mutation from a newer peer (see the [`Message`] forward-compat docs).
    #[serde(other)]
    Unknown,
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
    CellGet {
        cell_id: String,
    },
    CellsList,
    /// An unrecognised query from a newer peer (see the [`Message`] forward-compat docs).
    #[serde(other)]
    Unknown,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reactive_mode: Option<bool>,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    /// An unrecognised event from a newer peer (see the [`Message`] forward-compat docs).
    #[serde(other)]
    Unknown,
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
    },
    CellsList {
        cells: Vec<CellManifest>,
    },
    MutationOk {
        /// Echoed back so the client knows which mutation succeeded.
        detail: MutationResult,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    /// An unrecognised response from a newer peer (see the [`Message`] forward-compat docs).
    #[serde(other)]
    Unknown,
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
    /// Host → Server: keep-alive. Lets the relay detect a half-open connection
    /// (a network drop with no FIN) via a read-loop idle timeout and tear it
    /// down, instead of leaving a dead host registered indefinitely.
    Heartbeat,
    /// Browser → Server: the first frame on the host socket. Proves the claimant
    /// holds the per-notebook host secret before the relay registers it as host
    /// (PRD-0038 T-014). A mismatched/missing claim is rejected with a WS close
    /// (4403/4400); it is never broadcast.
    ClaimHost { secret: String },
    /// An unrecognised control message from a newer peer (see
    /// the [`Message`] forward-compat docs).
    #[serde(other)]
    Unknown,
}

// ── Permissions ─────────────────────────────────────────────────────────────

/// What a session token authorizes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Permissions {
    /// Can query cells and notebook state.
    pub read: bool,
    /// Can mutate cells (add, update, delete, reorder, update metadata).
    pub write: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            read: true,
            write: true,
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
                reactive_mode: None,
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
    fn control_claim_host_round_trips() {
        // The browser's first frame on the host socket (PRD-0038 T-014).
        let msg = Message {
            id: "host-1".into(),
            kind: MessageKind::Control(ControlMessage::ClaimHost {
                secret: "deadbeefcafef00d".into(),
            }),
        };
        round_trip(&msg);

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""control":"ClaimHost""#), "{json}");
        assert!(json.contains(r#""secret":"deadbeefcafef00d""#), "{json}");
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

    // ── reactive_mode in protocol messages ──────────────────────────────

    #[test]
    fn mutation_notebook_update_meta_with_reactive_mode() {
        let msg = Message {
            id: "req-reactive".into(),
            kind: MessageKind::Mutation(Mutation::NotebookUpdateMeta {
                title: None,
                shared_cargo_toml: None,
                shared_source: None,
                reactive_mode: Some(true),
            }),
        };
        round_trip(&msg);

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"reactive_mode\":true"));
    }

    #[test]
    fn mutation_notebook_update_meta_omits_none_reactive_mode() {
        let msg = Message {
            id: "req-no-reactive".into(),
            kind: MessageKind::Mutation(Mutation::NotebookUpdateMeta {
                title: Some("Title".into()),
                shared_cargo_toml: None,
                shared_source: None,
                reactive_mode: None,
            }),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            !json.contains("reactive_mode"),
            "reactive_mode=None should be omitted"
        );
        round_trip(&msg);
    }

    #[test]
    fn event_notebook_meta_updated_with_reactive_mode() {
        let msg = Message {
            id: "evt-reactive".into(),
            kind: MessageKind::Event(EventEnvelope {
                by: ClientId::browser(),
                event: Event::NotebookMetaUpdated {
                    title: None,
                    shared_cargo_toml: None,
                    shared_source: None,
                    reactive_mode: Some(false),
                },
            }),
        };
        round_trip(&msg);

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"reactive_mode\":false"));
    }

    // ── Protocol versioning + forward compatibility ────────────────────

    #[test]
    fn protocol_version_is_advisory_constant() {
        // The version travels for observability / negotiation, not rejection.
        // Bumping it is a deliberate, reviewed act — lock the current value so
        // an accidental change is caught.
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    /// Forward-compat (new → old), envelope level: a frame minted by a newer
    /// peer may carry envelope keys an older peer doesn't know — a future
    /// top-level `version`, or any other added field. Because [`Message`] does
    /// not `deny_unknown_fields`, such a frame must still deserialize (unknown
    /// keys are ignored, not rejected) rather than stalling the correlated
    /// oneshot. This is the safe half of T-012 that lands without touching the
    /// downstream `match`es.
    #[test]
    fn envelope_tolerates_unknown_future_fields() {
        let wire = r#"{"id":"req-1","version":99,"unknown_future":true,"type":"Query","payload":{"query":"CellsList"}}"#;
        let msg: Message =
            serde_json::from_str(wire).expect("unknown envelope fields must be ignored");
        assert_eq!(msg.id, "req-1");
        assert!(matches!(msg.kind, MessageKind::Query(Query::CellsList)));
    }

    /// Backward-compat (old → new), envelope level: a frame from a peer that
    /// predates versioning carries no `version` key at all. It must still parse
    /// (every existing round-trip test already exercises this shape, since no
    /// version field exists yet).
    #[test]
    fn envelope_without_version_still_parses() {
        let wire = r#"{"id":"req-1","type":"Query","payload":{"query":"CellsList"}}"#;
        let msg: Message = serde_json::from_str(wire).expect("versionless frame must parse");
        assert!(matches!(msg.kind, MessageKind::Query(Query::CellsList)));
    }

    /// Forward-compat (new → old), payload-variant level: a new inner variant
    /// (`action`/`event`/`control`/…) within a KNOWN top-level type now decodes
    /// to that sub-enum's `Unknown` arm instead of failing the ENTIRE `Message`,
    /// so consumers drop-and-warn rather than stalling a correlated request.
    /// This is the case that grows the protocol across versions and the one that
    /// could otherwise stall (e.g. an unresolved `Response`). Closes the gap the
    /// old `unknown_payload_tag_currently_fails_whole_message` test documented.
    /// See PRD-0038 T-016.
    #[test]
    fn unknown_payload_variant_decodes_to_unknown() {
        // Unknown mutation action → Mutation::Unknown.
        let wire = r#"{"id":"req-1","type":"Mutation","payload":{"action":"CellTeleport","cell_id":"c1"}}"#;
        let msg: Message =
            serde_json::from_str(wire).expect("unknown action must decode to Mutation::Unknown");
        assert!(matches!(msg.kind, MessageKind::Mutation(Mutation::Unknown)));

        // Unknown control message → ControlMessage::Unknown.
        let wire = r#"{"id":"c","type":"Control","payload":{"control":"Reboot"}}"#;
        let msg: Message = serde_json::from_str(wire)
            .expect("unknown control must decode to ControlMessage::Unknown");
        assert!(matches!(
            msg.kind,
            MessageKind::Control(ControlMessage::Unknown)
        ));

        // Unknown event (inside the EventEnvelope) → Event::Unknown.
        let wire = r#"{"id":"e","type":"Event","payload":{"by":"browser","event":{"event":"NewFangled"}}}"#;
        let msg: Message =
            serde_json::from_str(wire).expect("unknown event must decode to Event::Unknown");
        assert!(matches!(
            msg.kind,
            MessageKind::Event(EventEnvelope {
                event: Event::Unknown,
                ..
            })
        ));

        // A wholly-unknown top-level `type` (a new message CATEGORY) is not
        // decoded to a variant — the adjacently-tagged envelope can't route it.
        // It fails to parse and is dropped-with-a-warning at the decode site,
        // which is safe: such a frame is never a correlated Response, so it
        // can't stall a pending request. (See the `Message` docs.)
        let wire = r#"{"id":"m","type":"Telemetry","payload":{"whatever":1}}"#;
        assert!(serde_json::from_str::<Message>(wire).is_err());
    }

    #[test]
    fn event_notebook_meta_updated_omits_none_reactive_mode() {
        let msg = Message {
            id: "evt-no-reactive".into(),
            kind: MessageKind::Event(EventEnvelope {
                by: ClientId::agent("bot"),
                event: Event::NotebookMetaUpdated {
                    title: Some("New".into()),
                    shared_cargo_toml: None,
                    shared_source: None,
                    reactive_mode: None,
                },
            }),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            !json.contains("reactive_mode"),
            "reactive_mode=None should be omitted from event"
        );
        round_trip(&msg);
    }
}
