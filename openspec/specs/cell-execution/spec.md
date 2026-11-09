# cell-execution Specification

## Purpose
Running Code cells in the browser: how a cell's compiled WASM is found (share snapshot, local blob cache, server compile), executed in a Web Worker with crash recovery, a main-thread fallback and cancellation, and sequenced by Run, Run All, the dependency-aware cascade, staleness and reactive mode. It serves notebook authors in the editor and readers on read-only pages.

## Requirements
### Requirement: Cells execute in a Web Worker
The system SHALL load each Code cell's compiled WASM into a dedicated Web Worker and execute it there with the cell's assembled piped inputs, so a long-running cell never blocks the page's UI thread, and SHALL forward the host messages a cell sends from the worker to the page. (PRD-0001, PRD-0013)

#### Scenario: A cell returns a value
- **WHEN** an author adds a Code cell with source `CellOutput::text(format!("{}", 42))` and clicks the button titled "Run cell"
- **THEN** the cell's status badge reaches success
- **AND** its output shows `42` with no "main thread" badge

#### Scenario: A text-bearing plot stays in the worker
- **WHEN** a cell renders `Plot::scatter(&data).title("Squares").x_label("x").y_label("x squared")`
- **THEN** the SVG chart renders from the worker, which answers the text measurement plotters would otherwise ask the DOM for
- **AND** the output carries no "main thread" badge

### Requirement: Worker confinement
The system SHALL confine the executor worker and each rayon sub-worker before any cell runs in it: every `fetch` a cell makes, directly or through `ironpad_cell::http` or `blocking::fetch_*`, SHALL leave without credentials, and `indexedDB`, `caches` and `XMLHttpRequest` SHALL be absent. This is a mitigation, not a security boundary: a worker a cell creates itself is not confined. (review bug-db-auth-1)

#### Scenario: A stranger's cell makes a request as nobody
- **WHEN** a signed-in reader runs, on `/shared`, cells that request the reader page of a private notebook the reader owns and POST `list_mutable_shares`, through `ironpad_cell::http`, `blocking::fetch_text` and an eval'd `fetch`
- **THEN** each request reaches the server without the session cookie and its response holds neither the notebook's title nor its id
- **AND** the same requests made from the page's main thread return both

#### Scenario: No browser storage in the worker
- **WHEN** a cell evaluates `typeof indexedDB`, `typeof caches` and `typeof XMLHttpRequest`
- **THEN** each is `undefined`

### Requirement: Main-thread fallback
When the worker fails to load, execute, or tick a cell for any reason other than a user cancel, on a page that grants the fallback, the system SHALL retry that call once on a main-thread executor with the cell's most recent blob and SHALL flag a result produced there, so its output wears a "main thread" badge. With no blob to retry, the worker's error is reported. When the retry fails too, the system SHALL report the worker's error if it carried a panic message the retry's lacks, or if the call was a tick of a worker that ran it (the fallback instance never ran the cell, so its tick fails on missing state); otherwise it SHALL report the retry's error. A new blob that fails to load in the worker SHALL leave the cell unloaded there rather than keep the previous version, so the next run takes the fallback with the new blob. Only the editor and first-party `/public` notebooks grant the fallback; `/shared`, the `/mutable` reader and every `/embed` page SHALL NOT retry on the main thread, and SHALL report the worker's error followed by a note that the cell was not retried there and that forking the notebook runs it. The grant in force when a call is made decides that call's fallback, and the executor grants nothing until a page does. (PRD-0017, PRD-0033, review bug-db-auth-1)

#### Scenario: A stranger's cell is not retried on the main thread
- **WHEN** a reader runs, on `/shared`, a cell that panics only inside the worker
- **THEN** the cell shows its error with the advice to fork the notebook to run it
- **AND** it produces no output and no "main thread" badge

#### Scenario: The author's cell is retried on the main thread
- **WHEN** the same cell runs in the editor
- **THEN** it is retried on the main thread, its output appears and wears the "main thread" badge

#### Scenario: Ticks run on the fallback once the worker is gone
- **WHEN** the worker has stopped respawning and a page that grants the fallback loads a Simulation blob and a LiveView blob and ticks each
- **THEN** both ticks return results flagged `fallback: true`, a 1x1 frame of `[10, 20, 30]` and the text `hi`
- **AND** neither blob is reported as loaded in the worker

