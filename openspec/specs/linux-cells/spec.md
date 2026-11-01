# linux-cells Specification

## Purpose
Linux cells (`CellType::Linux`) are an explicitly opted-in cell kind whose source is a whole Rust program, compiled server-side to `wasm32-browserpod-linux-musl` and run as a real Linux process under BrowserPod's in-browser kernel, with a filesystem, subprocesses and threads shared by every Linux cell in the notebook. This spec covers how authors create them, how they build and run, what readers see, the limits and failure modes, how boots are metered, and why embeds refuse them.

## Requirements
### Requirement: An explicit cell type
The editor SHALL offer a "Linux" add-cell button, titled "A whole Rust program run as a Linux process in the browser", that inserts a cell persisted with `cell_type: "Linux"` and seeded with a `fn main` that prints "Hello from Linux.". No cell SHALL become a Linux cell because of its content. (PRD-0066)

#### Scenario: Adding a Linux cell
- **WHEN** an author clicks the "Linux" add-cell button in a new notebook
- **THEN** the stored notebook's cells have types `["Linux"]`
- **AND** the cell's source is `fn main() {\n    println!("Hello from Linux.");\n}`

#### Scenario: Content never changes the type
- **WHEN** an author writes `std::fs` and `std::process::Command` calls in a Code cell
- **THEN** it stays a Code cell and compiles for the ordinary cell target

### Requirement: Built as a whole program
The server SHALL compile a Linux cell's source verbatim as the `main.rs` of an edition-2024 bin crate for `wasm32-browserpod-linux-musl` on the `browserpod-3.0.0` toolchain, merging notebook and cell Cargo.toml dependencies as for any cell and appending the notebook's shared source as `mod shared`, with no ironpad-cell dependency, no `cellN` bindings, and no wasm-bindgen or wasm-opt pass. (PRD-0066, PRD-0067)

#### Scenario: Source passes through untouched
- **WHEN** a Linux cell with no shared source is compiled
- **THEN** its `main.rs` equals the cell's source exactly and no `cell_main` wrapper or `ironpad_cell` import appears

#### Scenario: Shared source as a module
- **WHEN** the notebook has shared source
- **THEN** `main.rs` is the cell's source followed by `#[allow(dead_code)] mod shared;`, so the program can call `shared::` helpers and still declare its own crate attributes

#### Scenario: One source, two targets, two blobs
- **WHEN** a Code cell and a Linux cell have identical source
- **THEN** they compile under different cache keys and each is served only its own blob

### Requirement: The editor points to Preview
The editor SHALL render a Linux cell with a "linux" badge and no Run button, and, outside its collapsible body so it stays readable when the cell is collapsed, the notice "This cell runs as a Linux process, which the editor cannot host. Switch to Preview, bottom left, to run it." No other cell type carries that notice. (PRD-0066)

#### Scenario: Only the Linux cell explains itself
- **WHEN** a notebook holds a Code cell and a Linux cell in the editor
- **THEN** exactly one notice is shown, on the Linux cell, and it names "Preview"
- **AND** the Code cell has a "Run cell" button and the Linux cell has none, collapsed or not

### Requirement: Running on a read-only surface
On a read-only page, including the editor's Preview, a Linux cell SHALL offer a Run button titled "Run this program on a Linux machine in your browser" that compiles the cell (honouring the share snapshot, local blob cache and Force Recompile), boots the notebook's machine if needed, and runs the program, showing "compiling", then "starting machine", then "running". (PRD-0066)

#### Scenario: A first run
- **WHEN** a reader clicks Run on a Linux cell of `/public/linux-cells`
- **THEN** the header shows "compiling", then "starting machine", then "running"
- **AND** a "Nms compile" timing badge appears once the compile succeeds

#### Scenario: A compile error
- **WHEN** the program fails to compile
- **THEN** the compiler's messages are shown in the cell's error panel and no machine is started

### Requirement: Real processes, files and threads
A Linux cell's program SHALL run as a Linux process that can use `std::fs`, `std::thread`, `std::process::Command` and `/proc`, with real concurrent threads and genuine fork/exec of the machine's BusyBox tools; `std::thread::available_parallelism()` reports 1 there. (PRD-0066)

#### Scenario: A real process
- **WHEN** the first Linux cell of `/public/linux-cells` runs
- **THEN** its terminal shows a `pid`, an entry for `/proc/self` containing `status`, and a count of executables in `/bin`, and the status reads "exit 0"

#### Scenario: Subprocesses and threads
- **WHEN** the pipeline cell runs after the writer cell
- **THEN** `cut` and `sort` run as child processes that each report `exit status: 0`, and `uniq -c` counts 24 rows each for cairo, oslo and quito
- **AND** the threads cell reports `available_parallelism Ok(1)` and four threads alive at once

### Requirement: One ephemeral machine per notebook
All Linux cells of one notebook SHALL run on one machine, booted on the first Linux-cell run and never persisted, with working directory `/home/ironpad`, so a file one cell writes is readable by the next; compiled programs live under the hidden `/home/ironpad/.ironpad-bin`, and rerunning an unchanged program does not write it again. (PRD-0066)

