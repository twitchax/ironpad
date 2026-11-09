# agent-cli Specification

## Purpose
The `ironpad-cli` binary is how an AI agent drives a collaboration session from a shell. A long-lived daemon holds the session's WebSocket and a cached copy of the notebook, and short-lived subcommands talk to it over a Unix socket, printing JSON and exiting with codes an agent can branch on.

## Requirements
### Requirement: Global connection flags
`ironpad-cli` SHALL take `--host` (env `IRONPAD_HOST`, a WebSocket origin such as `ws://localhost:3111`) and `--token` (env `IRONPAD_TOKEN`) as global flags. Only `daemon` needs them; without one it SHALL print `error: --host or IRONPAD_HOST required for daemon mode` or `error: --token or IRONPAD_TOKEN required for daemon mode` and exit 1. (f305339 agentic-collaboration/0009)

#### Scenario: The command the session panel teaches
- **WHEN** an agent runs `ironpad-cli --host ws://localhost:3111 --token <token> daemon` with a live session's token
- **THEN** the daemon connects to `ws://localhost:3111/ws/connect?token=<token>`

#### Scenario: Missing token
- **WHEN** `ironpad-cli --host ws://localhost:3111 daemon` runs with `IRONPAD_TOKEN` unset
- **THEN** it prints `error: --token or IRONPAD_TOKEN required for daemon mode` and exits 1

### Requirement: Daemon startup
`ironpad-cli daemon` SHALL run in the foreground; no other subcommand starts it. It SHALL create `~/.ironpad` with mode 0700, bind `daemon.sock` with mode 0600, and then write its pid to `daemon.pid`, all before connecting, then cache the notebook, waiting up to 10 seconds for it. The socket is per-user, so it SHALL refuse to start, printing `another ironpad daemon is serving <path> (run daemon-stop first)` and exiting 1, when a daemon is already listening on `daemon.sock`, and SHALL replace a leftover socket file that nothing is listening on. Its logs SHALL show the token as `<redacted>`. (PRD-0035, PRD-0038, f305339 agentic-collaboration/0008)

#### Scenario: Ready to serve
- **WHEN** the daemon connects to a live session
- **THEN** `ironpad-cli status` prints `{"connected":true,"cached":true}` once the notebook arrives

#### Scenario: Server unreachable
- **WHEN** the daemon cannot open its WebSocket
- **THEN** it keeps serving its socket, `status` reports `"connected":false`, and commands that need the browser fail with `connection_error`

#### Scenario: A daemon is already running
- **WHEN** `ironpad-cli daemon` starts while another daemon is serving `~/.ironpad/daemon.sock`
- **THEN** it exits 1 without touching that socket or its pidfile, and the running daemon keeps serving its session

### Requirement: Daemon lifetime follows the session
The daemon SHALL exit when its WebSocket closes, whether the session ended, the host left, or the server dropped the connection, and SHALL NOT reconnect; a new session needs a new daemon. It SHALL send a WebSocket Ping every 30 seconds, so the relay's guest idle timeout never reaps a daemon whose agent is only reading or waiting on a run, and SHALL treat 90 seconds without any inbound frame (the relay answers each Ping with a Pong) as a closed WebSocket. On that exit, and on SIGINT or SIGTERM, it SHALL remove its socket and pidfile, but only while `daemon.pid` still holds its own pid; files another daemon has since claimed SHALL be left in place. (PRD-0035, PRD-0038)

#### Scenario: Host tab closes
- **WHEN** the browser tab hosting the session closes
- **THEN** within a few seconds `ironpad-cli status` exits non-zero

#### Scenario: Stopped by signal
- **WHEN** the daemon receives SIGTERM
- **THEN** `~/.ironpad/daemon.sock` and `~/.ironpad/daemon.pid` no longer exist

#### Scenario: An agent that only reads
- **WHEN** an agent issues no mutation for longer than the relay's guest idle timeout while the session stays live
- **THEN** the daemon is still connected and `ironpad-cli status` reports `"connected":true`