#### Scenario: A reload that fails only in the worker
- **WHEN** a cell has run version 1 in the worker and version 2's glue fails to initialize in a worker but not on the page
- **THEN** the next run returns version 2's output flagged `fallback: true`, never version 1's

#### Scenario: A retry that only saw a trap
- **WHEN** the worker reports `panicked at x: boom` for a cell and its main-thread retry fails with a bare trap
- **THEN** the call fails with the worker's `boom` message

#### Scenario: Nothing to fall back with
- **WHEN** a tick is requested for a cell that was never loaded while the worker is unavailable
- **THEN** the call fails with "Worker unavailable (respawn limit reached)"

### Requirement: Worker crash recovery
When the execution worker raises an uncaught error, the system SHALL reject every in-flight executor request with "Worker crashed: <message>" so no cell hangs, terminate that worker, and spawn a fresh one. A WASM trap in an async cell's body, which runs outside the call that started the cell, SHALL instead fail that run with the trap's diagnosis, in the worker and on the main thread, and SHALL NOT reach the worker's error handler. After 3 consecutive crashes, each within 5 s of its worker's spawn, it SHALL stop respawning and leave cells to the main-thread fallback, where the page grants it, until the next cancel. (PRD-0033)

#### Scenario: An async cell's trap is not a crash
- **WHEN** a cell containing `.await` panics inside its async body
- **THEN** the cell shows error with its panic message and the worker is not replaced
- **AND** the next cell runs normally

#### Scenario: Crash, cap, and recovery
- **WHEN** the worker crashes once
- **THEN** a new worker replaces it
- **AND** after two more crashes in quick succession no worker remains and a request fails with "Worker unavailable (respawn limit reached)"
- **AND** a cancel then spawns a worker again and resets the crash count

### Requirement: Cancelling a cell
While an editor cell compiles or runs, its Run button SHALL become a Stop button titled "Cancel execution". Stopping a run SHALL terminate and respawn the worker, return the cell to idle and empty the run queue, and SHALL NOT re-run the cell on the main thread; a run already retrying on the main thread SHALL be abandoned with an `AbortError`. Stopping a compile SHALL return the cell to idle at once and discard the compile result when it arrives. (PRD-0013, PRD-0032)

#### Scenario: Stop during execution
- **WHEN** an author clicks "Cancel execution" on a cell whose WASM is running
- **THEN** the cell returns to idle and every cell queued behind it is dropped
- **AND** every other cell's loaded module is discarded, so its next run loads its blob again

#### Scenario: A cancel is never retried on the main thread
- **WHEN** a pending tick is rejected because the worker was terminated
- **THEN** the caller receives an `AbortError`
- **AND** nothing runs on the main-thread executor and no fallback warning is logged

#### Scenario: Stop during compile
- **WHEN** an author clicks "Cancel execution" while the cell is still compiling
- **THEN** the cell shows idle immediately and the worker is left untouched
- **AND** the server's compile response, when it arrives, is discarded without executing

### Requirement: Execution error messages
The system SHALL report a failed blob load as "WASM load error: ..." and a failed run as "Execution error: ...". A Rust panic SHALL be reported by its message, in the worker and on the main-thread fallback alike, cut before the `Stack:` trace the panic hook appends. A WASM trap with no captured panic message SHALL be translated into a diagnosis that names the cell's linear-memory size in MB. (PRD-0001, PRD-0033)

#### Scenario: A panic names itself
- **WHEN** a cell runs `panic!("kaboom-123")`
- **THEN** the cell shows error and its output contains `kaboom-123`
- **AND** the output does not say the cell ran out of memory

#### Scenario: A bare trap
- **WHEN** a cell's WASM traps with `unreachable` and no panic message was captured
- **THEN** the error reads "Execution trapped (linear memory: N MB) — the cell aborted (a panic or a failed allocation). The browser console may have more detail."

#### Scenario: Out-of-bounds access
- **WHEN** a cell's WASM traps on an out-of-bounds memory access
- **THEN** the error reads "Out-of-bounds memory access (linear memory: N MB) — the cell tried to read or write outside allocated memory."

