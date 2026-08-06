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
pub const PROTOCOL_VERSION: u32 = 5;

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

// ── Notebook metadata ───────────────────────────────────────────────────────

/// The notebook-level fields one metadata mutation can change.
///
/// Every field is tri-state: `None` leaves it alone, `Some(None)` clears it,
/// and `Some(Some(v))` sets it. `title` is the exception, since a notebook
/// cannot be untitled, and `reactive_mode` because `false` is its cleared form.
///
/// Flattened into both [`Mutation::NotebookUpdateMeta`] and
/// [`Event::NotebookMetaUpdated`], which is what keeps the two from drifting: a
/// connected guest rebuilds its cached notebook from the event, so a field the
/// mutation can carry and the event cannot is silent data loss. They used to be
/// two hand-maintained copies of the same list. Flattening leaves the wire
/// format byte-identical, since the fields still sit directly beside `action`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NotebookMetaPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "explicit_null_is_a_clear"
    )]
    pub shared_cargo_toml: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "explicit_null_is_a_clear"
    )]
    pub shared_source: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reactive_mode: Option<bool>,

    // ── Presentation metadata ───────────────────────────────────────────
    // What a link unfurl is built from. Clearable, hence the doubled option:
    // these are the fields a user is most likely to set once and then want
    // gone.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "explicit_null_is_a_clear"
    )]
    pub description: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "explicit_null_is_a_clear"
    )]
    pub tags: Option<Option<Vec<String>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "explicit_null_is_a_clear"
    )]
    pub og_image: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "explicit_null_is_a_clear"
    )]
    pub og_image_width: Option<Option<u32>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "explicit_null_is_a_clear"
    )]
    pub og_image_height: Option<Option<u32>>,
}

impl NotebookMetaPatch {
    /// Applies this patch to `notebook`, leaving untouched every field the
    /// patch says nothing about.
    ///
    /// Lives here rather than in either caller because there are two: the
    /// browser's model applies it as the authoritative mutation, and the CLI
    /// daemon applies the mirrored event to its cached copy. Those were
    /// separate hand-written field-by-field copies, which is exactly how an
    /// agent's view of a notebook drifts from the browser's.
    pub fn apply_to(&self, notebook: &mut crate::types::IronpadNotebook) {
        if let Some(title) = &self.title {
            notebook.title.clone_from(title);
        }
        if let Some(value) = &self.shared_cargo_toml {
            notebook.shared_cargo_toml.clone_from(value);
        }
        if let Some(value) = &self.shared_source {
            notebook.shared_source.clone_from(value);
        }
        if let Some(on) = self.reactive_mode {
            // `false` is stored as absent, which is what every reader already
            // treats as off.
            notebook.reactive_mode = on.then_some(true);
        }
        if let Some(value) = &self.description {
            notebook.description.clone_from(value);
        }
        if let Some(value) = &self.tags {
            notebook.tags.clone_from(value);
        }
        if let Some(value) = &self.og_image {
            notebook.og_image.clone_from(value);
        }
        if let Some(value) = self.og_image_width {
            notebook.og_image_width = value;
        }
        if let Some(value) = self.og_image_height {
            notebook.og_image_height = value;
        }
    }
}

/// Distinguishes an absent key from an explicit `null` for a doubled option.
///
/// Serde's default for `Option<Option<T>>` collapses both to `None`, which
/// silently drops every clear that crosses the wire: the sender means "unset
/// this", serializes `null`, and the receiver decodes "unchanged" and keeps the
/// old value. Nothing sent `Some(None)` before the presentation fields existed,
/// so the bug was latent; clearing a description is the first case that would
/// have hit it. Paired with `skip_serializing_if`, an absent key still means
/// unchanged, because the field never reaches this function at all.
// The doubled option IS the point here: three states, and clippy's suggested
// `Option<T>` collapses the two this function exists to tell apart.
#[allow(clippy::option_option)]
fn explicit_null_is_a_clear<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(deserializer).map(Some)
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
        /// `Some(None)` is an explicit clear. Without the custom
        /// deserializer the wire's `"cargo_toml": null` decoded as the
        /// OUTER `None` ("unchanged") and the clear was silently dropped.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "explicit_null_is_a_clear"
        )]
        cargo_toml: Option<Option<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// Toggle the shared-cell flag (PRD-0044). `None` leaves it unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shared: Option<bool>,
        /// Default collapse state for the code body (set from the cell's
        /// header toggle). `None` leaves it unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        collapsed: Option<bool>,
        /// Output panel starts collapsed. `None` leaves it unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_collapsed: Option<bool>,
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
        #[serde(flatten)]
        meta: NotebookMetaPatch,
    },
    /// Run a cell in the hosting browser (PRD-0052).
    ///
    /// Rides the `Mutation` envelope for the relay's write-permission gate,
    /// but it is NOT a state mutation: the browser dispatches it to the run
    /// queue instead of `model.apply`. Acked with
    /// [`MutationResult::CellRunStarted`]; results arrive as
    /// [`Event::CellCompiling`]/[`Event::CellCompiled`]/[`Event::CellExecuted`],
    /// correlated by `cell_id` (one run can cascade into prerequisites, so a
    /// message id cannot follow it).
    CellRun { cell_id: String },
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
    /// Create the cell as a shared cell (PRD-0044).
    #[serde(default)]
    pub shared: bool,
}

