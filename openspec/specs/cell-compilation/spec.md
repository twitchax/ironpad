# cell-compilation Specification

## Purpose
The server compiles one notebook cell's Rust source into a browser-loadable WASM artifact on request: it wraps the source in a generated micro-crate on a pinned nightly, opts heavier build modes in from what the cell says, caches results by content, maps compiler diagnostics back to the author's own lines, and bounds how much cargo work any one client or the whole instance can start.

## Requirements
### Requirement: Compile request and response contract
The system SHALL expose a `compile_cell` server function that takes a `CompileRequest` (`notebook_id`, `cell_id`, `source`, `cargo_toml`, `previous_cell_types`, optional `shared_cargo_toml` and `shared_source`, `cell_type`, `force`) and returns a `CompileResponse` of `wasm_blob`, `diagnostics`, `cached`, `preamble_lines` and optional `js_glue`. A compile that fails still returns `Ok`, with an empty blob. (PRD-0001, PRD-0066)

#### Scenario: Successful compile
- **WHEN** a Code cell whose source is `CellOutput::empty()` is compiled for the first time
- **THEN** the response carries a `wasm_blob` beginning with the bytes `\0asm` and a `js_glue` ES module
- **AND** `cached` is `false`

#### Scenario: Compile error
- **WHEN** a cell whose source is `let x: i32 = "oops";\nCellOutput::empty()` is compiled
- **THEN** the response has an empty `wasm_blob`, no `js_glue`, and a diagnostic with code `E0308` ("mismatched types")

#### Scenario: Request from an older client
- **WHEN** a request arrives with no `cell_type` field
- **THEN** it is treated as a `Code` cell
- **AND** a missing `force` is treated as `false` and a missing `previous_cell_types` as empty

### Requirement: Uncompilable cell types are refused
The system SHALL refuse, with an error naming the cell type's wire tag, any compile or check request whose `cell_type` is `Markdown` or a tag this build does not recognize, before any cache lookup, scaffold or build. (PRD-0066)

#### Scenario: Unknown cell type from a newer client
- **WHEN** `compile_cell` or `check_cell` receives a request with `cell_type` `"Wasi"`
- **THEN** the call fails with an error containing `Wasi` and nothing is built

#### Scenario: Markdown cell
- **WHEN** a request carries `cell_type` `Markdown`
- **THEN** the call fails with an error saying a Markdown cell has no Rust source to build

### Requirement: Cell id validation
The system MUST reject a compile or check request whose `cell_id` is not 1 to 64 characters of `[A-Za-z0-9_-]`, before the id touches the filesystem or the generated manifest. (PRD-0036)

#### Scenario: Path traversal id
- **WHEN** a request carries `cell_id` `../x`
- **THEN** the call fails with an error containing `invalid cell_id` and `expected 1-64 chars of [A-Za-z0-9_-]`
- **AND** no workspace directory is created in the cache

#### Scenario: Accepted ids
- **WHEN** a request carries a UUID such as `550e8400-e29b-41d4-a716-446655440000`, `Cell_42-abc`, or 64 `a` characters
- **THEN** the id is accepted

### Requirement: Compile target follows the cell type
The system SHALL derive the compile target from the request's `cell_type` alone: `Code` cells build for `wasm32-unknown-unknown` and `Linux` cells for `wasm32-browserpod-linux-musl`, with the target folded into the cache key so the two never share a cached artifact. (PRD-0066)

#### Scenario: Same source, different cell types
- **WHEN** the same source and manifest are compiled once as a `Code` cell and once as a `Linux` cell
- **THEN** the two compiles have different cache keys
- **AND** each builds for its own target triple

### Requirement: Generated micro-crate manifest
The system SHALL compile each Code cell as a standalone crate named `cell-{cell_id}` on edition 2024 with `crate-type = ["cdylib"]`, an empty `[workspace]`, the `ironpad-cell` runtime as a path dependency, and `wasm-bindgen` pinned exactly (`=X.Y.Z`) to the server's wasm-bindgen CLI version, falling back to `"0.2"` when the CLI version cannot be read. (PRD-0001, PRD-0031, PRD-0060)

