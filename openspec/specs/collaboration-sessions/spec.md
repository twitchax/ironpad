# collaboration-sessions Specification

## Purpose
Lets the author of a notebook open in the browser editor hand a short-lived token to an AI agent, which then reads, edits, and runs cells over a WebSocket relay. The browser stays the authoritative notebook model and the server only authenticates, routes, and enforces permissions. This covers the session UI, tokens and permissions, the two WebSocket endpoints, the message protocol and its version, and how agent edits and agent-triggered runs behave in the host browser.

## Requirements
### Requirement: Start an agent session from the editor
The notebook editor SHALL offer an icon-only session button in its toolbar while in Edit mode. Clicking it SHALL open a WebSocket to `/ws/host`, create a session with read and write permission, and open the session panel holding the new token. The browser offers no way to choose other permissions. (25022ad agentic-collaboration/0006 0007)

#### Scenario: A session starts from a fresh notebook
- **WHEN** the user creates a local notebook and clicks the toolbar button whose tooltip reads "Start Agent Session"
- **THEN** a panel titled "Agent Session" opens with a masked token
- **AND** clicking "Show" reveals exactly 64 lowercase hexadecimal characters

#### Scenario: No session control in View mode
- **WHEN** the editor is switched to View mode
- **THEN** the session button is not rendered

### Requirement: Session panel contents
The session panel SHALL show the token masked as asterisks (one per character, at most 16) with "Show"/"Hide" and "Copy" controls, a "CLI command" line of the form `ironpad-cli --host <ws-origin> --token <token> daemon` with its own "Copy", a "Connected agents" list (or "No agents connected"), and an "End Session" button. An outside click SHALL close the panel without ending the session. (25022ad agentic-collaboration/0007)

#### Scenario: CLI command matches the page origin
- **WHEN** the editor is served from `https://ironpad.twitchax.com` and the token is `deadbeef`
- **THEN** the panel's CLI command reads `ironpad-cli --host wss://ironpad.twitchax.com --token deadbeef daemon`
- **AND** a page served over plain `http://localhost:3111` yields `--host ws://localhost:3111`

#### Scenario: Dismissing is not ending
- **WHEN** the panel is open and the user clicks "Show" inside it
- **THEN** the panel stays open
- **AND** a click elsewhere in the editor closes the panel while the session button keeps its active state and reopens the same panel

### Requirement: Session status on the toolbar button
The session button SHALL carry its state in its tooltip and accessible name ("Start Agent Session", "Agent session active; no agents connected yet", "Agent session: N agent(s) connected", "Agent session: connecting…", "Agent session: disconnected") and a corner badge (a dot with no guests, else the guest count), and its width SHALL NOT change when guests connect. (25022ad agentic-collaboration/0007)

#### Scenario: One agent connects
- **WHEN** a CLI daemon connects with the session's token
- **THEN** the badge reads "1" and the tooltip matches "1 agent connected"
- **AND** the button's rendered width equals its width before the session started

#### Scenario: Connection lost without ending
- **WHEN** the host socket closes unexpectedly
- **THEN** the button stays active with the tooltip "Agent session: disconnected" rather than silently reverting to "Start Agent Session"

### Requirement: Session ends with its page
A session SHALL end, and its connected agents SHALL receive `SessionEnded` and be disconnected, when the user clicks "End Session", when the host navigates away from the notebook page, or when the host tab closes. The host page SHALL stay fully interactive after navigating away with a session running. (PRD-0035, 098f995 agentic-collaboration/0004)

#### Scenario: End Session from the panel
- **WHEN** an agent is connected and the user clicks "End Session"
- **THEN** the session button returns to its inactive state
- **AND** the agent's daemon observes the close and its next `status` exits non-zero

#### Scenario: Navigating away
- **WHEN** the host clicks the brand link back to `/` with an agent connected
- **THEN** a later `cells add` from the agent fails
- **AND** navigating back to the notebook still allows editing its title, with no "already been disposed" console errors

### Requirement: Session tokens
The server SHALL mint each session a fresh 64-character hex token from 32 random bytes, return it once to the host, and keep only its blake3 hash. A token SHALL be scoped to one notebook and one host connection, expire 24 hours after creation, and live in memory only, so a server restart invalidates every session. (098f995 agentic-collaboration/0004)

#### Scenario: Plaintext is never stored
- **WHEN** a session is created
- **THEN** the token returned to the host is 64 hex characters and two sessions never share a token
- **AND** validating that plaintext token resolves to the session while any other string does not