#### Scenario: Stack overflow
- **WHEN** a cell recurses until the call stack is exhausted
- **THEN** the error reads "Stack overflow — the cell likely has unbounded or very deep recursion."

### Requirement: Compile transport retries
The system SHALL retry a compile request that fails in transport up to 2 more times, waiting 1.5 s and then 3 s, before reporting the failure. A compile that returns diagnostics is a result, and SHALL NOT be retried. (PRD-0059)

#### Scenario: A dropped request recovers
- **WHEN** the first compile request for a cell fails in transport and the retry succeeds
- **THEN** the cell runs normally and shows no error

#### Scenario: Every attempt fails
- **WHEN** all three attempts fail in transport
- **THEN** the editor shows the error diagnostic "Server error: ..." and a read-only page shows "Compile error: ..."
- **AND** the run queue treats the cell as failed

### Requirement: Local blob cache
The browser SHALL keep successfully compiled blobs in an IndexedDB `blobs` store keyed by the server's own cache key bound to the server's toolchain fingerprint, capped at 64 entries least-recently-used, and SHALL serve a matching entry without a compile request. A deploy that changes the toolchain changes every key, so stale entries are never served. (PRD-0047)

#### Scenario: Rerunning an unchanged cell
- **WHEN** an author runs `CellOutput::text(format!("{}", 41 + 1))` and then runs it again unchanged
- **THEN** the second run issues no `compile_cell` request
- **AND** it shows `42`

#### Scenario: Recency is touched at most once a minute
- **WHEN** an entry last used two minutes ago is read twice in quick succession
- **THEN** the first read refreshes its last-used time
- **AND** the second read serves the blob without rewriting the record

#### Scenario: What is never cached
- **WHEN** a compile returns diagnostics and no blob, or the toolchain fingerprint cannot be fetched
- **THEN** nothing is stored, and with no fingerprint the local cache is skipped for the rest of the session

### Requirement: Share snapshot replay
A read-only page that carries a share manifest (`/shared/{hash}`, the `/mutable/{id}` reader, and their embeds) SHALL run a snapshotted cell from `/share-blobs/{blob}.wasm`, plus `/share-blobs/{blob}.js` when the manifest says the blob has glue, without compiling or probing the local cache. A failed fetch SHALL fall back to the local cache and then the server. (PRD-0047)

#### Scenario: A fresh reader compiles nothing
- **WHEN** a notebook whose cell returned `42` is shared and a new browser context with an empty local cache opens `/shared/{hash}` and clicks "Run All"
- **THEN** the page fetches from `/share-blobs/` and issues no `compile_cell` request
- **AND** it shows `42`

#### Scenario: A missing snapshot degrades
- **WHEN** fetching a cell's snapshot blob fails
- **THEN** the cell is served from the local cache or compiled by the server, and runs

### Requirement: Force Recompile
The editor's gear-menu "Force Recompile" toggle and a read-only page's "Cached" / "Fresh" toggle SHALL make every run bypass the share snapshot, the local blob cache and the server's compile cache until switched back, and the fresh result SHALL replace the local cache entry. The setting lasts for the page and is not saved with the notebook. (PRD-0047)

#### Scenario: Fresh, then cached
- **WHEN** an author turns on "Force Recompile" and runs a cell that already ran
- **THEN** the run sends a `compile_cell` request
- **AND** after turning it off, the next run of that cell is served locally with no `compile_cell` request

### Requirement: Dependency-aware cascade on Run
Running a single cell SHALL first run, in notebook order, the runnable cells it transitively consumes through `cellN` or `last` that have no live output in this page session, and no other cell. A saved output from the author's last run is not a live output. (PRD-0060, PRD-0052)

#### Scenario: An independent cell runs alone
- **WHEN** a notebook holds `CellOutput::text("upstream, never consumed")` then `40 + 2`, and only the second cell is run
- **THEN** the second cell succeeds and the first stays idle throughout

#### Scenario: A consumer cascades exactly its chain
- **WHEN** a notebook holds `7u32`, `CellOutput::text("independent bystander")`, `cell0 * 6`, and only the third cell is run
- **THEN** the first cell runs, then the third, which shows `42`
- **AND** the bystander stays idle

