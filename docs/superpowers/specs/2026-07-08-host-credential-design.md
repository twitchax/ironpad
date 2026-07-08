# Design — Per-notebook host credential (PRD-0038 T-014)

**Date:** 2026-07-08
**Status:** approved (design)
**Task:** PRD-0038 T-014 — "Host credential to claim a notebook_id (close the unauthenticated host role)"

## Context

The collaboration relay lets a browser register as the authoritative *host* for a
notebook via `GET /ws/host?notebook_id=<id>`. Today `handle_host` →
`WsState::register_host` does a last-writer-wins `hosts.insert(notebook_id, …)`
with **no authentication**. Anyone who learns a `notebook_id` (URL, referrer,
logs) can:

- connect as host and **evict** the real browser (the insert replaces the incumbent),
- receive every guest mutation/query broadcast for that notebook,
- mint sessions and tokens (`ControlMessage::CreateSession`).

Only the secrecy of the v4-UUID `notebook_id` protects the model — there is no
defense in depth. This was flagged HIGH in the 2026-07-07 review and deferred
from PRD-0036 T-008.

Relevant facts that shape the design:

- The server is **stateless for private notebooks** — they live in the hosting
  browser's IndexedDB; the `hosts` registry is in-memory (`Arc<RwLock<HashMap>>`).
- A private notebook exists in exactly **one** browser (IndexedDB is per-origin,
  per-device), so "same notebook on two devices" is not a real case.
- Two tabs of the same browser share `localStorage`, so "take over from another
  tab" must keep working.

## Goal

