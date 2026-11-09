# live-check Specification

## Purpose
While an author types in the notebook editor, ironpad type-checks the cell on the server without generating code and paints the resulting errors and warnings inline, for ordinary code cells and for shared cells alike, without ever making typing wait on a build. The editor also offers completions and hover documentation for the cell API and for the cell's upstream inputs.

## Requirements
### Requirement: Check request and response contract
The system SHALL expose a `check_cell` server function that takes the same `CompileRequest` as `compile_cell` and returns a `CheckResponse` whose `status` is `Clean`, `Errors`, `Skipped` or `TimedOut` and whose `diagnostics` are already mapped to the author's lines exactly as compile diagnostics are. (PRD-0045 T-001)

#### Scenario: Type error
- **WHEN** a cell whose source is `let x: i32 = "not a number";\nx` is checked
- **THEN** the status is `Errors` and a diagnostic with code `E0308` spans line 1

#### Scenario: Clean code
- **WHEN** a cell whose source is `let x: i32 = 42;\nx` is checked
- **THEN** the status is `Clean` and the diagnostics list is empty

#### Scenario: Skipped and timed-out rounds carry nothing
- **WHEN** a check returns `Skipped` or `TimedOut`
- **THEN** its diagnostics list is empty

### Requirement: A check is the build minus codegen
The system SHALL check a cell with the same request validation, generated crate, target, pinned toolchain, feature flags and compile proxy its build would use, running `cargo check --target <triple> --release --message-format=json`, so code that checks clean builds clean; a check MUST NOT read or write the compile cache. (PRD-0045 T-001)

#### Scenario: Invalid request
- **WHEN** a check request carries `cell_id` `../x` or `cell_type` `Markdown`
- **THEN** `check_cell` fails with the same error `compile_cell` gives

#### Scenario: Feature modes apply
- **WHEN** a cell using `std::simd` is checked
- **THEN** the check runs with the same `+simd128` flag and `portable_simd` gate as its build, and its diagnostics map to the same lines

#### Scenario: Linux cell
- **WHEN** a `Linux` cell is checked
- **THEN** it is checked for `wasm32-browserpod-linux-musl` on the BrowserPod toolchain, as it would be built

### Requirement: Check time budget
The system SHALL bound each live check by `IRONPAD_LIVE_CHECK_TIMEOUT_SECS` (default 10 seconds) and, on expiry, kill the whole cargo process group and return `TimedOut` rather than an error. Waiting for another build or check to release the shared cargo target directory SHALL NOT count against that budget: the check waits up to the budget for the directory first, and returns `Skipped` if it does not get it. (PRD-0045 T-001)

#### Scenario: Cold dependency tree
- **WHEN** a check would need longer than the budget to build its dependencies
- **THEN** the response is `TimedOut` with no diagnostics
- **AND** no cargo or rustc process from that check keeps running

### Requirement: Checks never wait on a build of the same cell
The system SHALL return `Skipped` immediately, without running cargo, when a compile or another check of the same cell id (or one differing only by `-` versus `_`) is in flight. (PRD-0045 T-001)

#### Scenario: Check during a compile
- **WHEN** a cell is compiling and its source is edited
- **THEN** the check for that cell returns `Skipped`

### Requirement: Checks shed load instead of queueing
The system SHALL run live checks in their own pool of `--max-concurrent-builds` slots (default 3), separate from compiles, and SHALL return `Skipped` when every check slot is busy. A check of a cell whose workspace already exists MUST NOT be charged against the per-client build rate limit; the first check of a cell id with no workspace SHALL cost one build token, and SHALL return `Skipped` without scaffolding anything when the client's bucket is empty. (PRD-0052)

#### Scenario: Pool exhausted
- **WHEN** every check slot is held and another check arrives
- **THEN** it returns `Skipped` with no diagnostics instead of waiting

#### Scenario: Builds do not starve checks
- **WHEN** every compile slot is busy
- **THEN** a live check can still take a check slot and run