### Requirement: Guest endpoint
The server SHALL accept agent (guest) WebSockets at `GET /ws/connect?token=<token>`, validating the token before upgrading: an unknown token SHALL get HTTP 401, an expired session HTTP 410, and a connection beyond the global guest cap HTTP 503. (PRD-0038, d1f812e agentic-collaboration/0005)

#### Scenario: Bad, expired, and over-cap tokens
- **WHEN** a client connects with `token=bogus-token`
- **THEN** the handshake fails with HTTP 401
- **AND** a token whose session has expired fails with HTTP 410, and a valid token arriving when the guest cap (2 in the test, 512 by default) is full fails with HTTP 503

### Requirement: Host endpoint and claim handshake
The server SHALL accept browser hosts at `GET /ws/host?notebook_id=<id>` and require the first frame, within 10 seconds, to be a `ClaimHost` carrying the notebook's host secret. The first claim for an idle notebook binds its secret; a later claim with a different secret SHALL be closed with code 4403 and leave the incumbent host untouched, and any other first frame SHALL be closed with 4400. (PRD-0038)

#### Scenario: Wrong first frame
- **WHEN** a host connects and sends a `CreateSession` before any `ClaimHost`
- **THEN** the socket is closed with code 4400

#### Scenario: Secret mismatch does not evict
- **WHEN** a second host claims a hosted notebook with a different secret
- **THEN** the second socket is closed with code 4403
- **AND** the incumbent host keeps its sessions and can still create new ones

#### Scenario: Browser host secret
- **WHEN** the browser starts a session for a notebook
- **THEN** it sends as its first frame a secret kept in `localStorage` under `ironpad:host-secret:{notebook_id}`, outside the notebook record, so export and sharing never carry it

### Requirement: Host takeover and host cap
A host that presents the bound secret SHALL replace the notebook's current host, whose socket is closed and whose sessions end. The server SHALL refuse a new host with HTTP 503 once 256 notebooks are hosted, except for a notebook that already has a host. A notebook's secret binding SHALL be forgotten once it has neither a host nor a session. (PRD-0035, PRD-0038)

#### Scenario: Same notebook in a second tab
- **WHEN** the same notebook, opened in a second tab of the same browser, starts a session
- **THEN** the second tab becomes the host and the first tab's sessions end with `SessionEnded` to their guests

#### Scenario: Idle notebook can be claimed afresh
- **WHEN** a notebook's host disconnects and it has no remaining sessions
- **THEN** a later claim with any secret is accepted as a first claim

### Requirement: Permission gate
The relay SHALL forward a guest's queries only when its session grants `read` and its mutations, including `CellRun`, only when it grants `write`; a denied frame SHALL never reach the host and SHALL be answered with a `PermissionDenied` error carrying the frame's id. (PRD-0052, 098f995 agentic-collaboration/0004)

#### Scenario: Read-only guest tries to mutate
- **WHEN** a guest on a `{read: true, write: false}` session sends mutation `m-1`
- **THEN** the host receives nothing
- **AND** the guest receives `Response::Error { code: PermissionDenied, message: "Insufficient permissions for this operation" }` with id `m-1`

### Requirement: Read permission gates notebook content
Notebook events SHALL be relayed only to guests whose session grants `read`. A write-only guest SHALL still receive the event that acknowledges its own mutation, but no event it did not originate. `SessionEnded` SHALL reach every guest regardless of permissions. (PRD-0038)

#### Scenario: Write-only agent
- **WHEN** a `{read: false, write: true}` guest sends mutation `m-1` and the host echoes the success event with id `m-1`
- **THEN** that guest receives the event with id `m-1`
- **AND** a later browser-originated event with an empty id is not delivered to it

### Requirement: Relay routing
The relay SHALL route a host `Response` only to the guest that sent the matching message id and drop responses nobody is waiting for; broadcast host events to every readable guest on any session of that notebook; route a mutation's error response back to its sender; and answer a guest frame sent while no host is connected with `SessionNotFound` "host disconnected". (PRD-0035, d1f812e agentic-collaboration/0005)

#### Scenario: Version conflict reaches the agent promptly
- **WHEN** a guest sends mutation `m-1` and the host replies `Response::Error { code: VersionConflict }` with id `m-1`
- **THEN** the guest receives that error immediately rather than waiting out its request timeout

#### Scenario: Host gone
- **WHEN** a guest sends query `q-1` and the notebook has no connected host
- **THEN** the guest receives `Response::Error { code: SessionNotFound, message: "host disconnected" }` with id `q-1`