/// Default label for cells created without one. Single source of truth —
/// the CLI mirrors this instead of hard-coding the string.
pub const DEFAULT_CELL_LABEL: &str = "New Cell";

fn default_cell_label() -> String {
    DEFAULT_CELL_LABEL.to_string()
}

/// Serde default for fields added after their event shipped, where absent
/// must mean the old (success-only) semantics.
fn default_true() -> bool {
    true
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
        /// `Some(None)` is an explicit clear. Without the custom
        /// deserializer the wire's `"cargo_toml": null` decoded as the
        /// OUTER `None` ("unchanged") and the clear was silently dropped.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "explicit_null_is_a_clear"
        )]
        cargo_toml: Option<Option<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// Shared-cell flag change (PRD-0044). `None` = unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shared: Option<bool>,
        /// Default collapse-state changes. `None` = unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        collapsed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_collapsed: Option<bool>,
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
        /// Whether execution completed without a runtime error (PRD-0052).
        /// Defaults to `true` so a legacy event without the field keeps its
        /// old success-only semantics; an agent waiting on a run needs a
        /// terminal event on the failure path too.
        #[serde(default = "default_true")]
        success: bool,
    },
    NotebookMetaUpdated {
        #[serde(flatten)]
        meta: NotebookMetaPatch,
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
    CellAdded {
        cell_id: String,
        version: u64,
    },
    CellUpdated {
        cell_id: String,
        version: u64,
    },
    CellDeleted {
        cell_id: String,
    },
    CellReordered,
    NotebookMetaUpdated,
    /// A [`Mutation::CellRun`] was accepted and queued (PRD-0052). The run's
    /// outcome arrives separately as execution events for the cell.
    CellRunStarted {
        cell_id: String,
    },
    /// An unrecognised result from a newer peer (see the [`Message`]
    /// forward-compat docs). Without this arm, a new result variant nested
    /// in `Response::MutationOk` failed the whole message parse and the
    /// requester hung to its timeout.
    #[serde(other)]
    Unknown,
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
    /// Forward-compatibility catch-all: a code minted by a newer peer.
    /// Without it, one unknown code failed the WHOLE message deserialize,
    /// and the correlated request stalled to its timeout instead of
    /// surfacing the error (same rationale as `Event::Unknown`).
    #[serde(other)]
    Unknown,
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
                    shared: false,
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
                shared: None,
                collapsed: None,
                output_collapsed: None,
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
                meta: NotebookMetaPatch {
                    title: Some("New Title".into()),
                    shared_cargo_toml: Some(Some("toml content".into())),
                    ..Default::default()
                },
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn mutation_notebook_update_meta_carries_the_presentation_fields() {
        let msg = Message {
            id: "req-5b".into(),
            kind: MessageKind::Mutation(Mutation::NotebookUpdateMeta {
                meta: NotebookMetaPatch {
                    description: Some(Some("A description.".into())),
                    tags: Some(Some(vec!["blog".into(), "autodiff".into()])),
                    og_image: Some(Some("/og-custom/x.png".into())),
                    og_image_width: Some(Some(1024)),
                    og_image_height: Some(Some(1024)),
                    ..Default::default()
                },
            }),
        };
        round_trip(&msg);
    }

    #[test]
    fn flattening_the_patch_left_the_wire_format_alone() {
        // The fields moved into a struct for the Rust API's sake; a peer built
        // before that must still read this frame, so they have to stay beside
        // `action` rather than nesting under a `meta` key.
        let json = serde_json::to_value(Message {
            id: "req-5c".into(),
            kind: MessageKind::Mutation(Mutation::NotebookUpdateMeta {
                meta: NotebookMetaPatch {
                    title: Some("T".into()),
                    ..Default::default()
                },
            }),
        })
        .unwrap();

        let payload = &json["payload"];
        assert_eq!(payload["action"], "NotebookUpdateMeta");
        assert_eq!(payload["title"], "T");
        assert!(
            payload.get("meta").is_none(),
            "the patch must be flattened, not nested: {payload}"
        );
        // Untouched fields stay off the wire entirely.
        assert!(payload.get("description").is_none(), "{payload}");
    }

    #[test]
    fn an_explicit_null_clears_and_an_absent_key_does_not() {
        // Serde's default for Option<Option<T>> collapses both to `None`,
        // which drops every clear that crosses the wire: the sender means
        // "unset this" and the receiver decodes "unchanged".
        let parse = |payload: &str| -> NotebookMetaPatch {
            let msg: Message = serde_json::from_str(&format!(
                r#"{{"id":"x","type":"Mutation","payload":{payload}}}"#
            ))
            .unwrap();
            match msg.kind {
                MessageKind::Mutation(Mutation::NotebookUpdateMeta { meta }) => meta,
                other => panic!("unexpected: {other:?}"),
            }
        };

        let cleared = parse(r#"{"action":"NotebookUpdateMeta","description":null}"#);
        assert_eq!(cleared.description, Some(None), "null must mean clear");

        let untouched = parse(r#"{"action":"NotebookUpdateMeta","title":"T"}"#);
        assert_eq!(
            untouched.description, None,
            "an absent key must mean unchanged"
        );
    }

    #[test]
    fn cell_update_explicit_null_cargo_toml_survives_the_wire() {
        // Regression: `"cargo_toml": null` (a clear, Some(None)) used to
        // decode as the OUTER None ("unchanged") on both the mutation and
        // the event, silently dropping the clear.
        let m: Mutation = serde_json::from_str(
            r#"{"action":"CellUpdate","cell_id":"c1","cargo_toml":null,"version":1}"#,
        )
        .unwrap();
        match m {
            Mutation::CellUpdate {
                cargo_toml, source, ..
            } => {
                assert_eq!(cargo_toml, Some(None), "explicit null is a clear");
                assert_eq!(source, None, "absent key stays unchanged");
            }
            other => panic!("wrong variant: {other:?}"),
        }

        let e: Event = serde_json::from_str(
            r#"{"event":"CellUpdated","cell_id":"c1","cargo_toml":null,"version":1}"#,
        )
        .unwrap();
        match e {
            Event::CellUpdated { cargo_toml, .. } => assert_eq!(cargo_toml, Some(None)),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn unknown_error_code_deserializes_instead_of_failing_the_message() {
        // A code minted by a newer peer must not fail the whole frame (the
        // correlated request would stall to timeout).
        let code: ErrorCode = serde_json::from_str(r#""SomeFutureCode""#).unwrap();
        assert_eq!(code, ErrorCode::Unknown);
        // Known codes still round-trip.
        let known: ErrorCode = serde_json::from_str(r#""PermissionDenied""#).unwrap();
        assert_eq!(known, ErrorCode::PermissionDenied);
    }

    #[test]
    fn a_patch_applies_only_the_fields_it_names() {
        use crate::types::IronpadNotebook;

        let mut nb = IronpadNotebook::new("Original");
        nb.description = Some("keep me".into());
        nb.og_image = Some("/a.png".into());

        NotebookMetaPatch {
            title: Some("Renamed".into()),
            og_image: Some(None),
            og_image_width: Some(Some(800)),
            ..Default::default()
        }
        .apply_to(&mut nb);

        assert_eq!(nb.title, "Renamed");
        assert_eq!(nb.description.as_deref(), Some("keep me"), "not named");
        assert_eq!(nb.og_image, None, "Some(None) clears");
        assert_eq!(nb.og_image_width, Some(800));
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
                    shared: None,
                    collapsed: None,
                    output_collapsed: None,
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
                    success: true,
                },
            }),
        };
        round_trip(&msg);
    }

    /// A legacy `CellExecuted` (pre-PRD-0052, no `success` field) must parse
    /// as a success — that was the only case the old event ever reported.
    #[test]
    fn event_cell_executed_without_success_defaults_to_true() {
        let json = r#"{"id":"","type":"Event","payload":{"by":"browser","event":{"event":"CellExecuted","cell_id":"c1","display_text":null,"type_tag":null,"execution_time_ms":2.0}}}"#;
        let msg: Message = serde_json::from_str(json).expect("legacy event parses");
        let MessageKind::Event(envelope) = msg.kind else {
            panic!("expected event");
        };
        let Event::CellExecuted { success, .. } = envelope.event else {
            panic!("expected CellExecuted");
        };
        assert!(
            success,
            "absent success must mean the old success-only case"
        );
    }

    #[test]
    fn mutation_cell_run_round_trips_and_acks() {
        let msg = Message {
            id: "req-run".into(),
            kind: MessageKind::Mutation(Mutation::CellRun {
                cell_id: "cell-3".into(),
            }),
        };
        round_trip(&msg);

        let ack = Message {
            id: "req-run".into(),
            kind: MessageKind::Response(Response::MutationOk {
                detail: MutationResult::CellRunStarted {
                    cell_id: "cell-3".into(),
                },
            }),
        };
        round_trip(&ack);
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
                    shared: false,
                    collapsed: false,
                    output_collapsed: false,
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

    // ── per-cell collapse flags in protocol messages ────────────────────

    #[test]
    fn mutation_cell_update_with_collapse_flags() {
        let msg = Message {
            id: "req-collapse".into(),
            kind: MessageKind::Mutation(Mutation::CellUpdate {
                cell_id: "cell-1".into(),
                source: None,
                cargo_toml: None,
                label: None,
                shared: None,
                collapsed: Some(true),
                output_collapsed: Some(false),
                version: 3,
            }),
        };
        round_trip(&msg);

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"collapsed\":true"));
        assert!(json.contains("\"output_collapsed\":false"));
    }

    #[test]
    fn mutation_cell_update_without_collapse_flags_omits_them() {
        // Old peers that never send the flags round-trip unchanged, and
        // unset flags stay out of the wire format.
        let msg = Message {
            id: "req-plain".into(),
            kind: MessageKind::Mutation(Mutation::CellUpdate {
                cell_id: "cell-1".into(),
                source: Some("42".into()),
                cargo_toml: None,
                label: None,
                shared: None,
                collapsed: None,
                output_collapsed: None,
                version: 1,
            }),
        };
        round_trip(&msg);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("collapsed"));
    }

    // ── reactive_mode in protocol messages ──────────────────────────────

    #[test]
    fn mutation_notebook_update_meta_with_reactive_mode() {
        let msg = Message {
            id: "req-reactive".into(),
            kind: MessageKind::Mutation(Mutation::NotebookUpdateMeta {
                meta: NotebookMetaPatch {
                    reactive_mode: Some(true),
                    ..Default::default()
                },
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
                meta: NotebookMetaPatch {
                    title: Some("Title".into()),
                    ..Default::default()
                },
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
                    meta: NotebookMetaPatch {
                        reactive_mode: Some(false),
                        ..Default::default()
                    },
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
        // Bumping it is a deliberate, reviewed act, so the current value is
        // locked here and an accidental change is caught.
        //
        // 2: `NotebookUpdateMeta` / `NotebookMetaUpdated` gained the
        // presentation fields (description, tags, og_image + dimensions) when
        // they became editable (PRD-0051).
        // 3: `Mutation::CellRun` + `MutationResult::CellRunStarted`, and
        // `CellExecuted` gained `success` (PRD-0052).
        assert_eq!(PROTOCOL_VERSION, 5);
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

        // Unknown MutationResult nested in a MutationOk → MutationResult::Unknown.
        // This one stalls a CORRELATED request if it fails to parse (the
        // requester's oneshot never resolves), so it must decode.
        let wire = r#"{"id":"r","type":"Response","payload":{"response":"MutationOk","detail":{"result":"CellTeleported","cell_id":"c1"}}}"#;
        let msg: Message = serde_json::from_str(wire)
            .expect("unknown mutation result must decode to MutationResult::Unknown");
        assert!(matches!(
            msg.kind,
            MessageKind::Response(Response::MutationOk {
                detail: MutationResult::Unknown
            })
        ));
    }

    #[test]
    fn event_notebook_meta_updated_omits_none_reactive_mode() {
        let msg = Message {
            id: "evt-no-reactive".into(),
            kind: MessageKind::Event(EventEnvelope {
                by: ClientId::agent("bot"),
                event: Event::NotebookMetaUpdated {
                    meta: NotebookMetaPatch {
                        title: Some("New".into()),
                        ..Default::default()
                    },
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