#### Scenario: Two processes share a filesystem
- **WHEN** the writer cell of `/public/linux-cells` runs and then the reader cell runs
- **THEN** the reader reports a different pid from the writer and prints the cairo, oslo and quito rows the writer wrote, `n=24`
- **AND** both runs share a single boot

### Requirement: Machine lifetime
The machine SHALL stay up while any Linux cell of its notebook is mounted, SHALL be torn down 300 s after the last one unmounts, and SHALL be replaced at once when a Linux cell in a different notebook runs; a later run on a torn-down machine starts a fresh one with an empty filesystem. (PRD-0066)

#### Scenario: Toggling Preview keeps the machine
- **WHEN** an author runs a Linux cell in Preview, returns to Edit for a minute, and switches back to Preview
- **THEN** the same machine and its files are still there, after one boot in total

#### Scenario: Nothing mounted for the grace window
- **WHEN** no Linux cell of the notebook is mounted for 300 s
- **THEN** the machine is torn down

#### Scenario: Another notebook
- **WHEN** a Linux cell in a second notebook runs while the first notebook's machine is up
- **THEN** the first machine is dropped and the run proceeds on a newly booted one

### Requirement: Streaming terminal panel
A Linux cell SHALL stream the program's terminal output into a "Terminal" panel that follows the tail while the reader is at it, stripping ANSI escape sequences, applying carriage returns and backspaces as a terminal would, and keeping the last 2,000 lines of at most 4,000 characters each behind an "[earlier output trimmed]" line. (PRD-0066)

#### Scenario: Carriage returns move the cursor
- **WHEN** a program prints `10%\r100%` and `aaa\rcc`
- **THEN** the panel shows `100%` and `cca`

#### Scenario: Colour codes are dropped
- **WHEN** a program prints `red \x1b[31` and the run then ends
- **THEN** the panel shows `red ` with no escape fragment

#### Scenario: An empty transcript says why
- **WHEN** a run finishes having printed nothing
- **THEN** the panel shows "The program produced no output." (and "Waiting for output…" while a run is still in flight)

### Requirement: Exit status
The system SHALL run each program under `/bin/sh` and report the exit status the shell prints after it, as "exit N", where a nonzero status marks the cell failed. If the shell cannot execute the program, the system SHALL run it directly and report "finished (inferred)" once its output has been quiet for 1.2 s. (PRD-0066)

#### Scenario: A clean exit
- **WHEN** a program prints `hello` and exits 0
- **THEN** the terminal shows `hello` and the status "exit 0"

#### Scenario: A failing exit
- **WHEN** a program exits with status 101
- **THEN** the status reads "exit 101" in the error style and the Run glyph turns red

### Requirement: Reporting horizon
When a program has not reported an exit after 300 s, the cell SHALL show "still running" and the notice "This program has not signalled that it finished. Output keeps arriving; stopping it means shutting the Linux machine down.", and SHALL keep streaming output and keep its Stop button. (PRD-0066)

#### Scenario: Past the horizon
- **WHEN** a program prints `working` and is still running 300 s later
- **THEN** the cell shows "still running" and the run stays open
- **AND** a later `more` line still appears, and Stop still ends the run

### Requirement: Stopping shuts the machine down
While a Linux cell's run is in flight its Run button SHALL become a Stop button titled "Stop by shutting down the Linux machine" that tears the machine down, ending every in-flight Linux run as "machine stopped" with the notice "The Linux machine was shut down. Running any Linux cell starts a fresh one, and its filesystem starts empty." A Stop during the compile SHALL end the run as "stopped" without booting a machine. (PRD-0066)

#### Scenario: Stop while booting
- **WHEN** Stop is clicked while the machine is still booting
- **THEN** the run ends as "machine stopped" at once, and the program is never written or started on the late-arriving machine

#### Scenario: Stop during the compile
- **WHEN** Stop is clicked while the cell is still compiling
- **THEN** the run ends as "stopped" with the terminal reading "Stopped before the program ran."
- **AND** the abandoned compile never boots a machine when it lands, even if Run was clicked again in the meantime

### Requirement: Never runs without a click
Only a click on a Linux cell's own Run button SHALL run it: Run All, Run All Below, reactive mode, a read-only page's autorun and dependency cascades all skip Linux cells, and an agent's `cells run` on one SHALL fail with "cell <id> is a Linux cell, which an agent cannot run: every run boots a metered pod and a CLI run has nobody watching it. Run it from the notebook in a browser." (PRD-0066, PRD-0052)

#### Scenario: A public notebook full of Linux cells
- **WHEN** a reader opens `/public/linux-cells`, a public notebook with five Linux cells, and waits 5 s
- **THEN** no Linux cell shows any run status

#### Scenario: An agent asks
- **WHEN** an agent runs `cells run` on a Linux cell
- **THEN** the run is refused with the message above and nothing is queued

### Requirement: No CDN contact without a click
A page SHALL make no request to `rt.browserpod.io` unless a reader clicks Run on a Linux cell: the runtime script and the instance's BrowserPod key are fetched only at that click, and the vendor SDK is imported only when a machine boots. (PRD-0066)