### Requirement: Guest presence reaches the host
The relay SHALL tell the host `GuestConnected` and `GuestDisconnected` for each guest connection, naming it `agent:<first 8 token characters>-<uuid>`, unique per connection even when two connections share a token. The panel's "Connected agents" list SHALL reflect these names. (PRD-0035, d1f812e agentic-collaboration/0005)

#### Scenario: Two daemons on one token
- **WHEN** two guests connect with the same token
- **THEN** the host sees two distinct client ids and the badge reads "2"
- **AND** either one disconnecting leaves the other connected and receiving its own responses

### Requirement: Connection liveness
The browser host SHALL send a `Heartbeat` every 30 seconds. The server SHALL close a host connection after 120 seconds without any inbound frame, close a guest connection after its configurable idle timeout (30 minutes by default), and every 5 minutes end expired sessions, sending their guests `SessionEnded` before disconnecting them. A guest frame arriving after its session ended or expired SHALL close that guest's connection without being relayed, a guest whose session ends between token validation and registration SHALL be closed rather than registered, and any connection whose outbound socket write does not complete within 120 seconds (a peer that stopped reading) SHALL be closed. (PRD-0035, PRD-0038)

#### Scenario: Silent guest is reaped
- **WHEN** a guest connects and sends nothing for longer than the guest idle timeout (200 ms in the test)
- **THEN** the server closes its connection

#### Scenario: Expired session with an active agent
- **WHEN** a session passes its 24-hour expiry while its agent keeps sending frames
- **THEN** the next sweep sends that agent `SessionEnded` and disconnects it

#### Scenario: Revoked agent that stopped reading
- **WHEN** a guest stops reading until its socket buffers fill and the host then ends its session while staying connected
- **THEN** a mutation the guest sends afterwards never reaches the host
- **AND** with the write timeout at 200 ms, a guest that stops reading is disconnected within seconds

### Requirement: Relay frame and queue limits
The relay SHALL accept frames up to 8 MiB (twice the 4 MiB share cap), so the reply to `NotebookGet` for any shareable notebook fits in one frame, and SHALL bound each connection's outbound queue to 1024 frames, dropping frames for a peer that stops reading rather than buffering without limit. Because `/ws/host` needs no account, the relay SHALL also bound what one connection can make it store: a host `notebook_id` that is empty or longer than 256 bytes SHALL be refused with HTTP 400 before the upgrade; one host connection SHALL hold at most 8 sessions, a further `CreateSession` being answered with an `InvalidMessage` error carrying its id; a guest message id longer than 128 bytes SHALL be refused with an `InvalidMessage` error whose id is empty, before the permission check and without reaching the host; and one guest SHALL have at most 1024 requests awaiting a host reply, a further request being refused with `InvalidMessage` and not forwarded. (PRD-0036, PRD-0038)

#### Scenario: A notebook over 4 MiB crosses the relay
- **WHEN** a host answers a guest's `NotebookGet` with a notebook whose one cell holds 5 MiB of source
- **THEN** the guest receives the whole notebook
- **AND** the host connection survives and can still create sessions

#### Scenario: One anonymous socket cannot grow the session store
- **WHEN** a host connects with a 1 KiB `notebook_id`
- **THEN** the handshake fails with HTTP 400, while a UUID-sized id connects
- **AND** a host that sends nine `CreateSession` frames gets eight sessions and an `InvalidMessage` error for the ninth

### Requirement: Message envelope
Every frame SHALL be a JSON text message `{"id", "type", "payload"}` where `type` is `Mutation`, `Query`, `Event`, `Response`, or `Control`, and the payload is tagged by `action`, `query`, `event` (inside `{by, event}`), `response`, or `control` respectively; mutation acknowledgements are tagged by `result`. (6962173 agentic-collaboration/0001)

#### Scenario: A query on the wire
- **WHEN** a guest lists cells
- **THEN** it sends `{"id":"req-1","type":"Query","payload":{"query":"CellsList"}}`

#### Scenario: An event on the wire
- **WHEN** the browser reports an execution
- **THEN** the frame looks like `{"id":"","type":"Event","payload":{"by":"browser","event":{"event":"CellExecuted","cell_id":"c1",...}}}`

### Requirement: Mutations and queries
The protocol SHALL offer mutations `CellAdd`, `CellUpdate`, `CellDelete`, `CellReorder`, `NotebookUpdateMeta`, and `CellRun`, and queries `NotebookGet`, `CellGet`, and `CellsList`. Patch fields are optional; for clearable fields an explicit JSON `null` SHALL clear the value while an absent key leaves it unchanged. (PRD-0044, PRD-0051, 6962173 agentic-collaboration/0001)