#### Scenario: Silent network drop
- **WHEN** the link to the relay stops delivering frames without closing
- **THEN** the daemon exits within 90 seconds instead of serving a stale cache

### Requirement: Stopping the daemon
`ironpad-cli daemon-stop` SHALL send SIGTERM to the pid in `~/.ironpad/daemon.pid` and print `sent stop signal to daemon (pid N)`. It SHALL exit 1 with `daemon is not running (no pidfile)` when no pidfile exists, and, where `/proc` exists, SHALL refuse to signal a pid whose command line lacks `ironpad`, deleting that stale pidfile. (PRD-0036, PRD-0038)

#### Scenario: Recycled pid
- **WHEN** the pidfile names a pid now owned by an unrelated process
- **THEN** no signal is sent, the pidfile is removed, and the command prints that the pidfile is stale and exits 1

### Requirement: Daemon status
`ironpad-cli status` SHALL print `{"connected":<bool>,"cached":<bool>}`: whether the daemon's WebSocket is up and whether it holds a copy of the notebook. With no daemon running it SHALL fail with `connection_error`. (f305339 agentic-collaboration/0009)

#### Scenario: No daemon
- **WHEN** `ironpad-cli status` runs with no daemon listening
- **THEN** stderr reads `{"error":"connection_error","message":"daemon is not running (cannot connect to socket)"}` and the exit code is 4

### Requirement: Output and exit codes
A subcommand that reaches the daemon SHALL print its result as one line of compact JSON on stdout and a failure as `{"error":<code>,"message":<text>}` on stderr. It SHALL exit 0 on success, 2 for `VersionConflict`, 3 for `PermissionDenied`, 4 for `connection_error`, and 1 for any other failure, including `timeout` and `CellNotFound`. (f305339 agentic-collaboration/0009)

#### Scenario: Stale version
- **WHEN** an agent updates a cell with a `--version` older than the cell's
- **THEN** stderr carries `"error":"VersionConflict"` and the process exits 2

#### Scenario: Read-only token writes
- **WHEN** an agent on a session without write permission runs `cells add`
- **THEN** stderr carries `"error":"PermissionDenied"` and the process exits 3

### Requirement: Error code strings
The `error` field SHALL be `connection_error` when the daemon, its socket, or its WebSocket is unreachable, `timeout` when a run wait expires, a protocol code verbatim (`VersionConflict`, `CellNotFound`, `PermissionDenied`, `SessionNotFound`, `InvalidMessage`, ...) when the browser or relay refused, and `error` for an uncoded failure. (f305339 agentic-collaboration/0009)

#### Scenario: No cached notebook
- **WHEN** `cells list` runs before the daemon has cached the notebook
- **THEN** stderr reads `{"error":"error","message":"no notebook cached"}` and the exit code is 1

### Requirement: Reads come from the daemon's cache
`cells list`, `cells get <id>`, and `notebook` (alias `notebook get`) SHALL answer from the daemon's cached notebook, which follows every event the host relays, with no round trip to the browser. `cells list` SHALL print, per cell in order, `id`, `order`, `label`, `cell_type`, `shared`, `collapsed`, `output_collapsed`, `version`, and the first 80 characters of its source as `source_preview`. (f305339 agentic-collaboration/0008 0009)

#### Scenario: Agent reads a one-cell notebook
- **WHEN** the host notebook holds one code cell and the agent runs `cells list`
- **THEN** it prints an array of one entry whose `cell_type` is `"Code"`
- **AND** `cells get <that id>` prints the full cell, including its `source` and `version`

#### Scenario: Unknown cell
- **WHEN** an agent runs `cells get no-such-cell`
- **THEN** stderr reads `{"error":"CellNotFound","message":"cell not found"}` and the exit code is 1

### Requirement: Add a cell
`cells add` SHALL create a cell from `--source <text>` (`-` reads stdin) or `--source-file <path>` (empty if neither), `--type code|markdown|linux` (default `code`), `--label` (default `New Cell`), `--after <cell-id>` (omitted inserts at the top), `--cargo-toml`, and `--shared`, and SHALL print the browser's acknowledging event. (PRD-0044, PRD-0066, f305339 agentic-collaboration/0009)