#### Scenario: A stream of fresh cell ids
- **WHEN** a client whose build bucket is empty sends a check for a cell id that has never been scaffolded
- **THEN** it returns `Skipped` and no workspace directory is created
- **AND** a re-check of a cell it already scaffolded still runs

### Requirement: Checks dispatch after the save debounce
The editor SHALL dispatch a live check for a cell one second after its source stops changing, at the moment the edit is saved, and as soon as an agent's source edit reaches the host's editor, and MUST NOT dispatch one for a Markdown or unrecognized cell, a cell that is compiling or running, or an edit to the cell's Cargo.toml alone. View-only and embedded pages SHALL NOT run live checks. (PRD-0045 T-003)

#### Scenario: Squiggle without running
- **WHEN** an author types `let x: i32 = "not a number";\nx` into a new cell and stops typing
- **THEN** an error squiggle appears in the editor
- **AND** the cell shows no output and no success status

#### Scenario: An agent's edit is checked
- **WHEN** an agent sets a code cell's source through a session
- **THEN** the host dispatches a live check of the agent's text

#### Scenario: Fixing clears the squiggle
- **WHEN** the author then changes the source to `let x: i32 = 42;\nx`
- **THEN** the squiggle disappears after the next check round

### Requirement: Live checks wait for a warm dependency tree
The editor SHALL live-check a cell immediately only when neither the notebook's shared manifest nor the cell's manifest declares a dependency beyond `ironpad-cell` (inline or `[dependencies.NAME]` form); otherwise it SHALL wait until that exact pair of manifests has compiled without errors in the current page session. (PRD-0045 T-002)

#### Scenario: Custom dependency not yet compiled
- **WHEN** a cell's manifest declares `rand = "0.8"` and the cell has not compiled cleanly since the page loaded
- **THEN** typing in it dispatches no check

#### Scenario: After a clean compile
- **WHEN** that cell then compiles without errors
- **THEN** subsequent edits to it are live-checked

#### Scenario: Dotted form counts as a dependency
- **WHEN** the shared manifest declares only `[dependencies.serde]`
- **THEN** cells in the notebook wait for a clean compile before being live-checked

### Requirement: Checks skip cells whose inputs are missing
The editor SHALL NOT live-check a cell that references a slot `cellN` whose upstream cell has not produced a typed output, or that references `last` while no upstream cell has, since the generated crate binds only typed slots and the check cannot run its predecessors. A reference to a slot past the cells above, or to the raw input buffer, SHALL still be checked: the first is a real error and the second is always bound. (PRD-0045 T-003, PRD-0060)

#### Scenario: Fresh notebook referencing an earlier cell
- **WHEN** a cell's source reads `cell0 + 1` and no cell above it has run
- **THEN** no check is dispatched and no false "cannot find value" marker appears

#### Scenario: Only some cells above have run
- **WHEN** only the first of three cells has run and the third's source reads `cell1 + 1`
- **THEN** no check is dispatched for the third cell and no false "cannot find value" marker appears
- **AND** once the second cell has run too, the third is checked

### Requirement: Freshest result owns the markers
The editor SHALL apply a check's result only if it answers the cell's newest dispatch, SHALL replace the cell's inline markers with a `Clean` or `Errors` result (a `Clean` result clears them), and SHALL let compile and check results share the markers with the most recent one winning. A failed request MUST leave existing markers untouched. (PRD-0045 T-003)

#### Scenario: Superseded response
- **WHEN** an author edits again before an earlier check returns
- **THEN** the earlier check's result is discarded

#### Scenario: Marker severities
- **WHEN** a result carries an error, a warning and a note with spans
- **THEN** they render as error, warning and info markers, each showing its span label when present and otherwise the diagnostic message

### Requirement: Busy and cold rounds degrade quietly
The editor SHALL retry a `Skipped` check up to 5 times, 3 seconds apart, re-reading the cell's current source and eligibility on each retry, and SHALL NOT retry a `TimedOut` check until the author's next edit. (PRD-0045 T-001 T-002, PRD-0052)

#### Scenario: Server busy
- **WHEN** a check returns `Skipped` and a slot frees 3 seconds later
- **THEN** the retry runs and its markers appear without further typing