#### Scenario: Exact wasm-bindgen pin
- **WHEN** the server's `wasm-bindgen --version` reports `0.2.127`
- **THEN** the generated manifest declares `wasm-bindgen = "=0.2.127"`

#### Scenario: Edition 2024 keyword rules apply
- **WHEN** a cell names a variable `gen`
- **THEN** the compile fails with rustc's reserved-keyword error rather than compiling as edition 2021 would

### Requirement: Shared and per-cell dependency merge
The system SHALL merge the notebook's shared Cargo.toml dependencies with the cell's own, the cell's entry winning for the same crate name (hyphens and underscores equal) across the inline and `[dependencies.NAME]` forms, SHALL drop any user-declared `ironpad-cell`, and SHALL forward other sections such as `[profile.release]`, a cell section replacing a shared one of the same header. (PRD-0007, PRD-0016)

#### Scenario: Cell overrides a shared subtable
- **WHEN** the shared manifest has `[dependencies.rayon]` with `version = "1.9"` and the cell manifest has `rayon = "1.10"`
- **THEN** the merged dependencies contain `rayon = "1.10"` and no `rayon` subtable

#### Scenario: User ironpad-cell is ignored
- **WHEN** a cell manifest declares `ironpad-cell = { path = "../ironpad-cell" }` and `serde = "1"`
- **THEN** the build uses the server's own `ironpad-cell` and keeps `serde`

#### Scenario: Shared profile forwarded
- **WHEN** the shared manifest carries `[profile.release]` with `opt-level = 1`
- **THEN** the generated manifest carries that profile

### Requirement: Cell source becomes the body of cell_main
The system SHALL compile an ordinary Code cell's source as the body of a block expression converted into `CellOutput` with `.into()`, exported through a `#[wasm_bindgen]` `cell_main(input_ptr: u32, input_len: u32) -> u32` entry point that installs a panic hook, so any type with `From<T> for CellOutput` is a valid final expression. (PRD-0001, PRD-0060)

#### Scenario: Value-returning cell
- **WHEN** a cell's source is `CellOutput::text(format!("{}", 42))` and it is run
- **THEN** the compiled module's `cell_main` yields output text `42`

#### Scenario: New syntax reaches rustc, not the bindgen macro
- **WHEN** a cell's source is `let g = gen { yield 1u32; yield 2; yield 3; };\nlet total: u32 = g.sum();\nCellOutput::from(total)`
- **THEN** the compile succeeds and produces a WASM module

### Requirement: Async wrapper on await
The system SHALL compile a cell whose source contains the substring `.await` anywhere, including inside a string or comment, with an async `cell_main` that awaits the cell body as an `async` block. (PRD-0001, PRD-0043)

#### Scenario: Await anywhere opts in
- **WHEN** a cell's source contains `.await`
- **THEN** its `cell_main` is generated as an `async fn`

#### Scenario: Synchronous cell stays synchronous
- **WHEN** a cell's source contains no `.await`
- **THEN** its `cell_main` is a plain function

### Requirement: Simulation and LiveView entry points
The system SHALL place the source of a cell whose non-comment lines contain `impl Simulation for <Name>` or `impl LiveView for <Name>` at module level rather than in a function body, and export `cell_main` (initialize and produce the first frame or content) and `cell_tick` (advance one tick). (PRD-0017, PRD-0019)

#### Scenario: Simulation cell
- **WHEN** a cell defines a struct and `impl Simulation for Ball`
- **THEN** the compiled module exports both `cell_main` and `cell_tick`

#### Scenario: Commented-out impl
- **WHEN** the only occurrence is on a line starting with `//`
- **THEN** the cell compiles as an ordinary `cell_main` cell

### Requirement: Upstream inputs bound only where referenced
The system SHALL bind an upstream output as `cellN: <type tag>` only when the source references `cellN` as a whole word, bind every typed slot plus `last` (the last typed slot) when it references `last`, and reconstruct the raw input buffer when it reads `__ironpad_inputs__`. Bindings MUST NOT raise unused-variable warnings. (PRD-0060)