#### Scenario: A saved output does not satisfy a dependency
- **WHEN** a reader runs a cell that uses `cell0` while cell 0 shows only its saved output
- **THEN** cell 0 runs first

### Requirement: Dependency detection
The system SHALL treat a cell as consuming slot N when its source contains `cellN` with no identifier character on either side, and as consuming every upstream runnable cell when its source contains `last` (other than a lone `.last` method call) or `__ironpad_inputs__`, or when its source is unknown. Matches inside comments and strings count. (PRD-0060)

#### Scenario: Word boundaries
- **WHEN** a cell's source is `let x = cell1 + cell2;`, `cell10`, or `mycell1 cell2x sub_cell3`
- **THEN** it consumes slots 1 and 2, slot 10 alone, and nothing, respectively

#### Scenario: The last alias
- **WHEN** a cell's source is `xs.last()`
- **THEN** it consumes nothing
- **AND** a cell whose source is `for i in 0..last.len() {}` consumes every upstream runnable cell

#### Scenario: Transitive closure skips what is not consumed
- **WHEN** cells are `1 + 1`, a markdown cell mentioning `cell0`, `cell0 * 2`, `42`, `cell2 + 1`, and the last one is run cold
- **THEN** exactly the first and third cells run before it

### Requirement: Run All
The editor's "Run All" button, each cell's "Run All Below" menu item, and a read-only page's "Run All" button SHALL queue every runnable cell (Code and not shared) from the starting cell in notebook order and run them one at a time, each waiting cell showing "queued". (PRD-0007, PRD-0060)

#### Scenario: What Run All includes
- **WHEN** Run All is pressed on Code `a`, a Markdown cell, a shared Code cell, a Linux cell, and Code `b`
- **THEN** exactly `a` then `b` run

#### Scenario: A read-only page while running
- **WHEN** a reader presses "Run All"
- **THEN** the button reads "Running…" and is disabled until the queue empties
- **AND** a queued cell's Run button is disabled and labelled "Queued"

#### Scenario: Running a cell while a queue is in flight
- **WHEN** an author runs independent cell `x` while a queue of `a`, `b` is running with `a` in front, in a notebook ordered `a`, `x`, `b`
- **THEN** `x` joins the queue as `a`, `x`, `b`, with `a` still in front

### Requirement: Continue past failures
When a queued cell fails to compile, load, or execute, the system SHALL drop it and every queued cell that transitively depends on it, mark each dropped dependent blocked by the failed cell, and keep running every remaining queued cell. (PRD-0060)

#### Scenario: Only true dependents block
- **WHEN** Run All runs `this does not compile`, `cell0`, and `"independent survivor".to_string()`
- **THEN** the first cell shows error, the third shows success, and the second shows "blocked"

#### Scenario: A read-only page names the failure
- **WHEN** a cell on a read-only page is skipped because a cell labelled "Setup" failed
- **THEN** it shows "Skipped: this cell uses the output of "Setup", which failed. Fix that cell or press Run to retry."

### Requirement: Clearing the blocked state
A blocked cell SHALL leave the blocked state when it is run directly or when the cell blocking it next runs successfully, and turning Reactive Mode off SHALL clear every blocked state in the editor. (PRD-0060, PRD-0025)

#### Scenario: A direct run is a fresh attempt
- **WHEN** a cell blocked by a failed `cell0` dependency is edited to `2 + 2` and run
- **THEN** it reaches success and stays there, with no blocked status

### Requirement: Rerun invalidates downstream outputs
Starting a cell's run in the editor SHALL discard the live outputs of that cell and of every later Code cell, and SHALL clear the output panel of each later idle cell, so no downstream cell shows or consumes a result computed from the previous upstream value. (PRD-0032)

#### Scenario: Earlier and non-code cells are untouched
- **WHEN** the second cell of `before`, `me`, a Markdown cell, `after` is rerun
- **THEN** the outputs of `me` and `after` are discarded and `after`'s output panel empties
- **AND** `before` keeps its output