#### Scenario: Cold cache
- **WHEN** a check returns `TimedOut`
- **THEN** the markers are left as they were until the next edit dispatches a new check

### Requirement: Shared cells are live-checked against the assembled library
The editor SHALL live-check a shared cell by sending a Code request whose body is `String::new()`, whose cell manifest is `[dependencies]`, whose shared source is the notebook's full assembled shared source, and whose `shared_check` names the 0-based line where this cell starts in that assembly and its line count. (PRD-0046 T-002 T-003)

#### Scenario: Error typed into a shared cell
- **WHEN** an author marks a cell shared and changes it to `pub fn double(x: &str) -> u32 {\n    x * 2\n}`
- **THEN** an error squiggle appears in that shared cell at the offending line
- **AND** nothing runs, and the cell shows no output or status

#### Scenario: Fixed shared cell
- **WHEN** the author changes it to `pub fn double(x: u32) -> u32 {\n    x * 2\n}`
- **THEN** the squiggle clears after the next check round

### Requirement: Shared-cell diagnostics anchor only inside the cell
The system SHALL return, for a check with `shared_check`, only diagnostics whose primary span starts inside that cell's slice of the shared source, renumbered to cell-local lines with ends clamped to the cell, and SHALL drop diagnostics from other shared cells, the notebook-level shared source, the throwaway body, and span-less messages. (PRD-0046 T-001 T-002)

#### Scenario: Error inside the slice
- **WHEN** the error is on line 3 of the assembled shared source and the cell starts at 0-based line 2 with 3 lines
- **THEN** the diagnostic is reported on cell-local line 1

#### Scenario: Error in an earlier shared cell
- **WHEN** the same error is checked for a cell starting at 0-based line 4
- **THEN** the diagnostic is dropped

#### Scenario: Assembly broken elsewhere
- **WHEN** the shared source fails to check only because of another shared cell
- **THEN** the status is `Errors` with an empty list, and this cell's markers clear

### Requirement: Completions for the cell API
Every Rust editor SHALL offer completions, triggered by typing or by `.` and `:`, drawn from a generated index of the `ironpad-cell` public API served at `/monaco/completions-index.json` (each entry with its signature and first doc paragraph) plus Rust keywords, snippets, common macros and `CellOutput` helpers. (PRD-0045 T-005)

#### Scenario: Runtime API entry
- **WHEN** an author opens completions in a cell
- **THEN** the list includes `blocking::sleep_ms` with detail `pub fn sleep_ms(ms: f64)` and its one-line doc ("Block for ms milliseconds")

#### Scenario: Index unavailable
- **WHEN** the index fails to load
- **THEN** keywords, snippets, macros and helpers are still offered

### Requirement: Completions for upstream inputs
The editor SHALL offer, ranked first in a code cell's completions, a `cellN` variable for each Code cell above it, where N is its position among all preceding cells, labeled with its output type tag or `unknown`, plus `last` typed as the nearest typed Code cell above when any Code cell precedes. (PRD-0045 T-005)

#### Scenario: Markdown cell keeps its slot
- **WHEN** a notebook opens with a Markdown cell followed by two Code cells and the author edits the third cell
- **THEN** the offered variables are `cell1`, `last` and no `cell0`

#### Scenario: Type shown after a run
- **WHEN** the cell above has run and produced a `u32`
- **THEN** its `cellN` completion shows the type `u32`

### Requirement: Hover documentation for the cell API
Every Rust editor SHALL show, when the pointer rests on a word naming an entry in the API index, that entry's signature as a Rust code block and its first doc paragraph, preferring the entry qualified by the `Type::` or `module::` prefix written before the word. (PRD-0045 T-005)

#### Scenario: Qualified hover
- **WHEN** the pointer rests on `sleep_ms` in `blocking::sleep_ms(300.0)`
- **THEN** the hover shows `pub fn sleep_ms(ms: f64)` as Rust code followed by its doc line

#### Scenario: Unknown word
- **WHEN** the pointer rests on a word that is not in the index
- **THEN** no hover is shown