#### Scenario: Notebooks without Linux cells
- **WHEN** a visitor opens the home page, a new notebook with a Code cell, and the autorunning `/public/welcome`
- **THEN** no request reaches `rt.browserpod.io` and no script error is raised

#### Scenario: A notebook that has a Linux cell
- **WHEN** an author adds a Linux cell and reloads the notebook
- **THEN** no request reaches `rt.browserpod.io`

### Requirement: Failures stay in the cell
Every Linux-cell failure SHALL render as an error inside that cell, leave the cell runnable again, and leave the rest of the notebook working; a boot not finished within 30 s and a program write not finished within 30 s SHALL fail rather than hang. (PRD-0066)

#### Scenario: The CDN is down
- **WHEN** requests to `rt.browserpod.io` fail and a reader clicks Run on a Linux cell in Preview
- **THEN** the cell's error panel reads "Could not load the Linux runtime from rt.browserpod.io (...). The rest of the notebook is unaffected."
- **AND** the Run button is enabled again and no script error is raised

#### Scenario: A slow boot
- **WHEN** the machine has not started within 30 s
- **THEN** the cell shows "The Linux machine did not start within 30s. rt.browserpod.io may be unreachable."

#### Scenario: No key
- **WHEN** the instance has no BrowserPod key
- **THEN** the cell shows "This ironpad instance has no BrowserPod key configured, so Linux cells cannot run here. Everything else in the notebook still works."

### Requirement: Embeds refuse Linux cells
Inside `/embed/*` a Linux cell SHALL render, in the server's HTML, a disabled Run button titled "Linux cells cannot run in an embed" and the notice "Linux cells need a cross-origin-isolated page, which an embed is not. Open the notebook in ironpad to run this one.", and SHALL never contact `rt.browserpod.io`. (PRD-0066, PRD-0039)

#### Scenario: The refusal is in the raw response
- **WHEN** a client fetches `/embed/shared/{hash}` for a notebook with a Linux cell
- **THEN** the response body contains a `<button` with `disabled` and the text "cross-origin-isolated page"
- **AND** it does not mention `rt.browserpod.io`

#### Scenario: A reader clicks anyway
- **WHEN** a reader force-clicks the disabled Run button in the embed
- **THEN** no request reaches `rt.browserpod.io`

### Requirement: BrowserPod attribution
Every Linux cell frame on a read-only surface SHALL show a visible BrowserPod logo and the name "BrowserPod", linked to `https://browserpod.io` and titled "Linux cells run on BrowserPod, by Leaning Technologies", and nothing else in ironpad SHALL carry that attribution. (PRD-0066)

#### Scenario: One mark per Linux cell
- **WHEN** a reader opens `/public/linux-cells`, which has five Linux cells among its cells
- **THEN** exactly five attribution links render, one inside each Linux cell
- **AND** the logo occupies more than 4x4 px and its vendored SVG asset resolves with status 200

### Requirement: Instance key and its rate limit
An operator SHALL enable Linux cells by setting `BROWSERPOD_KEY` (`--browserpod-key`); with none set, Linux cells say no key is configured. The key SHALL be handed out only on demand, rate-limited per client to a burst of 30 refilling at 30 per minute (`IRONPAD_KEY_RATE_BURST`, `IRONPAD_KEY_RATE_PER_MIN`), and a refused request reads as no key configured. (PRD-0066, PRD-0067)

#### Scenario: A contributor checkout
- **WHEN** the server starts without `BROWSERPOD_KEY`
- **THEN** every other cell type works and a Linux cell's Run reports the no-key message

#### Scenario: A client asks too often
- **WHEN** one client exceeds the key rate limit
- **THEN** its next key request answers as if no key were configured

### Requirement: No typed piping and no saved output
A Linux cell SHALL consume no `cellN` or `last` inputs and SHALL occupy an empty piping slot for later Code cells; it SHALL never record a `saved_output`, so read-only pages show its Run button rather than a snapshot. (PRD-0066)

#### Scenario: A share with a Linux cell
- **WHEN** a notebook with a run Code cell and a Linux cell is shared
- **THEN** the Linux cell carries no `saved_output` and no output type tag

### Requirement: Pod-booting tests are opt-in
Tests that boot a real machine SHALL live in `tests/e2e/linux-pod/` and run only under `cargo make test-linux-cells`, which refuses to run without a real `BROWSERPOD_KEY`; the default Playwright run SHALL ignore that directory and resolve `rt.browserpod.io` to nothing, so `cargo make uat` and CI never boot one. (PRD-0066)

#### Scenario: The gate cannot reach the CDN
- **WHEN** a default-gate test fetches `https://rt.browserpod.io/3.0.0/rust/install.sh`
- **THEN** the request fails with `net::ERR_NAME_NOT_RESOLVED`

#### Scenario: No real key
- **WHEN** `cargo make test-linux-cells` runs with `BROWSERPOD_KEY` unset and no `.hidden/dev.env`
- **THEN** it prints "These specs need a real key; refusing to run against the fake one." and exits 1
