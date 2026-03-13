# 0009: CLI Client

## Summary

Implement the user-facing CLI subcommands that agents use to interact with notebooks. Thin client that talks to the daemon over IPC.

## Motivation

This is what the LLM agent actually calls. Every command should be fast, predictable, and produce machine-readable output by default.

## Design

### Command structure

```
ironpad-cli [global options] <subcommand> [args]

Global options:
  --host <URL>          Ironpad server URL (or IRONPAD_HOST env)
  --token <TOKEN>       Session token (or IRONPAD_TOKEN env)
  --format <FORMAT>     Output format: json (default), pretty
  --quiet               Suppress non-essential output
```

### Subcommands

```
ironpad-cli connect
  Connect to a session (starts daemon if needed).
  Prints session info: notebook title, cell count, connected clients.

ironpad-cli status
  Show daemon status: connected/disconnected, session info, uptime.

ironpad-cli cells list
  List all cells in order.
  Output: [{ id, order, label, cell_type, source_preview, version }]

ironpad-cli cells get <cell_id>
  Get full cell content.
  Output: { id, order, label, cell_type, source, cargo_toml, version }

ironpad-cli cells add [options]
  Add a new cell.
  Options:
    --source <SOURCE>       Cell source code (or read from stdin with --)
    --source-file <PATH>    Read source from file
    --type <code|markdown>  Cell type (default: code)
    --label <LABEL>         Cell label
    --after <CELL_ID>       Insert after this cell (default: end)
    --cargo-toml <TOML>     Custom Cargo.toml content
  Output: { id, version }

ironpad-cli cells update <cell_id> [options]
  Update a cell's source.
  Options:
    --source <SOURCE>       New source (or stdin with --)
    --source-file <PATH>    Read source from file
    --cargo-toml <TOML>     Update Cargo.toml
    --version <VERSION>     Expected version for OCC (auto-fetched if omitted)
  Output: { id, version }

  If --version is omitted, the CLI reads the current version from its cache
  and uses that. This is the common case — explicit version is for advanced usage.

ironpad-cli cells delete <cell_id>
  Delete a cell.
  Output: { deleted: true }

ironpad-cli cells reorder <cell_id> [<cell_id> ...]
  Set cell order. All cell IDs must be provided.
  Output: { reordered: true }

ironpad-cli notebook
  Get notebook metadata.
  Output: { id, title, cell_count, shared_cargo_toml, shared_source }

ironpad-cli compile <cell_id>
  Trigger compilation of a cell (requires execute permission).
  Waits for result.
  Output: { success, diagnostics: [...], cached }

ironpad-cli run [<cell_id>]
  Compile and execute a cell (or all cells if no ID given).
  Requires execute permission.
  Waits for result.
  Output: { success, display_text, type_tag, execution_time_ms }

ironpad-cli watch
  Stream events as they happen (compilation results, cell updates, etc.).
  Outputs one JSON object per line (JSONL).
  Useful for agents that want to observe what the user is doing.

ironpad-cli daemon start
  Explicitly start the daemon (normally auto-started).

ironpad-cli daemon stop
  Stop the daemon.

ironpad-cli daemon status
  Show daemon process info.
```

### stdin support

For `cells add` and `cells update`, reading from stdin allows piping:

```bash
echo 'println!("hello");' | ironpad-cli cells add --source -
cat my_code.rs | ironpad-cli cells update abc123 --source -
```

### Output format

**JSON (default):** Machine-readable, one JSON object per response. This is what LLM agents consume.

**Pretty:** Human-readable table/formatted output. For manual debugging.

```bash
$ ironpad-cli cells list
[{"id":"abc","order":0,"label":"Cell 1","cell_type":"Code","source_preview":"fn main()...","version":3}]

$ ironpad-cli --format pretty cells list
#  ID       Label   Type      Version  Preview
0  abc123   Cell 1  Code      3        fn main() { println!("hel...
1  def456   Cell 2  Markdown  1        # Introduction...
```

### Error output

Errors are JSON on stderr:

```json
{"error": "version_conflict", "message": "Expected version 3, actual 5", "actual_version": 5}
```

Exit codes:
- 0: success
- 1: general error
- 2: version conflict (agent should retry)
- 3: permission denied
- 4: connection error (daemon not running, session ended)

### Version auto-fetch

For `cells update`, if `--version` is not provided, the CLI:
1. Reads the cell's current version from the daemon's local cache
2. Uses that as the expected version
3. If a conflict occurs, returns the error (agent retries with fresh state)

This makes the common case simple: `ironpad-cli cells update abc --source "new code"`

## Changes

- **`crates/ironpad-cli/src/commands/`** (new):
  - `mod.rs` — subcommand enum
  - `connect.rs` — connect command
  - `status.rs` — status command
  - `cells.rs` — cells subcommands (list, get, add, update, delete, reorder)
  - `notebook.rs` — notebook metadata
  - `compile.rs` — compile command
  - `run.rs` — run command
  - `watch.rs` — event streaming
  - `daemon.rs` — daemon management
- **`crates/ironpad-cli/src/output.rs`** (new): JSON/pretty formatting
- **`crates/ironpad-cli/src/main.rs`**: Top-level arg parsing, daemon auto-start, dispatch

## Dependencies

- **0001** (protocol types for messages)
- **0008** (daemon — CLI connects to it)

## Acceptance Criteria

- All subcommands work and produce correct JSON output
- `cells update` auto-fetches version when not specified
- stdin support works for source input
- Exit codes are correct and documented
- `--format pretty` produces readable table output
- `watch` streams events as JSONL
- Commands fail gracefully when daemon is not running or session is ended