#### Scenario: Only the referenced slot is bound
- **WHEN** upstream type tags are `["u32", "String"]` and the source is `cell1.len()`
- **THEN** the cell binds `cell1: String` and no `cell0`

#### Scenario: Independent cell
- **WHEN** upstream type tags are `["u32", "String"]` and the source is `40 + 2`
- **THEN** no input binding or buffer reconstruction is generated

#### Scenario: The last alias
- **WHEN** upstream type tags are `["u32", "", "bool"]` and the source is `(cell0, cell2, last)`
- **THEN** the cell binds `cell0: u32`, `cell2: bool`, and `last` as a reference to `cell2`

#### Scenario: Deserialization mismatch
- **WHEN** a bound slot's bytes do not deserialize as its tag at run time
- **THEN** the cell panics with `failed to deserialize cellN as` the tag, naming the input's byte length

### Requirement: Notebook shared source module
The system SHALL compile the request's `shared_source` as a module `shared` in every cell's crate, reachable as `shared::…`, without reporting dead-code warnings for shared helpers the cell does not call. (PRD-0007, PRD-0044)

#### Scenario: Helper used by a sibling only
- **WHEN** the shared source defines `used_here`, `used_by_another_cell`, an unused constant and an unused struct, and the cell calls only `shared::used_here()`
- **THEN** the compile succeeds with no "never used" warning

### Requirement: Rayon cells build with atomics
The system SHALL build a cell whose merged dependencies (shared or cell, inline or dotted) include `rayon` with the `ironpad-cell` `rayon` feature, `-C target-feature=+atomics,+bulk-memory,+mutable-globals`, shared-memory linker arguments (2 GiB maximum memory), `-Zbuild-std=std,panic_abort`, and `wasm-opt --enable-threads`, in a target directory shared by all rayon cells. (PRD-0024, PRD-0067)

#### Scenario: Rayon cell links a worker-pool module
- **WHEN** a cell with `rayon = "1"` sums `(0..1000)` with `par_iter()`
- **THEN** the compiled module exports `initThreadPool`, `wbg_rayon_start_worker`, `__wbindgen_thread_destroy`, `__tls_base` and `memory`

#### Scenario: Dotted dependency form
- **WHEN** rayon appears only as `[dependencies.rayon]` with `version = "1.10"`
- **THEN** the cell builds in the atomics mode

### Requirement: Autodiff cells build with Enzyme
The system SHALL build a cell whose source or shared source contains `autodiff_forward`, `autodiff_reverse` or `std::autodiff` with a crate-root `#![feature(autodiff)]`, `-Zautodiff=Enable`, and a release profile forced to `lto = "fat"` and `codegen-units = 1` while keeping the author's other profile keys. (PRD-0041)

#### Scenario: Author profile overridden, other keys kept
- **WHEN** an autodiff cell's shared manifest sets `[profile.release]` with `opt-level = 1`, `lto = false`, `codegen-units = 16`
- **THEN** the generated profile has `lto = "fat"`, `codegen-units = 1` and `opt-level = 1`

#### Scenario: Reverse mode through a loop
- **WHEN** shared source defines a drag simulation with `#[autodiff_reverse]` and a cell calls its derivative
- **THEN** the compile succeeds and produces a WASM module

### Requirement: SIMD cells build with simd128
The system SHALL build a cell whose source or shared source contains `std::simd`, `core::simd`, `std::arch::wasm32`, `fearless_simd`, `wide::` or `pulp::` with `-C target-feature=+simd128` and a crate-root `#![feature(portable_simd)]`, merging target features with any atomics features into one flag. (PRD-0042)

#### Scenario: Portable SIMD in the browser
- **WHEN** a cell computes `(a * b).reduce_sum()` over `f32x4` arrays `[1,2,3,4]` and `[5,6,7,8]` and is run
- **THEN** its output reads `dot=70`