#### Scenario: Clearing versus leaving alone
- **WHEN** a `NotebookUpdateMeta` arrives as `{"action":"NotebookUpdateMeta","description":null}`
- **THEN** the notebook's description is cleared
- **AND** `{"action":"NotebookUpdateMeta","title":"T"}` changes the title and leaves the description as it was

#### Scenario: Cell insertion position
- **WHEN** `CellAdd` names an `after_cell_id` that exists
- **THEN** the new cell lands right after it
- **AND** an `after_cell_id` that does not exist appends the cell, while omitting it inserts at the top

### Requirement: Cell versions and optimistic concurrency
Each cell SHALL carry a version that starts at 0 and rises by one on every accepted `CellUpdate`, even one that changes nothing. `CellUpdate` and `CellDelete` SHALL name the version they expect and SHALL be rejected with `VersionConflict` "Expected version X, actual Y" when it differs. (97fa066 agentic-collaboration/0003)

#### Scenario: Stale write loses
- **WHEN** an agent updates a cell at version 3 while expecting version 2
- **THEN** it receives `VersionConflict` with message "Expected version 2, actual 3" and the cell is unchanged

#### Scenario: Deleting a cell that never existed
- **WHEN** an agent sends `CellDelete` for an unknown cell id with version 0
- **THEN** it receives `CellNotFound` and no `CellDeleted` event is broadcast

### Requirement: Rejected mutations and queries answer with an error
The browser SHALL answer a mutation or query it cannot apply with a `Response::Error` carrying the request's id, an error code, and a message: `VersionConflict`, `CellNotFound`, `NotebookNotFound` when no notebook is loaded, or `InvalidMessage` for an unrecognized mutation or query from a newer peer. The relay adds `PermissionDenied` and `SessionNotFound`. (PRD-0035, PRD-0038)

#### Scenario: Unknown cell in a query
- **WHEN** an agent sends `CellGet` for a cell id that does not exist
- **THEN** it receives `CellNotFound` with message "Cell <id> not found"

#### Scenario: Mutation from a newer peer
- **WHEN** the browser receives `{"action":"CellTeleport","cell_id":"c1"}`
- **THEN** it replies with `InvalidMessage` "unrecognized mutation from a newer peer" and changes nothing

### Requirement: Agent edits behave like host edits
An agent's accepted mutation SHALL go through the same notebook model as the host's own edits: it SHALL be persisted the same way, stale the same cells, and refresh the host's visible Monaco editors (source and Cargo.toml), rendered markdown cells, and cell headers (label and collapse defaults). The acknowledging event SHALL echo the mutation's id and name its origin `agent:remote`. (PRD-0035, 9c27c48 agentic-collaboration/0002, 25022ad agentic-collaboration/0006)

#### Scenario: Agent Cargo.toml edit survives a host save
- **WHEN** an agent updates a cell's Cargo.toml to contain a marker comment
- **THEN** the host's Cargo.toml tab shows the marker
- **AND** after the host presses Ctrl+S and renames the cell, the cell's Cargo.toml in the daemon and in IndexedDB still contains the marker

#### Scenario: Agent markdown edit survives an untouched open
- **WHEN** an agent updates a markdown cell's source
- **THEN** the host's rendered cell shows the new text
- **AND** after the host opens that cell's editor and clicks away without typing, the daemon and IndexedDB still hold the agent's text

#### Scenario: Rapid agent edits both land
- **WHEN** an agent updates one cell's source twice within a second
- **THEN** the cell holds the second source, its version has risen by exactly two, and its Code tab shows no unsaved-changes dot

#### Scenario: Agent renames a cell
- **WHEN** an agent sets a cell's label to `AGENT LABEL`
- **THEN** the host's label input shows `AGENT LABEL`
- **AND** focusing and blurring that input without typing leaves the label and the cell's version unchanged

#### Scenario: Agent sets shared source
- **WHEN** an agent sets the notebook's shared source
- **THEN** the change is stored in IndexedDB and shown in the editor's shared appendix

### Requirement: Host edits stream to agents
Every edit the host makes through the editor SHALL be relayed to readable guests as an event with `by` set to `browser` and an empty id. Events produced while the host socket is still connecting SHALL be held (up to 64) rather than sent into the unopened socket, and flushed once it opens, after the `ClaimHost` and `CreateSession` frames. (PRD-0035, 25022ad agentic-collaboration/0006)