### Requirement: Staleness
Changing a cell's source SHALL mark that cell and every later runnable cell stale, by position rather than by dependency, shown as an indicator titled "Cell output is stale", until each next runs successfully. Markdown, shared, and Linux cells never go stale. (PRD-0025)

#### Scenario: Positional, not dependency-aware
- **WHEN** an author edits the second of three Code cells that have all run, none of which references another
- **THEN** the second and third cells show the stale indicator and the first does not
- **AND** running the third cell successfully clears only its own indicator

### Requirement: Reactive mode
The editor's gear-menu "Reactive Mode" toggle SHALL persist as the notebook's `reactive_mode` flag and, while on, SHALL queue every stale runnable cell in notebook order 500 ms after the most recent change that staled a cell, unless a run is already queued; stale cells then show "Pending re-execution (reactive mode)". (PRD-0025)

#### Scenario: Edits coalesce
- **WHEN** two edits stale cells 300 ms apart with Reactive Mode on
- **THEN** a single run of the stale cells starts 500 ms after the second

#### Scenario: The setting survives a reload
- **WHEN** an author turns Reactive Mode on and reloads the notebook
- **THEN** the gear menu shows Reactive Mode checked

### Requirement: Threaded cells
A cell built with rayon SHALL start a thread pool of `navigator.hardwareConcurrency` threads (4 when unknown) inside the execution worker, sharing the cell's memory; that needs `SharedArrayBuffer`, which the cross-origin isolation headers in `server-operations` provide. Each rayon cell SHALL own its pool, which is released when that cell is unloaded or its blob is replaced, so loading one rayon cell never stops another's threads. A panic on a pool thread SHALL fail the running cell with the panic's message and replace the execution worker, whose loaded cells reload on their next run, and SHALL NOT be retried on the main thread. The pool thread reports on a channel named per page, so a panic in one tab never touches another tab's worker. (PRD-0024)

#### Scenario: A parallel sum runs in the worker
- **WHEN** a cell with `rayon = "1"` in its Cargo.toml computes `(1..=1000u64).into_par_iter().sum()`, calls `sim::emit("e2e-rayon-sum", &total)`, and returns `CellOutput::text(format!("sum={total}"))`
- **THEN** the output shows `sum=500500` with no "main thread" badge
- **AND** `window.IronpadExecutor.simBusRead("e2e-rayon-sum")` on the page returns `500500`

#### Scenario: Two rayon cells in one notebook
- **WHEN** `/public/mandelbrot` has autorun both of its rayon cells and a reader presses "Run All" five seconds later
- **THEN** every Code cell runs again and the button is enabled again within a minute

#### Scenario: A panic inside a parallel closure
- **WHEN** a rayon cell computes `(0..100).into_par_iter().map(|i| { assert!(i != 50, "bad row"); i }).sum::<i32>()`
- **THEN** the cell shows error with `bad row` in its output instead of staying on Running

#### Scenario: A pool thread that never starts
- **WHEN** a pool thread does not report ready within 30 s
- **THEN** every pool thread already spawned is terminated, so no partial pool is left running
- **AND** the worker's load of that cell fails and takes the main-thread fallback like any other worker failure

### Requirement: Cells of an unknown type never run
A cell whose `cell_type` this build does not recognize SHALL never compile or run: a read-only page renders its source with an "unsupported" badge and the notice "This cell was written by a newer version of ironpad and cannot run here.", the editor offers no Run button, and saving keeps the original type tag. (PRD-0066)

#### Scenario: A cell from a newer release
- **WHEN** a notebook containing a cell with `"cell_type": "Quantum"` is opened
- **THEN** the rest of the notebook opens and runs normally
- **AND** that cell shows its source and the notice, and Run All skips it

### Requirement: Deleting a cell during a run
Deleting a cell, from the editor or by an agent, SHALL remove it from the run queue, drop its live output, and clear every blocked state it holds or causes, so cells queued behind it keep running; a compile, check, or run still in flight for it SHALL finish without error. (1130be9)

#### Scenario: Deleting mid-check
- **WHEN** an author deletes a cell while its live check is still round-tripping
- **THEN** no "already been disposed" error is logged and the page keeps working

#### Scenario: Deleting the queued front
- **WHEN** the cell at the front of a running queue is deleted
- **THEN** the cells queued behind it still run