#### Scenario: Rayon plus SIMD
- **WHEN** a cell both depends on rayon and uses `std::simd`
- **THEN** the build receives exactly one `-C target-feature=` flag containing `+atomics,+bulk-memory,+mutable-globals,+simd128`

#### Scenario: Shared source opts the whole notebook in
- **WHEN** the notebook's shared source mentions `std::simd`
- **THEN** every cell of that notebook builds with simd128

### Requirement: Generator block syntax gate
The system SHALL add a crate-root `#![feature(gen_blocks)]` to a cell whose source or shared source contains a `gen` block form (`gen {`, `gen move {`, or `async gen`, any whitespace before the brace), and MUST NOT add it for a bare word `gen`. (PRD-0060)

#### Scenario: Block forms detected
- **WHEN** a cell contains `let it = gen move { yield x; };`
- **THEN** its crate root carries `#![feature(gen_blocks)]`

#### Scenario: Prose and identifiers are not an opt-in
- **WHEN** a cell's only occurrences are `// generate the gen next` and `let gen_count = 3;`
- **THEN** no gate is added

### Requirement: Coroutine syntax gate
The system SHALL add a crate-root `#![feature(coroutines, coroutine_trait, stmt_expr_attributes)]` to a cell whose source or shared source contains `#[coroutine]`, `CoroutineState` or `ops::Coroutine`, and MUST NOT treat a bare `yield` as an opt-in. (f96e23c)

#### Scenario: Coroutine attribute
- **WHEN** a cell contains `#[coroutine]`
- **THEN** its crate root carries the coroutine feature gate

### Requirement: Pinned cell toolchain
The system SHALL build every Code cell with the explicitly selected toolchain `nightly-2026-05-19` whatever its features or the host's default, ignoring inherited `RUSTUP_TOOLCHAIN`, `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS` and `CARGO_BUILD_RUSTFLAGS`, and Linux cells with `browserpod-3.0.0`. (PRD-0042, PRD-0066, PRD-0067)

#### Scenario: Every feature combination on one pin
- **WHEN** a Code cell is built with any combination of atomics, autodiff and SIMD
- **THEN** cargo is invoked as `cargo +nightly-2026-05-19`

#### Scenario: Pins held in sync
- **WHEN** the cell toolchain, the workspace `rust-toolchain.toml` channel, the Dockerfile install, CI, or `BROWSERPOD_NIGHTLY` in `docker/browserpod.env` disagree
- **THEN** `cargo make test` fails

### Requirement: Cell builds inherit only an allowlisted environment
The system SHALL start every cell build's and live check's cargo process from an empty environment, adding back from the server's only `PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`, `LC_ALL`, `TMPDIR`, `RUSTUP_HOME`, `RUSTC_WRAPPER`, `SCCACHE_*`, `CC`, `CXX`, `AR`, `PKG_CONFIG_PATH`, `LD_LIBRARY_PATH`, `SSL_CERT_FILE`, `SSL_CERT_DIR`, and the upper- and lowercase `HTTP_PROXY`, `HTTPS_PROXY` and `NO_PROXY`, then setting `CARGO_HOME`, `CARGO_TARGET_DIR`, the compile proxy and the cell's `RUSTFLAGS` itself, so no server secret reaches `env!`, `option_env!`, a build script or a proc macro.

#### Scenario: A cell reads a server secret
- **WHEN** a cell's source uses `env!("GITHUB_CLIENT_SECRET")` or `option_env!` of any variable outside the allowlist
- **THEN** rustc sees the variable as unset

### Requirement: Cell builds run as the configured build user
When `IRONPAD_BUILD_UID` names a uid, the system SHALL run every cell build's and live check's cargo process as that uid and the gid of the same number, with `HOME` set to `<cache>/build-home`, after handing the top level of `CARGO_HOME`, the target directory and that home to it, and SHALL hand each scaffolded micro-crate's `Cargo.lock` (created empty when missing) to it while the micro-crate directory and its sources stay the server's. A value that is not a uid SHALL fail the compile rather than build as the server's user. Unset, builds run as the server's own user. The server SHALL read a build's artifact only after copying it out of the target directory without following a symlink, refusing anything that is not a regular file. The deploy image sets it to 2000. (PRD-0030, PRD-0038 T-015)

