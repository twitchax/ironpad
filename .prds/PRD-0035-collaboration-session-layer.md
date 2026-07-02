---
id: PRD-0035
title: "Collaboration & session layer fixes (C1-C7)"
status: draft
owner: "Aaron Roney"
created: 2026-07-02
updated: 2026-07-02

principles:
- "No edit is lost between the browser and connected agents"
- "A rejected mutation surfaces immediately — never silently times out"
- "Connection identity is per-socket, not derived from a shared token"

references:
- name: "Review report — sections C1-C7 (collaboration/session)"
  url: reviews/2026-07-02-codebase-review.md

acceptance_tests:
- id: uat-001
  name: "Edits made before a session's socket opens are delivered to the agent (not dropped)"
  command: cargo make playwright
  uat_status: unverified
- id: uat-002
  name: "A rejected mutation (version conflict) returns an error to the CLI within ~1s, not a 10s timeout"
  command: cargo make test
  uat_status: unverified
- id: uat-003
  name: "An agent's source/cargo edit updates the visible host editor"
  command: cargo make playwright
  uat_status: unverified

tasks:
- id: T-001
  title: "C1: Don't send (or drain) buffered events into a CONNECTING socket"
  priority: 1
  status: todo
  notes: "session/connection.rs:119-137 + ws_send:21-32: Leptos Effect::new runs its body immediately on creation inside start_session, BEFORE on_open, so it drain_events() (model.rs:175-181 mem::take) and ws_send into a CONNECTING socket -> InvalidStateError (the 4 failed sends seen live), events permanently lost. Fix: only send when ws.ready_state()==OPEN; if not open, don't drain (leave buffered) — or construct the event-bridge Effect inside on_open."
- id: T-002
  title: "C2: Route mutation error responses so OCC conflicts don't hang the agent 10s"
  priority: 1
  status: todo
  notes: "ws.rs:357-369 + :140-144: guest mutations aren't tracked (only queries, :373); success acks arrive via the broadcast Event (connection.rs:199-213 -> daemon.rs:247-255), but a Response::Error for a mutation id (connection.rs:214-226) routes only via resolve_query (ws.rs:141) -> None -> error discarded -> daemon oneshot never resolves -> forward_to_server times out (daemon.rs:531-543) instead of surfacing VersionConflict/CellNotFound. Fix: track mutations too (clear on the broadcast Event) or add a mutation-error route independent of the query map."
- id: T-003
  title: "C3: Compare-and-remove on host unregister (fix reconnect/two-tab race)"
  priority: 1
  status: todo
  notes: "ws.rs:104 + state.rs:88-94: unregister_host removes purely by notebook_id; on reconnect/two tabs, register_host replaces the handle (state.rs:72-84) then the OLD connection's cleanup removes the NEW host -> live host absent from hosts map -> guests get SessionNotFound. Fix: only remove if the stored connection_id matches the caller's."
- id: T-004
  title: "C4: Notify host UI on agent-originated content edits"
  priority: 1
  status: todo
  notes: "model.rs:336-364: content-only cell_update uses notebook.update_untracked and only sync_from_notebook when label.is_some(); fine for local Monaco edits, but agent mutations flow through the same model.apply (connection.rs:199), so an agent's source/cargo edit lands with NO reactive notification -> host editor doesn't refresh. Fix: for agent-originated content edits, call sync_from_notebook / a tracked bump so subscribers update."
- id: T-005
  title: "C5: Make client_id unique per connection, not the token prefix"
  priority: 2
  status: todo
  notes: "ws.rs:240-241 + state.rs:127-169: client_id = ClientId::agent(token[..8]); two connections on one token collide -> send_to_guest delivers to the first match only, and unregister_guest's retain removes BOTH handles when either disconnects, stranding the survivor. Fix: append a per-socket UUID to client_id."
- id: T-006
  title: "C6: Application-level heartbeat + idle timeout to detect half-open connections"
  priority: 2
  status: todo
  notes: "ws.rs:84-94,298-308: no ping/pong or read timeout; a network drop without FIN leaves the host registered indefinitely, send_to_host returns true into a dead channel (agent thinks it delivered, then times out — compounds C2). Fix: periodic ping/pong with an idle timeout that tears down and runs normal cleanup."
- id: T-007
  title: "C7: Session-store housekeeping and lifecycle cleanups"
  priority: 3
  status: todo
  notes: "Bundle the minor lifecycle items: schedule sweep_expired at startup (sessions.rs:170 — never called); surface an overflow/resync signal instead of silently dropping events past MAX_EVENT_BUFFER=64 (model.rs:113-118); expire pending_queries on timeout and drop a disconnecting guest's pending queries (state.rs:181-191); add a daemon WS reconnect loop or document intentional death-on-disconnect (daemon.rs:158-214); redact the token in the daemon's logged ws_url (daemon.rs:115-116)."
- id: T-008
  title: "Prune dead protocol surface and the non-functional execute permission"
  priority: 3
  status: todo
  notes: "model.rs:100-105: CellCompile/CellExecute pass the permission check but model.apply rejects them with InvalidMessage — either wire compile/execute through or drop the permission/variants. model.rs:152-155: Query::SessionStatus always errors and nothing produces Response::SessionStatus, so daemon.rs:669-675 is unreachable — remove the dead surface."
---

# Summary

The browser is the authoritative model and the server is a dumb relay; several bugs break that contract. The worst: edits buffered before a session's socket opens are drained and sent into a CONNECTING socket, so they're permanently lost (this is the `InvalidStateError` seen live). Rejected mutations have no route home, so every version conflict hangs the agent for 10s. A host reconnect can unregister the live host. And agent content edits never refresh the host's editor. This epic fixes the collaboration desyncs and connection-lifecycle gaps.

# Problem

The relay assumes reliable, ordered, acked delivery it doesn't actually provide: the pre-open send loses events, mutation errors fall through the query-only response map, host identity isn't checked on unregister, `client_id` collides on shared tokens, and there's no heartbeat to detect dead connections. Individually minor-looking, together they make agent collaboration unreliable.

# Goals

1. No edit is lost across session start / reconnect.
2. Mutation rejections surface to the CLI promptly.
3. Agent edits are reflected in the host UI.
4. Connection identity and lifecycle are correct (per-socket ids, heartbeat, cleanup).

# Technical Approach

Client-side fixes in `session/connection.rs` and `model.rs`; server-side in `ws.rs`, `state.rs`, `sessions.rs`; CLI in `daemon.rs`. T-001-T-004 are the correctness-critical desyncs; T-005-T-008 harden identity, lifecycle, and prune dead surface. See task notes for exact `file:line`.

# Assumptions

- The existing broadcast-Event ack path for successful mutations stays; C2 adds the error path alongside it.
- Bounded-channel/backpressure and host-auth hardening are handled in PRD-0036 (server hardening), not here — this epic is correctness of the existing design.

# Constraints

- Protocol changes must stay backward-compatible with the current CLI daemon or ship with a matching daemon change in the same batch.
- No blocking work added to the relay read loops.

# References to Code

- `crates/ironpad-app/src/session/connection.rs`, `crates/ironpad-app/src/model.rs`
- `crates/ironpad-server/src/ws.rs`, `state.rs`, `sessions.rs`
- `crates/ironpad-cli/src/daemon.rs`
- `crates/ironpad-common/src/protocol.rs`

# Non-Goals (MVP)

- Bounded channels / message-size caps / host authentication (PRD-0036).
- Reordering guarantees across different cells (finding 17 — harmless, deferred).

# History

(Entries appended during implementation go below this line.)