#### Scenario: Agent adds a cell
- **WHEN** an agent runs `cells add --source "let x = 42;" --label "Agent Cell"` against an empty notebook
- **THEN** it exits 0 and the host shows one cell
- **AND** stdout is the event envelope `{"by":"agent:remote","event":{"event":"CellAdded","cell":{...},"after_cell_id":null}}`

### Requirement: Update a cell
`cells update <id>` SHALL patch any of `--source`/`--source-file` (`-` reads stdin), `--cargo-toml`, `--label`, `--shared true|false`, `--collapsed true|false`, and `--output-collapsed true|false`, expecting `--version` or, if omitted, the version in the daemon's cache. With no field flag it SHALL exit 1 before contacting the daemon. (PRD-0044, f305339 agentic-collaboration/0009)

#### Scenario: Agent rewrites a cell's source
- **WHEN** an agent runs `cells update <id> --source "let x = 99;"`
- **THEN** it exits 0 and a following `cells get <id>` shows a source containing `let x = 99;`

#### Scenario: Nothing to update
- **WHEN** an agent runs `cells update <id>` with no field flag
- **THEN** it prints `error: nothing to update — pass at least one of --source, --source-file, --cargo-toml, --label, --shared, --collapsed, --output-collapsed` and exits 1

### Requirement: Delete and reorder cells
`cells delete <id>` SHALL delete the cell, expecting `--version` or the cached version. `cells reorder <id>...` SHALL order the notebook by the given ids; ids it does not know are ignored and cells it omits follow the listed ones in their existing order. Both print the acknowledging event. (f305339 agentic-collaboration/0009)

#### Scenario: Agent deletes the only cell
- **WHEN** an agent runs `cells delete <id>` on a one-cell notebook
- **THEN** it exits 0 and the host shows no cells

### Requirement: Update notebook fields
`notebook update` SHALL send one patch built from `--title`, `--shared-source`/`--shared-source-file`, `--shared-cargo-toml`/`--shared-cargo-toml-file`, `--description`, and comma-separated `--tags`; `-` reads stdin, for at most one field. Each clearable field has a `--clear-*` flag, an omitted flag leaves its field alone, and no flag at all SHALL exit 1 with "nothing to update". (PRD-0051)

#### Scenario: Set then clear shared source
- **WHEN** an agent runs `notebook update --shared-source "pub fn from_cli() -> i32 { 9 }" --shared-cargo-toml "[dependencies]..."`
- **THEN** `notebook` soon shows both fields and the host stores them
- **AND** `notebook update --clear-shared-source` makes `shared_source` absent, not an empty string

#### Scenario: Tags parsing
- **WHEN** an agent passes `--tags "blog, simd ,"`
- **THEN** the notebook's tags become `["blog","simd"]`
- **AND** `--tags " , "` clears the tags, like `--clear-tags`

### Requirement: Forwarded commands fail fast
Commands that need the browser (`cells add`, `update`, `delete`, `reorder`, `run`, and `notebook update`) SHALL fail with `connection_error` when the daemon's WebSocket is not connected or has closed, or when no answer arrives within 10 seconds (`request timed out`). A refusal from the browser or relay SHALL surface with its own code. (PRD-0035)

#### Scenario: Session already ended
- **WHEN** the host has left the notebook and an agent runs `cells add --label probe --source 42`
- **THEN** the command exits non-zero instead of hanging

### Requirement: Run a cell and wait for its outcome
`cells run <id>` SHALL ask the hosting browser to run the cell and wait up to `--timeout-secs` (default 360) for its outcome, printing `{"status":"executed"` or `"execution_error","cell_id","display_text","type_tag","execution_time_ms"}`, or `{"status":"compile_error","cell_id","diagnostics"}`. An output over 256 KiB arrives as a `display_text` placeholder saying it is too large to send to agents; the full output stays in the browser. `--no-wait` SHALL print the `CellRunStarted` acknowledgement and return. (PRD-0052)