#### Scenario: A cell embeds the accounts database
- **WHEN** a cell in the deploy image uses `include_bytes!("/ironpad/data/ironpad.db/wal/00000000000000000000.wal")`
- **THEN** the compile fails with a permission error

#### Scenario: An artifact replaced by a link
- **WHEN** the artifact path in the target directory is a symlink after cargo exits
- **THEN** the compile fails instead of reading the link's target

### Requirement: Build invocation and timeout
The system SHALL build with `cargo build --target <triple> --release --message-format=json`, bounded by `IRONPAD_BUILD_TIMEOUT_SECS` (default 300 seconds); on expiry it MUST kill the whole cargo process group and fail the call with `build invocation failed: compilation timed out after <N>s`. (PRD-0001, PRD-0036)

#### Scenario: Runaway build
- **WHEN** a build exceeds the timeout
- **THEN** cargo and every rustc or build-script child it spawned are killed
- **AND** `compile_cell` returns an error containing `compilation timed out after 300s`

#### Scenario: Grandchildren die too
- **WHEN** a timed subprocess has spawned a grandchild that would outlive it
- **THEN** the grandchild is killed with the group

#### Scenario: Abandoned compile
- **WHEN** the client disconnects mid-build, so the compile is dropped before cargo, wasm-bindgen or wasm-opt exits
- **THEN** that subprocess's whole process group is killed, as on a timeout, before the cell lock and build slot are reused

### Requirement: wasm-bindgen post-processing
The system SHALL run `wasm-bindgen --target web --no-typescript` over a successful Code cell build, returning the generated ES module as `js_glue` and the transformed module as the blob, bounded at 120 seconds; a wasm-bindgen failure, timeout or missing CLI fails the call with an error saying so. (PRD-0031, PRD-0067)

#### Scenario: Missing CLI
- **WHEN** no `wasm-bindgen` is on the server's PATH
- **THEN** the compile fails with an error naming the missing CLI and `cargo make install-tools`

#### Scenario: Post-processing failure
- **WHEN** wasm-bindgen exits non-zero
- **THEN** the compile fails with an error beginning `build invocation failed: wasm-bindgen failed:` followed by its stderr

### Requirement: Best-effort wasm-opt
The system SHALL pass each successful Code cell blob through `wasm-opt -O3 --debuginfo` under a 120-second cap, and SHALL return the unoptimized blob, not an error, when wasm-opt is missing, fails or times out. Linux cell artifacts are never passed to wasm-opt. (PRD-0001, PRD-0036)

#### Scenario: wasm-opt not installed
- **WHEN** the server has no `wasm-opt`
- **THEN** compiles still succeed with the unoptimized blob

#### Scenario: Failure leaves no temp files
- **WHEN** wasm-opt rejects its input, or the compile is abandoned while wasm-opt runs
- **THEN** no temporary blob is left in the work directory (and on rejection the original bytes are returned)

### Requirement: Diagnostics mapped to the author's lines
The system SHALL report rustc's `error`, `warning`, `note` and `help` messages (help and note as `Note`, internal compiler errors as `Error`) with their codes, shift primary spans in the cell body back to the author's line numbers, drop spans in generated lines or dependency files, and append rustc's help and note children to the message. (PRD-0001, PRD-0036)

#### Scenario: Type error lands on the author's line
- **WHEN** a cell's second line contains a mismatched-type error
- **THEN** the diagnostic's span has `line_start` 2 and code `E0308`

#### Scenario: Help text kept
- **WHEN** rustc warns `unused variable: x` with a help child
- **THEN** the warning's message contains `help: if this is intentional, prefix it with an underscore`

#### Scenario: Error in a dependency
- **WHEN** an error's primary span is in a registry crate's file
- **THEN** the diagnostic is reported with no span