#### Scenario: Host renames a cell
- **WHEN** the host edits a cell's label while an agent is connected
- **THEN** the agent's cached cell shows the new label without the agent sending anything

#### Scenario: Edits before the socket opens
- **WHEN** the host edits the notebook between clicking the session button and the socket opening
- **THEN** no send fails with `InvalidStateError`
- **AND** those events go out right after the claim and session-creation frames

### Requirement: Agent-triggered runs in the browser
The browser SHALL handle `CellRun` as an editor action, not a notebook change: it SHALL queue the cell on the same run queue Run All uses (once, even if requested twice), so unexecuted prerequisites run first as they do for a click on Run, and acknowledge with `MutationOk` `{"result":"CellRunStarted","cell_id":...}`. (PRD-0052)

#### Scenario: Agent runs a cell
- **WHEN** an agent adds a cell `CellOutput::text(format!("{}", 42))` and sends `CellRun` for it
- **THEN** the host cell shows a success status with "42" in its output
- **AND** the agent receives the `CellRunStarted` acknowledgement before any result

### Requirement: Agent run refusals
The browser SHALL refuse a `CellRun` immediately with a `CellNotFound` error whose message names the reason when the cell does not exist, is markdown or shared, is a Linux cell (every run boots a metered pod and nobody is watching), or the host is in View mode (nothing would drain the queue). (PRD-0052, PRD-0066)

#### Scenario: Unknown cell
- **WHEN** an agent requests a run of `no-such-cell`
- **THEN** it receives an error at once instead of a queued run that never finishes

#### Scenario: Host in View mode
- **WHEN** the host has switched to View mode and an agent requests a run
- **THEN** the error says the host notebook is in View mode and to switch it to Edit mode and retry
- **AND** nothing runs when the host later returns to Edit mode

### Requirement: Execution events to agents
While a session is active, the browser SHALL emit `CellCompiling`, `CellCompiled` (diagnostics and `success`), and `CellExecuted` (`display_text`, `type_tag`, `execution_time_ms`, `success`) for every compile and run in the editor, whoever started it; with no active session it SHALL emit none, so no stale backlog flushes at the next session start. (PRD-0052)

#### Scenario: Runtime failure is terminal
- **WHEN** a cell compiles but panics while executing during a session
- **THEN** guests receive `CellExecuted` with `success: false`

#### Scenario: Legacy event without success
- **WHEN** a peer receives a `CellExecuted` frame that has no `success` field
- **THEN** it reads it as `success: true`

### Requirement: Queue drops are always reported
When a queued run is dropped, the browser SHALL report it to agents: `CellBlocked { cell_id, blocked_by }` for each queued cell dropped because a cell it transitively depends on failed, and `RunCancelled { cell_ids }` naming the stopped cell and every queued cell dropped when the user cancels the run. (PRD-0052, PRD-0060)

#### Scenario: Dependency failure
- **WHEN** a queued cell reads `cell0` and cell 0 fails to compile during a session
- **THEN** guests receive `CellBlocked` naming the queued cell and `blocked_by` the failed one

#### Scenario: User stops the run
- **WHEN** the user terminates a running cell while two more cells are queued
- **THEN** the aborted cell reports `CellExecuted` with `success: false`, and guests receive `RunCancelled` naming it and those two cells

#### Scenario: User stops a compile
- **WHEN** the user stops a cell that is still compiling
- **THEN** guests receive `RunCancelled` naming that cell and every cell queued behind it, and no `CellCompiled` or `CellExecuted` for the discarded compile

### Requirement: Protocol version and forward compatibility
The protocol SHALL carry an advisory version, currently 7, that peers never reject on. The envelope SHALL ignore unknown keys, and an unknown `action`, `query`, `event`, `response`, `result`, `control`, or error-code tag SHALL decode to an unknown variant that is dropped or refused, not fail the whole frame; an unknown top-level `type` is dropped. (PRD-0038, PRD-0052, PRD-0060, PRD-0066)

#### Scenario: Extra envelope keys
- **WHEN** a peer receives `{"id":"req-1","version":99,"unknown_future":true,"type":"Query","payload":{"query":"CellsList"}}`
- **THEN** it parses as a `CellsList` query

#### Scenario: Unknown result inside an acknowledgement
- **WHEN** a peer receives `{"response":"MutationOk","detail":{"result":"CellTeleported","cell_id":"c1"}}`
- **THEN** the frame parses with an unknown result instead of failing, so the waiting request resolves rather than timing out