#### Scenario: Agent runs a cell
- **WHEN** an agent adds `CellOutput::text(format!("{}", 42))` and runs `cells run <id>`
- **THEN** stdout has `"status":"executed"`, the cell's id, a `display_text` containing `42`, and an `execution_time_ms` of at least 0

#### Scenario: Wait expires
- **WHEN** no outcome arrives within the timeout
- **THEN** stderr reads `{"error":"timeout","message":"timed out after <N>s waiting for the run to finish"}` and the exit code is 1

### Requirement: Run outcomes for dropped and deleted runs
`cells run` SHALL also end with `{"status":"prerequisite_failed","cell_id":<failed cell>,"blocked_cell_id":<target>}` when the browser reports the run blocked by a failed dependency, `{"status":"cancelled","cell_id"}` when the user cancels the queued run, and `{"status":"cell_deleted","cell_id"}` when the target is deleted mid-run. (PRD-0052, PRD-0060)

#### Scenario: Blocked by a dependency
- **WHEN** the browser reports `CellBlocked { cell_id: "mine", blocked_by: "dep" }` while `cells run mine` waits
- **THEN** the command prints `prerequisite_failed` with `cell_id` `dep` and `blocked_cell_id` `mine`

#### Scenario: Cancelled by the user
- **WHEN** the user stops the run and the browser reports `RunCancelled` naming the target
- **THEN** the command prints `{"status":"cancelled","cell_id":<target>}`

### Requirement: Unrelated failures do not end a run wait
While `cells run` waits, the failure of a cell the target does not transitively depend on SHALL NOT end the wait. If a dependency fails and the browser sends no drop report, the wait SHALL end as `prerequisite_failed` only after 1.5 seconds pass with neither a report nor activity from the target. (PRD-0060)

#### Scenario: Independent failure
- **WHEN** a cell the target does not read fails while the target is queued
- **THEN** the wait stays open and, absent a target result, ends in `timeout`

#### Scenario: Dependency failure from an older browser
- **WHEN** a cell the target reads through `cell0` fails and no `CellBlocked` follows
- **THEN** the command prints `prerequisite_failed` naming that cell once the grace window passes

### Requirement: Refused runs fail at once
A `cells run` the browser refuses (unknown, non-runnable, or Linux cell, or a host in View mode) SHALL fail immediately with the browser's message and code `CellNotFound`, exit 1, rather than waiting for an outcome. (PRD-0052, PRD-0066)

#### Scenario: Unknown cell
- **WHEN** an agent runs `cells run no-such-cell`
- **THEN** the command exits non-zero without waiting out its timeout

### Requirement: Daemon socket protocol
The daemon's socket SHALL speak newline-delimited JSON: a request `{"command":..., "args":...}` (`args` optional) gets `{"ok":true,"data":...}` or `{"ok":false,"error":...,"code":...}` with absent fields omitted. Commands are `status`, `notebook.get`, `notebook.update`, and `cells.list|get|add|update|delete|reorder|run`; a frame over 16 MiB (four times the 4 MiB notebook upload cap) SHALL drop the connection. Neither side SHALL send an over-cap frame: the daemon replaces an over-cap response with an uncoded error naming its size, and the CLI refuses an over-cap request before writing it, so both exit 1 rather than reporting `connection_error`. (PRD-0038, f305339 agentic-collaboration/0008)

#### Scenario: Status over the socket
- **WHEN** a client writes `{"command":"status"}` and a newline to `~/.ironpad/daemon.sock`
- **THEN** it reads back one line `{"ok":true,"data":{"connected":true,"cached":true}}`

#### Scenario: Oversized frame
- **WHEN** a client streams more than 16 MiB without a newline
- **THEN** the daemon drops that connection instead of buffering it

#### Scenario: Large run output
- **WHEN** `cells run` executes a cell that renders an 800x400 Canvas
- **THEN** the command exits 0 with status `executed`