#### Scenario: Injected feature gates do not shift lines
- **WHEN** a SIMD cell gains its crate-root gate
- **THEN** its diagnostics still point at the author's lines

### Requirement: Shared-source errors are labeled
The system SHALL report, in an ordinary cell's compile or check, a diagnostic whose primary span lies in the notebook's shared source with no span and a message prefixed `(in shared source)`. (PRD-0036, PRD-0044)

#### Scenario: Shared helper broken
- **WHEN** a cell's compile fails because the shared source refers to an undefined `foo`
- **THEN** the cell's diagnostic message begins `(in shared source)` followed by rustc's `cannot find value` message
- **AND** the diagnostic carries no span, since no line of this cell is at fault

### Requirement: Unstructured build failures are reported with paths redacted
The system SHALL, when a failed build yields no structured diagnostics, return one `Error` whose message is `Compilation failed:` followed by cargo's stderr (or stdout), and otherwise append the stderr as a `Build stderr:` error, replacing the crate directory with `<cell>`, the cache directory with `<cache>` and the server's home with `~`. (PRD-0036)

#### Scenario: Cargo fails before rustc runs
- **WHEN** a build fails with an error on stderr and no compiler message in cargo's JSON output, such as a manifest cargo cannot resolve
- **THEN** the response carries one error beginning `Compilation failed:` that contains cargo's message
- **AND** the message contains no server filesystem path

#### Scenario: Compiler errors plus stderr
- **WHEN** a build fails with structured compiler errors and cargo also wrote to stderr
- **THEN** the structured errors are followed by an error beginning `Build stderr:`

### Requirement: Host imports link
The system SHALL link cells that call host-provided runtime functions, including `sim::read`, `host_message` and `blocking::*`, against the browser executor's `env` import module rather than failing with undefined symbols. (PRD-0031, PRD-0043)

#### Scenario: Simulation bus and host message
- **WHEN** a cell calls `sim::read("regression_key")` and `host_message("prd-0031-regression")`
- **THEN** the compile succeeds and produces a WASM module

### Requirement: Content-addressed compile cache
The system SHALL key each compile by a 64-hex blake3 digest of length-framed inputs: source, cell manifest, target triple, the referenced upstream type tags (none for Linux cells), shared manifest and shared source (absent distinct from empty), the atomics, autodiff and SIMD verdicts, cache epoch 12, and the toolchain fingerprint. (PRD-0001, PRD-0031, PRD-0038, PRD-0060)

#### Scenario: Pinned key
- **WHEN** the key is computed for source `40 + 2`, an empty manifest, no upstream types, no shared inputs, a Code cell and fingerprint `tc`
- **THEN** it is `25d1ed89955f7de1e4f55a60f6792d551689b5d61dab349057bac0d9fdd8f313`

#### Scenario: Field boundaries are unambiguous
- **WHEN** keys are computed for source `ab` with manifest `c` and for source `a` with manifest `bc`
- **THEN** the keys differ

#### Scenario: Unreferenced upstream types do not fork the key
- **WHEN** a cell referencing no upstream input is compiled with the same source, manifests and shared source at index 7 of one notebook and index 2 of another
- **THEN** both compiles produce the same key and the second is a cache hit

#### Scenario: Manifest edits invalidate
- **WHEN** only the cell's Cargo.toml changes from `serde = "1"` to `rand = "0.8"`
- **THEN** the key changes and the next compile builds afresh

### Requirement: Cache hits and stores
The system SHALL answer a compile whose key exists under `{cache_dir}/blobs/` with the stored blob, glue and warnings, `cached: true`, and no build. It SHALL store only successful compiles, written atomically, and SHALL treat an unreadable entry as a miss. (PRD-0001, PRD-0036)

#### Scenario: Warnings survive a hit
- **WHEN** a cell that compiled with an `unused_variables` warning is compiled again unchanged
- **THEN** the response is `cached: true` and still carries that warning

#### Scenario: Failures are never cached
- **WHEN** a cell fails to compile and is compiled again unchanged
- **THEN** the second compile runs cargo again