Bind the host claim to a per-notebook secret so only a browser holding that
secret can become — or replace — the host. Trust-on-first-use (TOFU): the first
claimant of a `notebook_id` (within the server's lifetime) sets the secret;
later claims must match.

## Approach (chosen)

**In-memory, TOFU, forget-when-idle** secret model + a **first-message
handshake** to carry the secret. (Alternatives considered and rejected:
disk-persisted secrets — adds server-side per-notebook state contra the stateless
design; stateless signed tokens — more moving parts than the threat warrants.
Query-param transport rejected — the secret is a long-lived credential and would
leak into access/proxy logs, the same issue the review raised for the guest
token.)

### 1. Protocol (`ironpad-common/src/protocol.rs`)

Add one variant to `ControlMessage`:

```rust
/// Browser → Server: first frame on the host socket. Proves the claimant holds
/// the per-notebook host secret before the server registers it as host.
ClaimHost { secret: String },
```

Rejection is **not** a new message — the server closes the WebSocket with
application close code **4403** (reason `"host claim rejected"`). A malformed or
missing first frame closes with **4400** (`"expected ClaimHost"`). `ControlMessage`
already carries `#[serde(other)] Unknown` (T-016), so adding a variant is clean.

### 2. Server (`ironpad-server`)

**State (`state.rs`).** Add a map beside the host registry so a secret binding
outlives any single host connection:

```rust
// notebook_id → blake3(secret). Bound on first claim (TOFU), required to match
// on later claims, dropped when the notebook is idle (no host + no sessions).
notebook_secrets: Arc<RwLock<HashMap<String, [u8; 32]>>>,
```

New `WsState` methods:

- `claim_host(notebook_id, secret) -> ClaimOutcome` — under the lock: hash the
  secret; if unbound, bind + return `Accepted { tofu: true }`; if bound and equal
  (constant-time compare), `Accepted { tofu: false }`; else `Rejected`.
- `forget_secret_if_idle(notebook_id)` — drop the binding iff there is no host
  connection AND `sessions_for_notebook(notebook_id)` is empty.

**Handshake (`ws.rs handle_host`).** After the upgrade, do **not** register.
Read the first frame with the existing `MAX_WS_MESSAGE_BYTES` cap and the host
idle timeout:

1. Parse it as `Message`; require `MessageKind::Control(ControlMessage::ClaimHost { secret })`.
   Anything else → close **4400**.
2. `claim_host(notebook_id, &secret)`:
   - `Rejected` → close **4403**; do **not** touch the incumbent host or its sessions.
   - `Accepted` → `register_host(...)` as today (this replaces a stale connection
     for the *same* notebook — the legit reconnect/takeover) and enter the normal
     host read loop.

On host disconnect (the existing `unregister_host` path) and on session-end,
call `forget_secret_if_idle(notebook_id)`.

### 3. Browser (`ironpad-app/src/session/connection.rs` + storage)

- **Secret storage** — a tiny helper reads/writes `localStorage` key
  `ironpad:host-secret:{notebook_id}`. On host-start: read it; if absent,
  generate 32 bytes via `crypto.getRandomValues`, hex-encode, store. Kept in
  `localStorage`, **separate from the IndexedDB notebook record**, so
  export/share of the notebook never carries the secret.
- **Handshake** — in the host `onopen`, send
  `ControlMessage::ClaimHost { secret }` as the **first** frame, before any other
  traffic.
- **Rejection** — the `onclose` handler inspects the code: **4403** → surface a
  clear, non-technical error ("This notebook's host credential doesn't match — it
  may be open in another session.") and do not auto-reconnect into a loop.

### 4. Rollout / backward compatibility

- Existing notebooks have no stored secret → one is generated on the first
  host-start after the update and TOFU-bound server-side. No migration.
- A pre-update browser (old WASM) sends no `ClaimHost` first frame → the server
  closes it (4400) until the user reloads and gets the new WASM. This is a
  page-reload window, accepted for a single-author app. We **require** `ClaimHost`
  (cleanly closing the hole) rather than keeping a legacy last-writer-wins
  fallback (which would leave the hole open).

## Data flow

```
Browser                              Relay (WsState)
  |  GET /ws/host?notebook_id=N        |
  |----------------------------------->|  upgrade; NOT yet host
  |  Control::ClaimHost{ secret=S }    |
  |----------------------------------->|  claim_host(N, S):
  |                                    |    unbound → bind blake3(S)      → Accepted (TOFU)
  |                                    |    bound & match (const-time)    → Accepted (replace stale)
  |                                    |    bound & mismatch              → Rejected
  |   (Accepted) register_host; normal host loop …
  |   (Rejected) <--- close 4403 ------|  incumbent + sessions untouched
  |
  |  (host disconnects) --------------->  unregister_host; forget_secret_if_idle(N)
```

## Error handling

- Missing/malformed first frame → close 4400; never registered.
- Mismatched secret → close 4403; incumbent host keeps hosting and receiving guest
  traffic; the attacker learns only "rejected".
- Secret compare is constant-time over the two 32-byte blake3 hashes (a timing
  leak of a hash is useless without a preimage, but constant-time is cheap and
  correct).
- Oversized first frame is bounded by the existing `MAX_WS_MESSAGE_BYTES`; a host
  that never sends the first frame is reaped by the existing host idle timeout.

## Testing

Server (`ironpad-server/tests/relay.rs` + `state.rs`/`ws.rs` units):

1. **TOFU accept** — first claim of a fresh notebook_id is accepted; host receives guest traffic.
2. **Matching reconnect** — a second connection with the *same* secret is accepted and replaces the stale host connection.
3. **Mismatch rejected** — a claim with a different secret is closed 4403; the incumbent host stays registered and still receives a subsequent guest mutation.
4. **Forget-when-idle** — after the host disconnects with no sessions, the secret is dropped, and a fresh claim with a *new* secret is TOFU-accepted.
5. **Bad first frame** — a non-`ClaimHost` first frame is closed 4400; never registered.
6. **Protocol round-trip** — `ClaimHost` serializes/deserializes (protocol.rs unit test).

Browser: the wasm host path is not unit-testable; covered by the existing
`tests/e2e/session.spec.ts` flow (host starts a session, a CLI guest connects and
edits). Add an assertion that a session still establishes end-to-end after the
handshake. A negative (wrong-secret) browser test is out of scope for e2e.

## Non-goals

- Cross-restart persistence of the secret (in-memory only; after a restart there
  is no active host/session to hijack, and TOFU re-establishes on next claim).
- Protecting an idle notebook with no host and no sessions (nothing to hijack —
  that is exactly when the secret is forgotten).
- Multi-device / multi-browser hosting of the same private notebook (impossible by
  construction — the notebook lives in one browser's IndexedDB).
- Read-only shared/public notebook viewers (they do not claim a host role).

## Affected files

- `crates/ironpad-common/src/protocol.rs` — `ClaimHost` variant + round-trip test.
- `crates/ironpad-server/src/state.rs` — `notebook_secrets`, `claim_host`, `forget_secret_if_idle`.
- `crates/ironpad-server/src/ws.rs` — handshake in `handle_host`; call forget-if-idle on disconnect.
- `crates/ironpad-server/src/sessions.rs` — hook session-end → forget-if-idle (if not already reachable from ws.rs).
- `crates/ironpad-server/tests/relay.rs` — the integration tests above.
- `crates/ironpad-app/src/session/connection.rs` — secret load/generate, first-frame handshake, 4403 handling.
- (browser storage helper — colocated in `connection.rs` or `session/mod.rs`.)