### Requirement: Force recompile
The system SHALL skip the cache lookup when a request sets `force: true`, build afresh through admission control, and on success overwrite the cached entry under the same key with the new result. (PRD-0047)

#### Scenario: Forced compile of a cached cell
- **WHEN** a cached cell is compiled with `force: true`
- **THEN** cargo runs, the response has `cached: false`, and the stored entry is replaced

### Requirement: Toolchain fingerprint
The system SHALL fingerprint its toolchain as the pinned rustc's `--version` output and the wasm-bindgen CLI version, read from the file named by `IRONPAD_TOOLCHAIN_VERSIONS_FILE` when present and well formed and probed otherwise, and SHALL serve it from `get_toolchain_fingerprint` so browsers compute identical keys. (PRD-0031, PRD-0047, PRD-0067)

#### Scenario: Baked versions file
- **WHEN** the versions file holds `rustc 1.97.0-nightly (9eb3be26b 2026-05-18)` and `wasm-bindgen 0.2.127` on two lines
- **THEN** the fingerprint equals the one probing those binaries would give
- **AND** the boot log's `toolchain fingerprint` line reports source `baked` rather than `probed`

#### Scenario: Truncated file
- **WHEN** the versions file holds only the rustc line
- **THEN** the server probes the toolchain instead

#### Scenario: Toolchain upgrade
- **WHEN** the deploy changes the rustc or wasm-bindgen version
- **THEN** every existing cache entry misses on its next compile

### Requirement: Same-cell compiles serialize
The system SHALL run concurrent compiles or checks that share a cell id one at a time, treating ids that differ only by `-` versus `_` as the same cell, while compiles of distinct cells proceed concurrently. (PRD-0036)

#### Scenario: Two tabs, one cell
- **WHEN** two compiles of `cell-1` arrive together
- **THEN** the second waits for the first to finish

#### Scenario: Dash and underscore variants
- **WHEN** a compile of `cell-a-b` is in flight and one of `cell-a_b` arrives
- **THEN** the second waits

### Requirement: Build concurrency cap
The system SHALL let only cache misses take a build slot, run at most `--max-concurrent-builds` (`IRONPAD_MAX_CONCURRENT_BUILDS`, default 3, minimum 1) compiles at once, and queue others up to `IRONPAD_BUILD_QUEUE_TIMEOUT_SECS` (default 180) before failing with an at-capacity error. (PRD-0052)

#### Scenario: Queue then proceed
- **WHEN** every slot is held and another miss arrives
- **THEN** it starts as soon as a slot frees within the queue timeout

#### Scenario: At capacity
- **WHEN** no slot frees within the queue timeout
- **THEN** the compile fails with `the server is at build capacity right now. Cached runs are unaffected; retry shortly`

#### Scenario: Hits are free
- **WHEN** every slot is held and a cached cell is compiled
- **THEN** it is answered from the cache without waiting

### Requirement: Per-client build rate limit
The system SHALL charge each client one token per build start (cache miss) from a bucket of `IRONPAD_BUILD_RATE_BURST` (default 20) refilling at `IRONPAD_BUILD_RATE_PER_MIN` (default 30), keyed by `Fly-Client-IP`, else the first `X-Forwarded-For` hop, else one shared `local` key, refusing immediately when empty. (PRD-0052)

#### Scenario: Burst exhausted
- **WHEN** a client starts more builds than its burst allows faster than the refill rate
- **THEN** the next miss fails with `rate limited: too many builds started from this client. Cached runs are unaffected; wait a minute and retry`
- **AND** a different client is unaffected

#### Scenario: Client identity
- **WHEN** a request carries `X-Forwarded-For: 9.9.9.9, 10.0.0.1` and `Fly-Client-IP: 1.2.3.4`
- **THEN** it is charged to `1.2.3.4`

#### Scenario: Invalid override
- **WHEN** `IRONPAD_BUILD_RATE_BURST` is `0` or not a number
- **THEN** the default burst of 20 applies
