---
id: PRD-0001
title: "ironpad MVP: First Pass Implementation"
status: active
owner: Aaron Roney
created: 2026-03-06
updated: 2026-03-06

principles:
  - "Reference MegaPrd.md as the single source of truth for architecture and design decisions"
  - "Leptos 0.8 + Axum 0.8 + Thaw 0.5.0-beta — consult official docs heavily"
  - "Binary must be runnable outside Docker via --data-dir and --cache-dir flags"
  - "Every feature must be verifiable by an agent via cargo make tasks and Playwright"
  - "Small, focused tasks — each completable in one agent session"
  - "ironpad-cell distributed as a path dependency from the server filesystem"
  - "Monaco bundled with the server — no CDN dependencies"

references:
  - name: "MegaPrd (architecture, design, all details)"
    url: "MegaPrd.md"
  - name: "Leptos 0.8 docs"
    url: "https://leptos.dev/"
  - name: "Leptos start-axum-workspace template"
    url: "https://github.com/leptos-rs/start-axum-workspace"
  - name: "Thaw UI (0.5.0-beta for Leptos 0.8)"
    url: "https://github.com/thaw-ui/thaw"
  - name: "Monaco Editor"
    url: "https://microsoft.github.io/monaco-editor/"
  - name: "cargo-leptos"
    url: "https://github.com/leptos-rs/cargo-leptos"

acceptance_tests:
  - id: uat-001
    name: "Server starts and home page loads in browser"
    command: cargo make uat
    uat_status: unverified
  - id: uat-002
    name: "Can create a new notebook from the home page"
    command: cargo make uat
    uat_status: unverified
  - id: uat-003
    name: "Can add a cell to a notebook and see Monaco editor"
    command: cargo make uat
    uat_status: unverified
  - id: uat-004
    name: "Can compile a trivial cell and see WASM execution output"
    command: cargo make uat
    uat_status: unverified
  - id: uat-005
    name: "Two-cell data flow works (cell 0 output piped as cell 1 input via bincode)"
    command: cargo make uat
    uat_status: unverified
  - id: uat-006
    name: "Compiler errors render inline in Monaco with span highlighting"
    command: cargo make uat
    uat_status: unverified
  - id: uat-007
    name: "Notebook persists after save and page reload"
    command: cargo make uat
    uat_status: unverified
  - id: uat-008
    name: "Docker container builds and serves the app"
    command: cargo make docker-uat
    uat_status: unverified
  - id: uat-009
    name: "Sample notebook is pre-loaded on first run"
    command: cargo make uat
    uat_status: unverified
  - id: uat-010
    name: "Binary accepts --data-dir and --cache-dir flags and uses them"
    command: cargo make test
    uat_status: unverified

tasks:
  # ── Phase 0: Project Scaffolding ───────────────────────────────────────
  - id: T-001
    title: "Initialize Cargo workspace with all crate stubs"
    priority: 1
    status: done
    notes: >
      Create root Cargo.toml (workspace members), and stub crates:
      crates/ironpad-app/ (Leptos components, shared),
      crates/ironpad-frontend/ (hydration entry),
      crates/ironpad-server/ (Axum binary),
      crates/ironpad-cell/ (injected into user cells),
      crates/ironpad-common/ (shared types).
      Follow the start-axum-workspace pattern for Leptos 0.8.
      Add all dependencies at correct versions:
      leptos 0.8, leptos_router 0.8, leptos_meta 0.8, leptos_axum 0.8,
      axum 0.8, thaw 0.5.0-beta, serde, bincode, blake3, clap, tokio,
      tracing, tracing-subscriber, uuid, chrono,
      wasm-bindgen, web-sys, js-sys.
      Ensure `cargo build` succeeds with stubs.

  - id: T-002
    title: "Configure cargo-leptos metadata and features"
    priority: 1
    status: done
    notes: >
      Add [package.metadata.leptos] to root or app Cargo.toml:
      output-name, site-root, site-pkg-dir, site-addr, bin-features=[ssr],
      lib-features=[hydrate]. Configure SSR and hydrate feature flags
      across all crates. Ensure `cargo leptos build` succeeds.

  - id: T-003
    title: "Create Makefile.toml with all dev/CI/UAT tasks"
    priority: 1
    status: done
    notes: >
      Follow microralph/razel patterns. Tasks:
      install-tools (cargo-binstall nextest, cargo-leptos, playwright),
      dev (cargo leptos watch), build (cargo leptos build --release),
      fmt / fmt-check, clippy, test (cargo nextest run),
      ci (fmt-check + clippy + test), uat (ci + playwright),
      playwright (run playwright tests), playwright-install (install browsers),
      docker-build, docker-up, docker-down, docker-uat.
      The `uat` task is the one true gate.

  - id: T-004
    title: "Create Dockerfile and docker-compose.yml"
    priority: 2
    status: done
    notes: >
      Multi-stage Dockerfile: (1) builder stage with Rust toolchain +
      wasm32-unknown-unknown target + cargo-leptos + wasm-opt/binaryen,
      (2) runtime stage with just the binary + site assets + Rust toolchain
      (needed for cell compilation). See MegaPrd §9 for details.
      docker-compose.yml with volumes for notebooks and cache.
      Expose port 3000. Set IRONPAD_DATA_DIR and IRONPAD_CACHE_DIR env vars.

  - id: T-005
    title: "CLI argument parsing and configuration module"
    priority: 1
    status: done
    notes: >
      Use clap in ironpad-server for: --data-dir (default: ./data),
      --cache-dir (default: ./cache), --port (default: 3000),
      --ironpad-cell-path (default: ./crates/ironpad-cell).
      Also read from env vars: IRONPAD_DATA_DIR, IRONPAD_CACHE_DIR,
      IRONPAD_PORT, IRONPAD_CELL_PATH. CLI args override env vars.
      Store config in a shared AppConfig struct accessible via Leptos context.

  # ── Phase 1: Core Types ────────────────────────────────────────────────
  - id: T-006
    title: "Define shared types in ironpad-common"
    priority: 1
    status: done
    notes: >
      Types: CompileRequest { cell_id, source, cargo_toml },
      CompileResponse { wasm_blob: Vec<u8>, diagnostics: Vec<Diagnostic>, cached: bool },
      Diagnostic { message, severity, spans: Vec<Span> },
      Span { line_start, line_end, col_start, col_end, label },
      NotebookManifest { id, title, created_at, updated_at, compiler_version, cells },
      CellManifest { id, order, label },
      NotebookSummary { id, title, updated_at, cell_count } (for list view).
      All types derive Serialize, Deserialize, Clone, Debug.

  - id: T-007
    title: "Implement ironpad-cell crate"
    priority: 1
    status: done

  # ── Phase 2: Server-Side Compilation Pipeline ──────────────────────────
  - id: T-008
    title: "Micro-crate scaffolding service"
    priority: 2
    status: done
    notes: >
      Given (cell_id, source, cargo_toml, session_id), write a micro-crate to
      {cache_dir}/workspaces/{session_id}/{cell_id}/ with:
      Cargo.toml (inject ironpad-cell as path dep),
      src/lib.rs (wrap user code in cell_main function).
      See MegaPrd §5.2 and §5.4 for wrapper template.
      Ensure crate-type = ["cdylib"] is set.

  - id: T-009
    title: "Cargo build invocation for WASM target"
    priority: 2
    status: done
    notes: >
      Invoke `cargo build --target wasm32-unknown-unknown --release
      --message-format=json` in the micro-crate directory.
      Set CARGO_HOME to shared registry cache dir.
      Set CARGO_TARGET_DIR per-session for incremental reuse.
      Hard timeout at 30s. Capture stdout (JSON messages) and stderr.
      Return the .wasm blob path on success, or raw output on failure.

  - id: T-010
    title: "Blake3 content-hash caching for compiled WASM blobs"
    priority: 2
    status: done
    notes: >
      Hash: blake3(source || cargo_toml || "wasm32-unknown-unknown").
      Cache path: {cache_dir}/blobs/{hash}.wasm.
      On cache hit, return blob immediately (skip compilation).
      On cache miss, compile and store result.
      Add cache stats logging via tracing.

  - id: T-011
    title: "Rustc JSON diagnostic parser"
    priority: 2
    status: done
    notes: >
      Parse --message-format=json output from cargo build.
      Extract: message, level (error/warning/note), code,
      spans (file, line_start, line_end, col_start, col_end, label, text).
      Map span line numbers to user code by subtracting the wrapper offset
      (number of lines in the auto-generated cell_main wrapper).
      Convert to ironpad-common Diagnostic types.
      Add unit tests with sample rustc JSON output.

  - id: T-012
    title: "WASM optimization (wasm-opt, best-effort)"
    priority: 3
    status: done
    notes: >
      After successful compilation, attempt `wasm-opt -Oz` on the blob.
      If wasm-opt is not installed, skip silently (log at debug level).
      This is best-effort for smaller blob sizes.

  - id: T-013
    title: "compile_cell server function"
    priority: 2
    status: done
    notes: >
      Leptos #[server] fn that ties together T-008 through T-012:
      receive CompileRequest, check cache (T-010), on miss scaffold (T-008),
      build (T-009), parse diagnostics (T-011), optimize (T-012),
      cache result, return CompileResponse.
      Include timing info in response via tracing spans.

  # ── Phase 3: Notebook Persistence ──────────────────────────────────────
  - id: T-014
    title: "Notebook filesystem CRUD"
    priority: 2
    status: done
    notes: >
      Under {data_dir}/notebooks/{id}/:
      Create: generate UUID, write ironpad.json manifest + cells/ dir.
      Read: parse ironpad.json, load cell sources.
      Update: rewrite ironpad.json (title, cell order, updated_at).
      Delete: remove entire notebook directory.
      List: scan notebooks/ dir, return Vec<NotebookSummary>.
      See MegaPrd §8 for directory structure and manifest schema.

  - id: T-015
    title: "Cell filesystem CRUD"
    priority: 2
    status: done
    notes: >
      Under {notebook_dir}/cells/{cell_id}/:
      Add cell: create dir with default source.rs and Cargo.toml.
      Update source: overwrite source.rs.
      Update cargo_toml: overwrite Cargo.toml.
      Delete cell: remove dir + update manifest cell list.
      Reorder: update manifest cells array order fields.
      Default Cargo.toml per MegaPrd §8.3.

  - id: T-016
    title: "Server functions for notebook and cell operations"
    priority: 2
    status: done
    notes: >
      Leptos #[server] fns: list_notebooks, create_notebook(title),
      get_notebook(id), update_notebook(id, title),
      delete_notebook(id), add_cell(notebook_id, after_cell_id),
      update_cell_source(notebook_id, cell_id, source),
      update_cell_cargo_toml(notebook_id, cell_id, cargo_toml),
      delete_cell(notebook_id, cell_id),
      reorder_cells(notebook_id, cell_ids: Vec<String>),
      rename_cell(notebook_id, cell_id, label).
      All use AppConfig from context for data_dir.

  - id: T-017
    title: "Sample notebook pre-loaded on first run"
    priority: 3
    status: todo
    notes: >
      On startup, if {data_dir}/notebooks/ is empty, create a sample notebook
      titled "Welcome to ironpad" with two cells demonstrating the Fibonacci
      example from MegaPrd Appendix A. Cell 0 generates data, Cell 1 consumes it.

  # ── Phase 4: UI — App Shell ────────────────────────────────────────────
  - id: T-018
    title: "Leptos app root component with routing"
    priority: 2
    status: done
    notes: >
      In ironpad-app: root App component with Router.
      Routes: "/" (home/notebook list), "/notebook/{id}" (editor).
      Use leptos_router 0.8.
      Wrap in Thaw ConfigProvider + ThemeProvider (dark theme default).
      Include leptos_meta for <Title> and <Stylesheet>.

  - id: T-019
    title: "App layout with header and status bar"
    priority: 2
    status: done
    notes: >
      Thaw Layout component: header with "ironpad" branding,
      main content area (child routes), footer status bar.
      Header: logo/name on left, notebook title (when in editor) center,
      save button right. Status bar: compiler version, cell count, last save time.
      Use Thaw components: Layout, LayoutHeader, LayoutFooter.

  - id: T-020
    title: "Home page with notebook list and create button"
    priority: 2
    status: done

  - id: T-021
    title: "Notebook editor page skeleton"
    priority: 2
    status: done
    notes: >
      Route /notebook/{id}: fetch notebook via get_notebook server fn.
      Render notebook title (editable), list of cell components (ordered),
      "Add Cell" buttons between cells and at the end.
      Wire up notebook-level state (cell order, which cell is active).
      Use Leptos signals for reactive cell state management.

  # ── Phase 5: UI — Monaco Integration ───────────────────────────────────
  - id: T-022
    title: "Bundle Monaco editor JS/CSS with the server"
    priority: 2
    status: done
    notes: >
      Monaco must be served from the ironpad server (no CDN, per MegaPrd OQ5 option b).
      Options: (a) npm install monaco-editor, copy dist to site assets,
      (b) use monaco-editor ESM bundle.
      Serve monaco JS/CSS/workers from /pkg/monaco/ or similar path.
      Ensure the monaco worker files (editor.worker, json.worker, etc.) are served.
      Add a <script> tag or dynamic import in the app shell.

  - id: T-023
    title: "Monaco Leptos wrapper component via wasm-bindgen"
    priority: 2
    status: done
    notes: >
      Create a MonacoEditor Leptos component that:
      (1) Renders a container div with a node_ref,
      (2) On mount (Effect), calls monaco.editor.create() via JS interop,
      (3) Accepts props: initial_value, language (rust/toml), on_change callback,
      (4) Exposes get_value() and set_value() methods via signals or callbacks,
      (5) Handles cleanup on unmount (editor.dispose()).
      Use wasm-bindgen + js-sys for the JS interop.
      Handle SSR gracefully (Monaco only initializes client-side).

  - id: T-024
    title: "Monaco language configuration for Rust and TOML"
    priority: 3
    status: todo
    notes: >
      Configure Monaco with Rust syntax highlighting (monarch grammar).
      Configure TOML syntax highlighting.
      Set theme to match ironpad dark theme (vs-dark base, customized).
      Configure basic editor options: minimap off, line numbers on,
      automatic layout, word wrap, font size.

  # ── Phase 6: UI — Cell Component ───────────────────────────────────────
  - id: T-025
    title: "Cell card component with tab bar"
    priority: 2
    status: done
    notes: >
      Thaw Card wrapping a cell. Tab bar with "Code" and "Cargo.toml" tabs
      (Thaw Tabs). Each tab shows a Monaco editor instance.
      Cell header shows: cell label, status indicator, run button, menu button.
      Cell is collapsible (Thaw Collapse or custom).
      Props: cell_id, notebook_id, initial_source, initial_cargo_toml, order.

  - id: T-026
    title: "Cell code editor (Monaco for source.rs)"
    priority: 2
    status: done
    notes: >
      Monaco instance in the "Code" tab with language="rust".
      On change, debounce and call update_cell_source server fn.
      Track dirty state (unsaved changes indicator).
      Expose current source value for the compile flow.

  - id: T-027
    title: "Cell Cargo.toml editor (Monaco for Cargo.toml)"
    priority: 2
    status: done

  - id: T-028
    title: "Cell run button with compile and execute flow"
    priority: 2
    status: done
    notes: >
      Run button (▶) on cell header. On click:
      (1) Set status to "compiling", (2) Call compile_cell server fn with current
      source + cargo_toml, (3) On success with blob: set status to "running",
      execute WASM via executor (T-036), capture output, display in output panel,
      set status to "success" with timing. (4) On compile failure: set status to
      "error", render diagnostics in error panel.
      Also bind Shift+Enter to this flow.

  - id: T-029
    title: "Cell status indicator"
    priority: 3
    status: todo
    notes: >
      Visual indicator on cell header showing current state:
      idle (gray), compiling (yellow/spinner), running (blue/spinner),
      success (green checkmark + timing), error (red X).
      Use Thaw Tag or Badge component with appropriate colors.

  - id: T-030
    title: "Cell output panel"
    priority: 2
    status: done
    notes: >
      Below the editor area, show cell execution output.
      For MVP: display text (from CellResult display field),
      and raw bytes as hex dump (from CellResult output bytes) with byte count.
      Show execution timing. Collapsible panel.
      Panel is hidden when cell has no output yet.

  - id: T-031
    title: "Cell error panel with formatted compiler diagnostics"
    priority: 2
    status: done
    notes: >
      When compilation fails, render diagnostics below the editor.
      Show error messages with severity (error/warning/note) color-coded.
      Show relevant source spans with line numbers.
      Link to Rust error index (https://doc.rust-lang.org/error_codes/{code}.html)
      where error codes are available.
      Replaces the output panel when there are errors.

  - id: T-032
    title: "Cell menu (delete, move up/down, duplicate)"
    priority: 3
    status: todo
    notes: >
      ⋯ button on cell header opens a dropdown menu (Thaw Dropdown / Popover).
      Options: Delete (confirm dialog), Move Up, Move Down, Duplicate, Rename label.
      Each action calls the corresponding server fn and updates local state.
      Move Up/Down disabled at boundaries.

  - id: T-033
    title: "Add Cell button between cells and at bottom"
    priority: 2
    status: done
    notes: >
      "+" button rendered between every pair of cells and at the bottom.
      On click, calls add_cell server fn (inserts after the preceding cell).
      New cell gets a default label ("Cell N"), empty source, default Cargo.toml.
      Scroll to and focus the new cell's editor.

  # ── Phase 7: Error Rendering (Advanced) ────────────────────────────────
  - id: T-034
    title: "Error span mapping (wrapper offset adjustment)"
    priority: 2
    status: done
    notes: >
      The server wraps user code in a cell_main function (see MegaPrd §5.2).
      When rustc reports error spans, line numbers are relative to the wrapped
      file. Subtract the wrapper offset (number of preamble lines) to get
      line numbers relative to user code. Also handle column offsets.
      This logic lives in the diagnostic parser (T-011) but deserves focused
      testing with various error types: syntax errors, type errors, borrow errors.

  - id: T-035
    title: "Monaco inline error markers via setModelMarkers"
    priority: 2
    status: done
    notes: >
      After compilation, if there are diagnostics with spans, call
      monaco.editor.setModelMarkers on the cell's Monaco model.
      Map Diagnostic severity to MarkerSeverity (Error, Warning, Hint, Info).
      Clear markers before each compile. Show inline decorations for errors.
      This requires extending the Monaco wasm-bindgen bindings (T-023).

  # ── Phase 8: Client-Side WASM Execution ────────────────────────────────
  - id: T-036
    title: "WASM executor JS module"
    priority: 2
    status: todo
    notes: >
      JavaScript module (served with the app) implementing the CellExecutor
      class from MegaPrd §7.5. Key methods:
      loadBlob(cellId, hash, wasmBytes) — instantiate WASM module with imports,
      execute(cellId, inputBytes) — call cell_main, read CellResult from memory,
      return { outputBytes, displayText }.
      Provide ironpad_alloc/ironpad_dealloc as imports.
      Handle errors gracefully (WASM traps, OOM).

  - id: T-037
    title: "wasm-bindgen bindings for the WASM executor"
    priority: 2
    status: todo
    notes: >
      Rust-side wasm-bindgen bindings to call the CellExecutor JS module
      from Leptos components. Functions: init_executor(), load_blob(cell_id, hash, bytes),
      execute_cell(cell_id, input_bytes) -> (output_bytes, display_text).
      These are called from the cell run flow (T-028).

  - id: T-038
    title: "Cell I/O pipeline (output → next cell input)"
    priority: 2
    status: todo
    notes: >
      Notebook-level state tracking each cell's output bytes.
      When cell N executes, its output is stored and made available as
      cell N+1's input. Cell 0 always receives empty input.
      When a cell re-executes, all downstream cells' outputs are invalidated.
      The output bytes are stored in a reactive signal map: Map<cell_id, Vec<u8>>.

  - id: T-039
    title: "Run All Below (sequential cell execution)"
    priority: 3
    status: todo
    notes: >
      From a given cell, execute it and then sequentially execute all cells
      below it, piping outputs forward. Stop on first compile/execution error.
      Triggered from cell menu or Ctrl+Shift+Enter (from top).
      Show progress as each cell compiles and executes.

  # ── Phase 9: Notebook Features ─────────────────────────────────────────
  - id: T-040
    title: "Save notebook (Ctrl+S and save button)"
    priority: 2
    status: todo
    notes: >
      Save button in header. Ctrl+S keyboard shortcut.
      Collects current state of all cells (source, cargo_toml, order, labels)
      and calls update server fns. Updates notebook updated_at timestamp.
      Visual feedback: brief "Saved" indicator or save button state change.
      Note: individual cell edits are already debounce-saved (T-026, T-027),
      but this ensures manifest consistency.

  - id: T-041
    title: "Notebook title editing"
    priority: 3
    status: todo
    notes: >
      In the notebook editor header, notebook title is displayed and
      click-to-edit (inline editable text). On blur or Enter, calls
      update_notebook server fn with new title.
      Use Thaw Input or custom inline-edit component.

  - id: T-042
    title: "Keyboard shortcuts"
    priority: 3
    status: todo
    notes: >
      Global keyboard listener (on the notebook editor page):
      Shift+Enter — run current (focused) cell,
      Ctrl+Shift+Enter — run all cells from top,
      Ctrl+S — save notebook,
      Ctrl+Shift+N — add new cell below current.
      Avoid conflicts with Monaco's own shortcuts.
      Use web-sys KeyboardEvent listener.

  # ── Phase 10: Styling & Polish ─────────────────────────────────────────
  - id: T-043
    title: "Dark theme CSS and overall styling"
    priority: 3
    status: todo
    notes: >
      Thaw ThemeProvider with dark mode as default.
      Custom CSS for: notebook layout spacing, cell card styling,
      output/error panel styling, status bar, typography.
      Monaco theme matching the app theme (vs-dark customized).
      Tailwind or vanilla CSS (match Thaw's approach).
      Responsive but desktop-first (notebooks are a desktop use case).

  - id: T-044
    title: "Status bar implementation"
    priority: 3
    status: todo
    notes: >
      Footer bar showing: compiler version ("stable"),
      cell count ("Cells: N"), last save time ("Saved: 2m ago").
      Use Thaw Layout footer. Reactive to notebook state changes.

  # ── Phase 11: Testing Infrastructure ───────────────────────────────────
  - id: T-045
    title: "Playwright setup and configuration"
    priority: 2
    status: todo
    notes: >
      Initialize Playwright in the repo (npx playwright init or manual setup).
      Configure: base URL http://localhost:3000, browsers (chromium only for CI speed).
      Add playwright.config.ts with webServer config that runs `cargo leptos serve`
      (or the built binary) before tests. Add .gitignore for test-results/.
      Ensure `cargo make playwright-install` installs browsers.
      Ensure `cargo make playwright` runs the test suite.
      The agent running on this PRD should be able to run playwright to check work.

  - id: T-046
    title: "Playwright smoke test — home page loads"
    priority: 2
    status: todo
    notes: >
      Test: navigate to /, verify "ironpad" title/branding is visible,
      verify the page renders without JS errors.
      This is the most basic sanity check.

  - id: T-047
    title: "Playwright smoke test — create notebook and add cell"
    priority: 2
    status: todo
    notes: >
      Test: click "New Notebook", verify redirected to /notebook/{id},
      verify a cell editor is visible, click "Add Cell", verify two cells exist.

  - id: T-048
    title: "Playwright smoke test — compile and execute a trivial cell"
    priority: 2
    status: todo
    notes: >
      Test: create notebook, type trivial Rust code in cell 0
      (e.g., CellOutput::text("hello")), click Run, wait for compilation,
      verify output panel shows "hello". Requires the full compile pipeline
      to be working (Rust toolchain available in test env).

  - id: T-049
    title: "Playwright smoke test — two-cell data flow"
    priority: 3
    status: todo
    notes: >
      Test: create notebook with two cells. Cell 0 serializes a Vec<i32>,
      Cell 1 deserializes and sums it. Run both, verify Cell 1 output
      shows the expected sum. This validates the full bincode pipeline.

  - id: T-050
    title: "Playwright smoke test — save and reload notebook"
    priority: 3
    status: todo
    notes: >
      Test: create notebook, add a cell with code, save (Ctrl+S or button),
      navigate away to home, navigate back to the notebook,
      verify cell source code is preserved.

  - id: T-051
    title: "Unit tests for compilation pipeline"
    priority: 2
    status: todo
    notes: >
      Tests in ironpad-server:
      - Micro-crate scaffolding generates valid Cargo.toml and wrapped lib.rs.
      - Content hash is deterministic for same input.
      - Content hash changes when source/cargo_toml changes.
      - Diagnostic parser correctly extracts errors from sample rustc JSON.
      - Wrapper offset calculation is correct.
      Does NOT require actually invoking cargo build (mock the command).

  - id: T-052
    title: "Unit tests for notebook persistence"
    priority: 2
    status: todo
    notes: >
      Tests in ironpad-server (using tempdir):
      - Create notebook → directory structure is correct.
      - Add cell → cell directory with source.rs and Cargo.toml created.
      - Update cell source → file updated on disk.
      - Delete cell → directory removed, manifest updated.
      - List notebooks → returns correct summaries.
      - Reorder cells → manifest reflects new order.

  - id: T-053
    title: "Integration test — compile and execute E2E"
    priority: 3
    status: todo
    notes: >
      Test that requires the Rust toolchain (wasm32-unknown-unknown target).
      Scaffolds a micro-crate, compiles it, verifies the .wasm blob is produced.
      Optionally execute in a headless WASM runtime (wasmtime) to verify output.
      This test may be slow — mark with #[ignore] and run only in CI/UAT.

  # ── Phase 12: Docker Verification ──────────────────────────────────────
  - id: T-054
    title: "Docker build and run verification"
    priority: 3
    status: todo
    notes: >
      Verify: `cargo make docker-build` succeeds.
      `cargo make docker-up` starts the container.
      Port 3000 is accessible. Home page loads.
      Volume mounts work (create notebook, restart container, notebook persists).
      `cargo make docker-down` stops cleanly.

  - id: T-055
    title: "Loading states and final polish"
    priority: 3
    status: todo
    notes: >
      Add loading spinners/skeletons for: notebook list loading,
      notebook loading, cell compilation in progress.
      Add error boundaries for server fn failures.
      Add toast/notification for save confirmation.
      Final pass on spacing, alignment, and visual consistency.
---

# Summary

First complete implementation pass for ironpad — a Leptos-based interactive Rust notebook.
This PRD covers the full MVP scope from MegaPrd.md: workspace scaffolding, compilation pipeline,
notebook persistence, cell UI with Monaco editors, client-side WASM execution, error rendering,
Docker deployment, and Playwright smoke tests. See MegaPrd.md for all architectural details.

# Problem

There is no compelling, Rust-native interactive notebook experience. Existing options
(Rust Playground, Jupyter + evcxr, WASM playgrounds) all fall short on cell isolation,
dependency management, compilation feedback, and UI polish. See MegaPrd.md §1 for full problem statement.

# Goals

1. Scaffold the full ironpad workspace (Leptos 0.8 + Axum 0.8 + Thaw 0.5.0-beta)
2. Implement server-side compilation pipeline (micro-crate → cargo build → WASM blob → cache)
3. Implement notebook persistence to filesystem (directory-based, JSON manifest)
4. Build cell UI with Monaco editors (code + Cargo.toml tabs), run button, output/error panels
5. Implement client-side WASM execution with cell-to-cell data flow via bincode
6. Render compiler errors inline in Monaco with span highlighting
7. Package in Docker with volume mounts for notebooks and cache
8. Add Playwright smoke tests covering the core user flow
9. Wire up cargo-make tasks so agents can build, test, and verify via `cargo make uat`
10. Ensure the binary is runnable outside Docker with --data-dir and --cache-dir flags

# Technical Approach

## Tech Stack

| Layer | Technology | Version |
|-------|-----------|---------|
| Framework | Leptos | 0.8.x |
| HTTP Server | Axum | 0.8.x |
| UI Components | Thaw UI | 0.5.0-beta |
| Code Editor | Monaco | Latest (bundled) |
| Serialization | bincode | 2.x |
| Content Hash | blake3 | Latest |
| CLI Args | clap | 4.x |
| Build Tool | cargo-leptos | Latest |
| Task Runner | cargo-make | Latest |
| E2E Testing | Playwright | Latest |
| Container | Docker | Latest |

## Workspace Structure

```
ironpad/
  Cargo.toml                    # workspace root
  Makefile.toml                 # cargo-make tasks
  playwright.config.ts          # Playwright config
  tests/e2e/                    # Playwright smoke tests
  docker/
    Dockerfile
    docker-compose.yml
  crates/
    ironpad-app/                # Leptos components (SSR + hydrate)
      src/
        lib.rs                  # root App, routing
        pages/
          home.rs               # notebook list page
          editor.rs             # notebook editor page
        components/
          cell.rs               # cell card component
          monaco.rs             # Monaco wrapper
          output.rs             # output panel
          error_panel.rs        # error display
    ironpad-frontend/           # WASM hydration entry point
      src/
        main.rs                 # hydrate(App)
    ironpad-server/             # Axum binary + compile service
      src/
        main.rs                 # server entry, CLI args
        config.rs               # AppConfig
        compile/
          mod.rs
          scaffold.rs           # micro-crate writer
          build.rs              # cargo invocation
          cache.rs              # blake3 cache
          diagnostics.rs        # rustc JSON parser
          optimize.rs           # wasm-opt
        notebook/
          mod.rs
          storage.rs            # filesystem CRUD
          sample.rs             # sample notebook generator
    ironpad-cell/               # injected into user cells
      src/
        lib.rs                  # CellInput/Output/Result, FFI
    ironpad-common/             # shared types
      src/
        lib.rs                  # CompileRequest/Response, Notebook types
  assets/
    monaco/                     # bundled Monaco editor files
  data/                         # default notebook storage (gitignored)
  cache/                        # default compilation cache (gitignored)
```

## Architecture Diagram

```
┌──────────────────────────────────────────────────────────────────┐
│                    Browser (Leptos Hydrated)                      │
│                                                                   │
│  ironpad-app (WASM)           JS Modules                          │
│  ┌─────────────────────┐     ┌─────────────────────────┐         │
│  │ NotebookEditor page │     │ Monaco Editor instances  │         │
│  │  ├─ CellComponent   │◄───►│  (Rust + TOML modes)    │         │
│  │  │   ├─ CodeTab     │     └─────────────────────────┘         │
│  │  │   ├─ TomlTab     │     ┌─────────────────────────┐         │
│  │  │   ├─ OutputPanel │◄────│ CellExecutor (JS)       │         │
│  │  │   ├─ ErrorPanel  │     │  ├─ loadBlob()          │         │
│  │  │   └─ RunButton ──┼────►│  ├─ execute(cellId, in) │         │
│  │  └─ AddCellButton   │     │  └─ cell I/O pipeline   │         │
│  └──────────┬──────────┘     └─────────────────────────┘         │
│             │ #[server] fn                                        │
└─────────────┼────────────────────────────────────────────────────┘
              │
              ▼
┌──────────────────────────────────────────────────────────────────┐
│                    Server (Axum + Leptos SSR)                      │
│                                                                   │
│  ironpad-server                                                   │
│  ┌──────────────────────────────────────────────────────────────┐ │
│  │ compile_cell(CompileRequest) → CompileResponse               │ │
│  │  ├─ blake3 hash → cache check                                │ │
│  │  ├─ scaffold micro-crate (inject ironpad-cell path dep)      │ │
│  │  ├─ cargo build --target wasm32-unknown-unknown --release    │ │
│  │  ├─ parse rustc JSON diagnostics (map spans to user code)    │ │
│  │  ├─ wasm-opt (best-effort)                                   │ │
│  │  └─ cache .wasm blob                                         │ │
│  └──────────────────────────────────────────────────────────────┘ │
│  ┌──────────────────────────────────────────────────────────────┐ │
│  │ Notebook CRUD (filesystem)                                   │ │
│  │  {data_dir}/notebooks/{id}/ironpad.json + cells/             │ │
│  └──────────────────────────────────────────────────────────────┘ │
│                                                                   │
│  AppConfig: --data-dir --cache-dir --port --ironpad-cell-path     │
└──────────────────────────────────────────────────────────────────┘
```

## Key Design Decisions

1. **Leptos 0.8 workspace pattern**: Three crates (app/frontend/server) following start-axum-workspace.
   This separates concerns and allows cargo-leptos to handle SSR build + WASM hydration.

2. **Monaco bundled**: Served from the ironpad server's static assets. No CDN dependency.
   Integrated via wasm-bindgen JS interop (no Rust crate for Monaco exists).

3. **Config flexibility**: `--data-dir` and `--cache-dir` CLI flags (+ env vars) allow
   running outside Docker for development and agent testing. Defaults to `./data` and `./cache`.

4. **Playwright for agent verification**: The `cargo make playwright` task starts the server
   and runs smoke tests. Agents looping on this PRD can run `cargo make uat` to check their work.

5. **ironpad-cell as path dep**: The server injects `ironpad-cell = { path = "/path/to/crate" }`
   into each cell's Cargo.toml. The path is configurable via `--ironpad-cell-path`.

# Assumptions

- Rust stable toolchain with `wasm32-unknown-unknown` target is available in the build environment
- Node.js/npm is available for Playwright and Monaco asset management
- cargo-leptos and cargo-make are installable via cargo
- The development machine has enough resources to compile WASM targets
- Leptos 0.8.x and Thaw 0.5.0-beta APIs are stable enough for production use

# Constraints

- Monaco must be loaded client-side only (not during SSR) — requires careful hydration handling
- WASM cell execution is browser-only — no server-side execution in MVP
- Single-user only — no auth, no multi-tenant, no collaboration
- Linear cell order only — no DAG
- `wasm32-unknown-unknown` compatible crates only (no system deps, no WASI for MVP)
- Compile timeout hard-capped at 30 seconds

# References to Code

This is a greenfield project — no existing code. See MegaPrd.md for all architecture details:
- §5: Architecture (cell model, data flow, compilation pipeline, WASM execution, persistence)
- §6: UI/UX Design (layout, components, Monaco config, error rendering)
- §7: Technical Design (tech stack, crate structure, ironpad-cell, compilation service, executor)
- §8: Notebook File Format (directory structure, manifest schema, default Cargo.toml)
- §9: Deployment (Dockerfile, docker-compose, first-run experience)
- §10: Performance Budget

# Non-Goals (MVP)

1. Server-side execution toggle (post-MVP Phase 2)
2. Multi-user / collaboration / auth
3. DAG-based cell ordering
4. Multiple compiler versions (stable only)
5. `ironpad-std` charting/visualization library
6. Custom system dependencies (apt-get)
7. Multi-tenant SaaS deployment
8. Auto-save (explicit save only)
9. Single-file `.ironpad` export format
10. LSP / rust-analyzer integration in Monaco
11. Light theme toggle (dark only for MVP, but Thaw supports it for later)

# History

## 2026-03-06 — T-001 Completed
- **Task**: Initialize Cargo workspace with all crate stubs
- **Status**: ✅ Done
- **Changes**:
  - Created root `Cargo.toml` with workspace definition (5 member crates) and `[workspace.dependencies]` for all shared deps
  - Created `crates/ironpad-common/` — shared types crate (stub, deps: serde, chrono, uuid)
  - Created `crates/ironpad-cell/` — cell injection crate (stub, deps: serde, bincode)
  - Created `crates/ironpad-app/` — Leptos app crate with `ssr`/`hydrate` features, minimal App + HomePage components, shell function
  - Created `crates/ironpad-frontend/` — WASM hydration entry point (cdylib+rlib), calls `hydrate_body(App)`
  - Created `crates/ironpad-server/` — Axum binary with tracing, Leptos SSR routing, all server-side deps
  - Created `style/main.scss` — minimal dark-theme placeholder stylesheet
  - Created `.gitignore` for target/, data/, cache/, node_modules/, etc.
  - Added `[[workspace.metadata.leptos]]` section for cargo-leptos integration
  - Follows `leptos-rs/start-axum-workspace` template pattern for Leptos 0.8
  - Dependency versions: leptos 0.8, axum 0.8, thaw 0.5.0-beta, bincode 2, blake3, clap 4, tracing, etc.
  - `cargo build` ✅ succeeds (all 5 crates compile cleanly)
  - `cargo check` ✅ passes with no warnings
  - `cargo make uat` — not yet available (Makefile.toml with `uat` task is T-003)
- **Constitution Compliance**: No violations.

## 2026-03-06 — T-002 Completed
- **Task**: Configure cargo-leptos metadata and features
- **Status**: ✅ Done
- **Changes**:
  - Fixed `uuid` workspace dependency: added `js` feature required for `wasm32-unknown-unknown` target (enables `crypto.getRandomValues()` RNG in browser)
  - Fixed `ironpad-app` workspace dependency: added `default-features = false` to resolve cargo warning about `default-features` being ignored in consumer crates
  - Removed unrecognized `env` and `watch` metadata keys from `[[workspace.metadata.leptos]]` (deprecated in cargo-leptos 0.3.5)
  - Made `thaw` a regular (non-optional) dependency in `ironpad-app` — required for both SSR and hydrate modes so Thaw UI components can properly hydrate client-side
  - Removed `dep:thaw` from `ironpad-app`'s `ssr` feature (no longer optional)
  - Installed `wasm-bindgen-cli` 0.2.114 to match the project's `wasm-bindgen` dependency version
  - `cargo leptos build` ✅ succeeds (both server binary and WASM frontend compile cleanly)
  - `cargo build` ✅ still succeeds
  - `cargo make uat` — not yet available (Makefile.toml with `uat` task is T-003)
- **Opportunistic UAT**: No UATs could be verified yet — uat-001 requires a running server (depends on routing/pages from later tasks), and Playwright infrastructure is not yet set up (T-045).
- **Constitution Compliance**: No violations.

## 2026-03-06 — T-003 Completed
- **Task**: Create Makefile.toml with all dev/CI/UAT tasks
- **Status**: ✅ Done
- **Changes**:
  - Created `Makefile.toml` at workspace root with `[config] default_to_workspace = false` to run tasks at workspace level only
  - Tasks implemented: `install-tools`, `dev`, `build`, `fmt`, `fmt-check`, `clippy`, `test`, `ci`, `uat`, `playwright`, `playwright-install`, `docker-build`, `docker-up`, `docker-down`, `docker-uat`
  - `ci` = `fmt-check` → `clippy` → `test` (dependency chain)
  - `uat` = `ci` → `playwright` (the one true gate)
  - `test` uses `cargo nextest run --no-tests=pass` to handle zero-test state gracefully
  - `playwright` task gracefully skips when no `playwright.config.ts` or test files exist (ready for T-045)
  - Docker tasks (`docker-build`, `docker-up`, `docker-down`, `docker-uat`) gracefully skip when no Dockerfile exists (ready for T-004)
  - Fixed pre-existing formatting issue in `crates/ironpad-server/src/main.rs` (discovered by `cargo fmt --check`)
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, test ✅ (0 tests, pass), playwright skipped)
- **Opportunistic UAT**: No UATs verifiable yet — all require server running or Playwright infrastructure.
- **Constitution Compliance**: No violations. Fixed a pre-existing fmt issue as it was directly blocking the `fmt-check` task in the CI pipeline (Root Cause Resolution principle).

## 2026-03-06 — T-005 Completed
- **Task**: CLI argument parsing and configuration module
- **Status**: ✅ Done
- **Changes**:
  - Created `crates/ironpad-common/src/config.rs` — `AppConfig` struct (data_dir, cache_dir, port, ironpad_cell_path) in the shared crate so server functions in ironpad-app can access it via `expect_context::<AppConfig>()`
  - Updated `crates/ironpad-common/src/lib.rs` — added `pub mod config` and re-export of `AppConfig`
  - Created `crates/ironpad-server/src/config.rs` — `CliArgs` clap parser with `#[arg(env = "...")]` for each flag, plus `From<CliArgs> for AppConfig` conversion and 3 unit tests
  - Updated `crates/ironpad-server/src/main.rs` — parses CLI args, creates AppConfig, provides it via `leptos_routes_with_context`, overrides listen address with `--port` value
  - CLI flags: `--data-dir` (default: `./data`, env: `IRONPAD_DATA_DIR`), `--cache-dir` (default: `./cache`, env: `IRONPAD_CACHE_DIR`), `--port` (default: 3000, env: `IRONPAD_PORT`), `--ironpad-cell-path` (default: `./crates/ironpad-cell`, env: `IRONPAD_CELL_PATH`)
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, test ✅ (3 tests pass), playwright skipped)
- **Opportunistic UAT**: uat-010 ("Binary accepts --data-dir and --cache-dir flags and uses them") is partially verifiable — the 3 unit tests confirm flag parsing and defaults. Full verification (files written to specified dirs) requires notebook persistence (T-014+).
- **Constitution Compliance**: No violations.

## 2026-03-06 — T-006 Completed
- **Task**: Define shared types in ironpad-common
- **Status**: ✅ Done
- **Changes**:
  - Created `crates/ironpad-common/src/types.rs` — all shared types for compilation pipeline and notebook persistence
  - Types added: `CompileRequest`, `CompileResponse`, `Diagnostic`, `Severity` (enum), `Span`, `NotebookManifest`, `CellManifest`, `NotebookSummary`
  - All types derive `Serialize`, `Deserialize`, `Clone`, `Debug`; `Severity` also derives `PartialEq`, `Eq`
  - `NotebookManifest` uses `Uuid` for id and `DateTime<Utc>` for timestamps (matching MegaPrd §8.2 manifest schema)
  - `Span.label` is `Option<String>` (not all spans have labels)
  - Updated `crates/ironpad-common/src/lib.rs` — added `pub mod types` and glob re-export `pub use types::*`
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, test ✅ (3 tests pass), playwright skipped)
- **Opportunistic UAT**: No UATs verifiable yet — all require running server or Playwright infrastructure.
- **Constitution Compliance**: No violations.

## 2026-03-06 — T-007 Completed
- **Task**: Implement ironpad-cell crate
- **Status**: ✅ Done
- **Changes**:
  - Implemented `crates/ironpad-cell/src/lib.rs` — full ironpad-cell API per MegaPrd §7.3
  - `CellInput<'a>` — wraps `&[u8]` with `new()`, `deserialize::<T>()`, `is_empty()`, `raw()`
  - `CellOutput` — serializes via bincode with `new::<T>()`, `with_display()`, `empty()`, `text()`
  - `CellResult` — `#[repr(C)]` FFI struct (output_ptr, output_len, display_ptr, display_len) with `From<CellOutput>` conversion
  - `ironpad_alloc(len) -> *mut u8` — WASM memory allocator (`#[no_mangle] extern "C"`)
  - `ironpad_dealloc(ptr, len)` — WASM memory deallocator (`#[no_mangle] unsafe extern "C"`)
  - `prelude` module — re-exports `bincode`, `serde::{Serialize, Deserialize}`, `CellInput`, `CellOutput`, `CellResult`
  - Updated `crates/ironpad-cell/Cargo.toml` — enabled bincode `serde` feature for serde compat layer (bincode 2 requires opt-in)
  - Added 11 unit tests: round-trip struct/vec serialization, CellInput helpers, CellOutput constructors, CellResult repr(C) layout, FFI alloc/dealloc smoke tests
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, test ✅ (14 tests pass), playwright skipped)
- **Opportunistic UAT**: No UATs verifiable yet — all require running server or Playwright infrastructure.
- **Constitution Compliance**: No violations. bincode 2 uses a different API than the pseudocode in MegaPrd §7.3 (which used bincode 1 style `serialize`/`deserialize`); adapted to use `bincode::serde::encode_to_vec` / `bincode::serde::decode_from_slice` with `bincode::config::standard()` while preserving identical semantics.

## 2026-03-06 — T-004 Completed
- **Task**: Create Dockerfile and docker-compose.yml
- **Status**: ✅ Done
- **Changes**:
  - Created `docker/Dockerfile` — multi-stage build: builder stage (rust:latest + wasm32-unknown-unknown + cargo-leptos + binaryen) compiles the app with `cargo leptos build --release`; runtime stage (rust:latest + wasm32-unknown-unknown + binaryen) copies the binary, site assets, and ironpad-cell crate. Runtime retains Rust toolchain for compiling user cells.
  - Created `docker/docker-compose.yml` — single `ironpad` service with port 3000 exposed, named volumes for `/data` and `/cache`, env vars for `IRONPAD_DATA_DIR`, `IRONPAD_CACHE_DIR`, `IRONPAD_PORT`, `IRONPAD_CELL_PATH`, and `RUST_LOG`.
  - Created `docker/warmup-Cargo.toml` — warmup crate used during Docker build to pre-download ironpad-cell transitive dependencies into the cargo registry cache, so first user cell compile skips dep download.
  - Created `.dockerignore` — excludes `target/`, `data/`, `cache/`, `node_modules/`, `.git/`, `.mr/`, test artifacts, and IDE files from the Docker build context.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, test ✅ (14 tests pass), playwright skipped)
- **Opportunistic UAT**: uat-008 ("Docker container builds and serves the app") cannot be verified yet — requires `docker` to be available and the full app (routing, pages) to be implemented. The Dockerfile itself is syntactically correct and follows MegaPrd §9 patterns.
- **Constitution Compliance**: No violations. The Dockerfile deviates slightly from MegaPrd §9.1 (uses multi-stage instead of single-stage, does not include sccache) because the task notes explicitly require multi-stage and sccache is an optimization that can be added later. The docker-compose.yml uses modern format (no `version` key) per compose spec v2.

## 2026-03-06 — T-008 Completed
- **Task**: Micro-crate scaffolding service
- **Status**: ✅ Done
- **Changes**:
  - Created `crates/ironpad-app/src/compiler/mod.rs` — compiler pipeline module (ssr-gated).
  - Created `crates/ironpad-app/src/compiler/scaffold.rs` — micro-crate scaffolding service with:
    - `scaffold_micro_crate()` function that writes a compilable micro-crate to `{cache_dir}/workspaces/{session_id}/{cell_id}/`.
    - `generate_cargo_toml()` — builds Cargo.toml with `crate-type = ["cdylib"]`, injects `ironpad-cell` as an absolute path dependency, and merges user-specified dependencies.
    - `generate_lib_rs()` — wraps user source code in the `cell_main` FFI function per MegaPrd §5.2.
    - `extract_user_dependencies()` — extracts `[dependencies]` entries from user's Cargo.toml, filtering out any user-specified ironpad-cell (we inject our own).
    - `WRAPPER_PREAMBLE_LINES` constant (= 4) exported for T-011 diagnostic line-number adjustment.
    - 12 unit/integration tests covering dependency extraction, Cargo.toml generation, lib.rs wrapping, preamble line count, full scaffold integration, and overwrite behavior.
  - Updated `crates/ironpad-app/src/lib.rs` — added `#[cfg(feature = "ssr")] pub mod compiler;`.
  - Updated `crates/ironpad-app/Cargo.toml` — added `anyhow` and `uuid` as optional ssr-gated dependencies.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 26 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs can be verified at this stage — all depend on the full compilation pipeline and UI being functional.
- **Constitution Compliance**: No violations.

## 2026-03-06 — T-009 Completed
- **Task**: Cargo build invocation for WASM target
- **Status**: ✅ Done
- **Changes**:
  - Created `crates/ironpad-app/src/compiler/build.rs` — async `build_micro_crate()` function that:
    - Invokes `cargo build --target wasm32-unknown-unknown --release --message-format=json` in the scaffolded micro-crate directory.
    - Sets `CARGO_HOME` to `{cache_dir}/cargo-home/` for shared registry caching.
    - Sets `CARGO_TARGET_DIR` to `{cache_dir}/targets/{session_id}/` for per-session incremental reuse.
    - Enforces a 30-second hard timeout via `tokio::time::timeout`.
    - Returns `BuildResult::Success` with the `.wasm` blob path, or `BuildResult::Failure` with raw stdout/stderr.
    - Exports `cargo_home_dir()`, `target_dir()`, and `expected_wasm_path()` helpers for downstream consumers.
    - 7 unit tests covering path computation for cargo home, target dirs, and WASM artifact paths with various cell ID formats.
  - Updated `crates/ironpad-app/src/compiler/mod.rs` — registered `build` module.
  - Updated `crates/ironpad-app/Cargo.toml` — added `tokio` and `tracing` as optional SSR-gated dependencies.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 33 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs can be verified at this stage — all depend on the full compilation pipeline and UI being functional.
- **Constitution Compliance**: No violations.

## 2026-03-06 — T-010 Completed
- **Task**: Blake3 content-hash caching for compiled WASM blobs
- **Status**: ✅ Done
- **Changes**:
  - Created `crates/ironpad-app/src/compiler/cache.rs` — blake3 content-hash caching module with:
    - `content_hash(source, cargo_toml)` — deterministic blake3 hash of `source || cargo_toml || "wasm32-unknown-unknown"` (64-char hex digest).
    - `cache_blob_path(cache_dir, hash)` — returns `{cache_dir}/blobs/{hash}.wasm`.
    - `try_cache_hit(cache_dir, hash)` — reads cached blob on hit, returns `None` on miss (filesystem errors treated as misses with warn-level logging).
    - `store_blob(cache_dir, hash, wasm_bytes)` — writes blob to cache, creating `blobs/` directory if needed.
    - Cache stats logging via `tracing` (info for hit/miss/store, warn for read errors).
    - 9 unit tests: hash determinism, hash sensitivity to source/cargo_toml changes, hex format validation, path layout, miss on empty dir, store-and-hit round trip, directory auto-creation, real-hash round trip.
  - Updated `crates/ironpad-app/src/compiler/mod.rs` — registered `cache` module.
  - Updated `crates/ironpad-app/Cargo.toml` — added `blake3` as optional SSR-gated dependency, added `tempfile` as dev-dependency for cache tests.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 42 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs can be verified at this stage — all depend on the full compilation pipeline and UI being functional.
- **Constitution Compliance**: No violations.

## 2026-03-06 — T-011 Completed
- **Task**: Rustc JSON diagnostic parser
- **Status**: ✅ Done
- **Changes**:
  - Added `serde_json = "1"` to workspace dependencies in root `Cargo.toml`.
  - Added `serde` and `serde_json` as optional SSR-gated dependencies to `crates/ironpad-app/Cargo.toml`.
  - Created `crates/ironpad-app/src/compiler/diagnostics.rs` — rustc JSON diagnostic parser with:
    - Intermediate serde structs (`CargoMessage`, `RustcMessage`, `RustcCode`, `RustcSpan`) matching rustc's `--message-format=json` schema.
    - `parse_diagnostics(cargo_stdout)` — parses multi-line cargo JSON output, filters for `"compiler-message"` entries, maps to `ironpad_common::Diagnostic` types.
    - Level mapping: `error` → `Severity::Error`, `warning` → `Severity::Warning`, `note`/`help` → `Severity::Note`.
    - Error code appended to message text when present (e.g., `"mismatched types [E0308]"`).
    - Span filtering: only primary spans from `src/lib.rs` (the wrapped user code file) are included.
    - `adjust_span()` — subtracts `WRAPPER_PREAMBLE_LINES` (4) from line numbers, filtering out spans that fall within the auto-generated wrapper preamble.
    - 13 unit tests: type error parsing, warning parsing, artifact line skipping, note-level handling, preamble span filtering, dependency file span filtering, primary-only span filtering, multiline cargo output, empty input, malformed JSON, multiline span adjustment, preamble boundary edge case, first user line edge case.
  - Updated `crates/ironpad-app/src/compiler/mod.rs` — registered `diagnostics` module.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 55 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs can be verified at this stage — all depend on the full compilation pipeline and UI being functional.
- **Constitution Compliance**: No violations.

## 2026-03-06 — T-012 & T-013 Completed
- **Task**: WASM optimization (wasm-opt, best-effort) + compile_cell server function
- **Status**: ✅ Done
- **Changes**:
  - Created `crates/ironpad-app/src/compiler/optimize.rs` — best-effort wasm-opt optimization module (T-012):
    - `optimize_wasm(wasm_bytes, work_dir)` — writes bytes to temp file, runs `wasm-opt -Oz`, returns optimized bytes on success or original bytes if wasm-opt is unavailable or fails.
    - Logs optimization stats (original/optimized sizes, bytes saved) at info level, skips silently at debug level.
    - 1 unit test: verifies fallback to original bytes when wasm-opt is missing or input is invalid.
  - Created `crates/ironpad-app/src/server_fns.rs` — Leptos `#[server]` fn `compile_cell` (T-013):
    - Receives `CompileRequest` (cell_id, source, cargo_toml).
    - Gets `AppConfig` via `expect_context`.
    - Computes content hash, checks cache — returns cached blob on hit.
    - On miss: scaffolds micro-crate (T-008), builds via cargo (T-009), parses diagnostics (T-011), runs wasm-opt optimization (T-012), stores blob in cache (T-010).
    - On build failure: returns parsed diagnostics, or synthesizes a raw-output diagnostic if structured parsing yields nothing.
    - Uses `tracing::info!` for timing/status at each pipeline stage.
    - Uses session_id "default" for shared incremental build cache (single-user MVP).
  - Updated `crates/ironpad-app/src/compiler/mod.rs` — registered `optimize` module.
  - Updated `crates/ironpad-app/src/lib.rs` — registered `server_fns` module (not feature-gated, as `#[server]` macro handles SSR/hydrate gating internally).
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 56 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs can be verified at this stage — all depend on the full compilation pipeline, UI, and Playwright infrastructure being functional.
- **Constitution Compliance**: T-012 was implemented alongside T-013 because T-013 explicitly references "optimize (T-012)" as a pipeline stage. Implementing them together avoids creating a compile_cell function that skips a documented step. No other constitutional violations.

## 2026-03-07 — T-014 Completed
- **Task**: Notebook filesystem CRUD
- **Status**: ✅ Done
- **Changes**:
  - Added `chrono` as optional SSR-gated dependency in `crates/ironpad-app/Cargo.toml`.
  - Created `crates/ironpad-app/src/notebook/mod.rs` — notebook module declaration.
  - Created `crates/ironpad-app/src/notebook/storage.rs` — full notebook filesystem CRUD:
    - Path helpers: `notebooks_dir()`, `notebook_dir()`, `manifest_path()`.
    - `create_notebook(data_dir, title)` — generates UUID, writes `ironpad.json` manifest, creates `cells/` directory.
    - `get_notebook(data_dir, id)` — reads and parses `ironpad.json` manifest.
    - `update_notebook(data_dir, id, title?, cells?)` — updates manifest fields, bumps `updated_at`.
    - `delete_notebook(data_dir, id)` — removes entire notebook directory.
    - `list_notebooks(data_dir)` — scans `notebooks/` dir, returns `Vec<NotebookSummary>` sorted by `updated_at` descending, gracefully skips malformed entries.
    - Internal helpers: `write_manifest()`, `read_manifest()`.
    - Uses `anyhow::Result` with `.context()` for error handling, `tracing` for diagnostics.
    - 15 unit tests: create writes manifest and cells dir, get reads manifest, get missing returns error, update title, update cells, delete removes directory, delete missing returns error, list empty, list returns summaries, list sorted by updated_at descending, list skips malformed manifests, list skips non-directory entries, list skips dir without manifest, round-trip preserves all fields, summary cell_count matches.
  - Updated `crates/ironpad-app/src/lib.rs` — added `#[cfg(feature = "ssr")] pub mod notebook;`.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 71 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs can be verified at this stage — uat-007 (notebook persistence after save/reload) and uat-009 (sample notebook pre-loaded) depend on UI and server function tasks (T-016+, T-017+).
- **Constitution Compliance**: No violations.

## 2026-03-07 — T-015 Completed
- **Task**: Cell filesystem CRUD
- **Status**: ✅ Done
- **Changes**:
  - Created `crates/ironpad-app/src/notebook/cells.rs` — full cell filesystem CRUD:
    - Path helpers: `cells_dir()`, `cell_dir()`, `source_path()`, `cargo_toml_path()`.
    - `add_cell(data_dir, notebook_id, cell_id, label, after_cell_id)` — creates `cells/{cell_id}/` directory with default `source.rs` and `Cargo.toml` (per MegaPrd §8.3), appends `CellManifest` to notebook manifest with optional insertion position.
    - `get_cell_source()` / `get_cell_cargo_toml()` — reads cell files from disk.
    - `update_cell_source()` / `update_cell_cargo_toml()` — overwrites cell files, bumps notebook `updated_at`.
    - `delete_cell()` — removes cell from manifest first (for consistency), then removes directory (tolerates missing dir).
    - `reorder_cells(data_dir, notebook_id, cell_ids)` — validates all IDs exist and count matches, rebuilds cells array in new order with sequential numbering.
    - `rename_cell()` — updates cell label in manifest.
    - `renumber_cells()` internal helper for sequential order assignment.
    - Default `Cargo.toml` template per MegaPrd §8.3: `cdylib` crate-type, `ironpad-cell = "0.1"` dependency, cell ID as package name.
    - Default `source.rs`: minimal `CellOutput::text("hello from ironpad").into()`.
    - 26 unit tests covering all CRUD operations, edge cases, and helper functions.
  - Updated `crates/ironpad-app/src/notebook/mod.rs` — registered `cells` module.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs can be verified at this stage — all depend on UI, server functions (T-016+), and Playwright infrastructure (T-045+).
- **Constitution Compliance**: No violations.

## 2026-03-07 — T-016 Completed
- **Task**: Server functions for notebook and cell operations
- **Status**: ✅ Done
- **Changes**:
  - Updated `crates/ironpad-app/src/server_fns.rs` — added 11 Leptos `#[server]` functions wrapping the notebook/cell storage layer:
    - `list_notebooks()` → `Vec<NotebookSummary>` — lists all notebooks sorted by most recently updated.
    - `create_notebook(title)` → `NotebookManifest` — creates a new notebook with the given title.
    - `get_notebook(id)` → `NotebookManifest` — retrieves a notebook manifest by UUID string.
    - `update_notebook(id, title)` → `NotebookManifest` — updates a notebook's title.
    - `delete_notebook(id)` → `()` — deletes a notebook by ID.
    - `add_cell(notebook_id, after_cell_id)` → `CellManifest` — adds a new cell with auto-generated UUID and sequential label.
    - `update_cell_source(notebook_id, cell_id, source)` → `()` — updates a cell's source code.
    - `update_cell_cargo_toml(notebook_id, cell_id, cargo_toml)` → `()` — updates a cell's Cargo.toml.
    - `delete_cell(notebook_id, cell_id)` → `()` — deletes a cell from a notebook.
    - `reorder_cells(notebook_id, cell_ids)` → `()` — reorders cells in a notebook.
    - `rename_cell(notebook_id, cell_id, label)` → `()` — renames a cell's label.
  - Added `parse_uuid()` SSR-gated helper for converting string IDs to `Uuid` with proper error handling.
  - All server functions follow the existing `compile_cell` pattern: `expect_context::<AppConfig>()`, `map_err` to `ServerFnError::new()`.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs can be verified at this stage — all depend on UI components (T-018+) and Playwright infrastructure (T-045+).
- **Constitution Compliance**: No violations.

## 2026-03-07 — T-018 Completed
- **Task**: Leptos app root component with routing
- **Status**: ✅ Done
- **Changes**:
  - Updated `crates/ironpad-app/src/lib.rs`:
    - Wrapped entire app in Thaw `ConfigProvider` with `Theme::dark()` default
    - Added `/notebook/{id}` route using `(StaticSegment("notebook"), ParamSegment("id"))`
    - Added placeholder `NotebookEditorPage` component (to be replaced by T-021)
    - Added `ParamSegment` import from `leptos_router`
    - Added `ConfigProvider` and `Theme` imports from `thaw`
  - Existing `HomePage` placeholder and `shell()` function unchanged
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped)
- **Opportunistic UAT**: No UATs can be verified yet — all depend on Playwright infrastructure (T-045+).
- **Constitution Compliance**: No violations.

## 2026-03-07 — T-019 Completed
- **Task**: App layout with header and status bar
- **Status**: ✅ Done
- **Changes**:
  - Created `crates/ironpad-app/src/components/mod.rs` — new UI components module.
  - Created `crates/ironpad-app/src/components/app_layout.rs` — app layout with:
    - `LayoutContext` — shared reactive state (`Copy`-safe) with `RwSignal` fields for notebook_title, show_save_button, save_generation, cell_count, last_save_time, compiler_version. Provided via Leptos context so child pages (T-020, T-021) can update the header and status bar.
    - `AppLayout` component — wraps child routes in Thaw `Layout` (absolute position) with `LayoutHeader`, content area, and custom footer.
    - `HeaderContent` component — "ironpad" brand link (left), reactive notebook title (center, shown only when set), save `Button` with `ButtonAppearance::Primary` (right, shown only when `show_save_button` is true). Save click increments `save_generation` signal.
    - `StatusBar` component — displays "Status: Ready", compiler version, cell count, and optional last-save time with pipe separators.
  - Updated `crates/ironpad-app/src/lib.rs` — added `pub mod components`, imported `AppLayout`, wrapped `<Routes>` inside `<AppLayout>` (replaced bare `<main>` wrapper).
  - Updated `style/main.scss` — added layout CSS: root layout (flex column, 100vh), header (48px, dark nav bar, flexbox three-section layout), brand styling (red accent with hover), notebook title (centered, truncated), content area (flex-grow, scrollable), status bar (28px, muted text, gap-separated items).
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs can be verified at this stage — all depend on Playwright infrastructure (T-045+) and a running server.
- **Constitution Compliance**: Thaw 0.5.0-beta does not export `LayoutFooter`, so a custom `<footer>` element with CSS styling is used instead. This is a minor deviation from the task notes ("Use Thaw components: Layout, LayoutHeader, LayoutFooter") but unavoidable given the library API. `Layout` and `LayoutHeader` from Thaw are used as specified.

## 2026-03-07 — T-020 Completed
- **Task**: Home page with notebook list and create button
- **Status**: ✅ Done
- **Changes**:
  - Created `crates/ironpad-app/src/pages/mod.rs` — new pages module for route-level components.
  - Created `crates/ironpad-app/src/pages/home_page.rs` — full `HomePage` component:
    - Resets `LayoutContext` signals on mount (clears notebook title, hides save button, zeros cell count).
    - Uses `Resource::new` to fetch notebooks via `list_notebooks` server fn.
    - Renders notebook list as a responsive CSS grid of Thaw `Card` components, each showing title, cell count, and last-updated timestamp.
    - Cards link to `/notebook/{id}` for client-side navigation.
    - "New Notebook" button uses `Action` to call `create_notebook("Untitled Notebook")`, with an `Effect` that navigates to the new notebook on success.
    - Empty state message when no notebooks exist.
    - Loading state with Thaw `Spinner` via `Suspense`/`Suspend`.
    - `NotebookCard` sub-component for individual notebook cards with `CardHeader` and metadata body.
  - Updated `crates/ironpad-app/src/lib.rs` — added `pub mod pages`, imported `pages::HomePage`, removed inline placeholder `HomePage` component.
  - Updated `style/main.scss` — added home page styles: centered max-width container, flexbox header with title and button, responsive grid layout for cards (`auto-fill, minmax(280px, 1fr)`), card hover border accent, loading/empty/error states.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs can be verified at this stage — all depend on Playwright infrastructure (T-045+) and a running server.
- **Constitution Compliance**: No violations. Thaw `Card` has a default `width: 720px` which is overridden via `.ironpad-notebook-card { width: 100% }` to work within the grid layout — this is standard CSS customization, not a Thaw API deviation.

## 2026-03-07 — T-021 Completed
- **Task**: Notebook editor page skeleton
- **Status**: ✅ Done
- **Changes**:
  - Created `crates/ironpad-app/src/pages/notebook_editor.rs` — full notebook editor skeleton:
    - `NotebookState` — reactive state struct (`Copy`-safe) with `RwSignal` fields for `notebook_id`, `cells` (ordered list), `active_cell`, and `refresh_generation` (triggers re-fetch). Provided via Leptos context for child components.
    - `NotebookEditorPage` component — extracts notebook ID from URL params via `use_params_map`, fetches notebook via `get_notebook` server fn with `Resource::new`, wires up `LayoutContext` (sets title, shows save button, updates cell count and compiler version), renders loading/error/content states.
    - `NotebookContent` component — renders editable title (`<input>` with `on:blur` save via `update_notebook` server fn), ordered cell list, and `AddCellButton` between each cell and at the end. Add-cell action calls `add_cell` server fn and triggers notebook refresh.
    - `CellItem` component — Thaw `Card` with dynamic active-state class (via `Signal::derive`), cell order badge, editable label input (with `rename_cell` on blur), delete button (calls `delete_cell` and triggers refresh), and placeholder body for future Monaco integration.
    - `AddCellButton` component — dashed-border button centered between cells, dispatches add action with optional `after_cell_id` for insertion positioning.
  - Updated `crates/ironpad-app/src/pages/mod.rs` — registered `notebook_editor` module and re-exported `NotebookEditorPage`.
  - Updated `crates/ironpad-app/src/lib.rs` — imported `NotebookEditorPage` from `pages` module, removed inline placeholder component.
  - Updated `style/main.scss` — added notebook editor styles: editor container (max-width 960px), editable title input (transparent background, red accent on focus), cell list (flex column), cell cards (dark background, border transitions, active state highlight), cell header (order badge + label input + action buttons), placeholder body, add-cell button (dashed border, hover opacity transition).
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs can be verified at this stage — all depend on Playwright infrastructure (T-045+) and a running server.
- **Constitution Compliance**: No violations.

## 2026-03-07 — T-022 Completed
- **Task**: Bundle Monaco editor JS/CSS with the server
- **Status**: ✅ Done
- **Changes**:
  - Created `package.json` with `monaco-editor` (v0.55.1) dependency — Monaco is installed via npm and copied to `public/monaco/` at build time.
  - Copied `node_modules/monaco-editor/min/vs/` to `public/monaco/vs/` — serves Monaco AMD loader, editor core, CSS, workers, and language assets from the ironpad server (no CDN, per MegaPrd OQ5 option b).
  - Created `public/monaco/init.js` — configures the AMD loader (`require.config({ paths: { vs: "/monaco/vs" } })`) and sets `window.MonacoEnvironment.getWorkerUrl` to map worker labels (json, css, html, typescript, editor) to the correct content-hashed worker files under `/monaco/vs/assets/`.
  - Updated `crates/ironpad-app/src/lib.rs` — added `<script src="/monaco/vs/loader.js">` and `<script src="/monaco/init.js">` to the shell's `<head>` section, ensuring the Monaco AMD loader and worker configuration are available on every page.
  - Updated `Makefile.toml` — added `setup-monaco` task (runs `npm install` + copies dist files to `public/monaco/`), wired as dependency for `dev` and `build` tasks so Monaco is always available.
  - Updated `.gitignore` — added `public/monaco/vs/` (generated from node_modules, not committed).
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs can be verified at this stage — all depend on Playwright infrastructure (T-045+) and a running server.
- **Constitution Compliance**: No violations. The hashed worker filenames in `init.js` are tied to monaco-editor v0.55.1; when the dependency is updated, `setup-monaco` will copy new files and the hashes in `init.js` will need updating (documented in init.js comments).

## 2026-03-07 — T-023 Completed
- **Task**: Monaco Leptos wrapper component via wasm-bindgen
- **Status**: ✅ Done
- **Changes**:
  - Created `public/monaco/bridge.js` — JS bridge (`window.IronpadMonaco`) wrapping Monaco AMD API with `create`, `getValue`, `setValue`, and `dispose` methods. Handles asynchronous Monaco loading via `require()` with queued operations.
  - Created `crates/ironpad-app/src/components/monaco_editor.rs` — `MonacoEditor` Leptos component:
    - Renders a container `<div>` with `NodeRef` for DOM access.
    - On mount (via `Effect`), creates a Monaco editor through the JS bridge using `#[wasm_bindgen]` extern bindings.
    - Props: `initial_value` (String), `language` (String, default "rust"), `on_change` (optional `Callback<String>`), `handle` (optional `RwSignal<Option<MonacoEditorHandle>>`).
    - `MonacoEditorHandle` struct exposes imperative `get_value()` / `set_value()` methods.
    - Cleanup via `on_cleanup` calls `dispose()` on unmount.
    - SSR-safe: all JS interop gated behind `#[cfg(feature = "hydrate")]`.
  - Updated `crates/ironpad-app/src/components/mod.rs` — exported `monaco_editor` module.
  - Updated `crates/ironpad-app/Cargo.toml` — added `wasm-bindgen`, `js-sys`, `web-sys` (with `HtmlElement`, `HtmlDivElement` features) as optional deps behind `hydrate` feature.
  - Updated `crates/ironpad-app/src/lib.rs` — added `<script src="/monaco/bridge.js">` to the shell `<head>`.
  - Updated `crates/ironpad-app/src/pages/notebook_editor.rs` — replaced cell body placeholder with `<MonacoEditor initial_value="" language="rust"/>`.
  - Updated `style/main.scss` — added `.ironpad-monaco-container` CSS (min-height 200px, rounded bottom corners, overflow hidden).
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs can be verified at this stage — all depend on Playwright infrastructure (T-045+) and a running server.
- **Constitution Compliance**: No violations.

## 2026-03-07 — T-025 Completed
- **Task**: Cell card component with tab bar
- **Status**: ✅ Done
- **Changes**:
  - Added `CellContent` struct (source + cargo_toml) to `crates/ironpad-common/src/types.rs` for cell content transfer.
  - Added `get_cell_content` server function to `crates/ironpad-app/src/server_fns.rs` — fetches cell source and Cargo.toml from disk.
  - Refactored `CellItem` component in `crates/ironpad-app/src/pages/notebook_editor.rs`:
    - Tab bar using Thaw `TabList` + `Tab` with "Code" and "Cargo.toml" tabs.
    - Two independent Monaco editor instances (Rust + TOML) — shown/hidden based on selected tab.
    - Cell content loaded lazily via `Resource` calling `get_cell_content`, with spinner fallback.
    - Collapse/expand toggle button (▸/▾) with CSS-driven animation.
    - Header elements: collapse button, order badge, editable label, status indicator placeholder ("idle"), run button (▶, placeholder for T-028), delete button (✕).
    - Source and cargo_toml stored as `RwSignal` for downstream compile flow (T-028).
    - `MonacoEditorHandle` signals exposed for imperative access.
    - `on_change` callbacks keep reactive state in sync with editor content.
  - Updated `style/main.scss`:
    - Collapse button styles, cell body collapse animation (max-height + opacity).
    - Status indicator styles for all states (idle, compiling, running, success, error).
    - Tab bar styles (`.ironpad-cell-tabs`).
    - Editor pane and loading spinner styles.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs verified — all depend on Playwright infrastructure (T-045+).
- **Constitution Compliance**: No violations.

## 2026-03-07 — T-026 Completed
- **Task**: Cell code editor (Monaco for source.rs)
- **Status**: ✅ Done
- **Changes**:
  - Added `Window` feature to `web-sys` in `crates/ironpad-app/Cargo.toml` for `setTimeout`/`clearTimeout` access.
  - Added `source_dirty: RwSignal<bool>` to track unsaved source editor changes.
  - Implemented debounced auto-save in `on_source_change` callback (hydrate build only):
    - Uses a reusable `Closure<dyn Fn()>` created once per cell (bounded one-time leak via `forget`).
    - `clearTimeout`/`setTimeout` pattern with 1 s debounce window.
    - Save reads current source from signal, calls `update_cell_source` server fn.
    - Race-safe: only clears dirty flag if source hasn't changed during save flight.
  - SSR build retains simple signal-update callback (no debounce plumbing).
  - Updated "Code" tab label to show `"Code ●"` when `source_dirty` is true.
  - Added `update_cell_source` to imports in `notebook_editor.rs`.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs verified — all depend on Playwright infrastructure (T-045+).
- **Constitution Compliance**: No violations.

## 2026-03-07 — T-027 Completed
- **Task**: Cell Cargo.toml editor (Monaco for Cargo.toml)
- **Status**: ✅ Done
- **Changes**:
  - Added debounce-save to the Cargo.toml Monaco editor in `crates/ironpad-app/src/pages/notebook_editor.rs`.
  - Mirrors the source editor pattern: `#[cfg(feature = "hydrate")]` branch uses `Closure` + `setTimeout`/`clearTimeout` with 1 s debounce window to call `update_cell_cargo_toml` server fn.
  - SSR build retains simple signal-update callback (no debounce plumbing).
  - Added `cargo_toml_dirty` signal for unsaved-changes tracking.
  - Updated "Cargo.toml" tab label to show `"Cargo.toml ●"` when `cargo_toml_dirty` is true.
  - Added `update_cell_cargo_toml` to imports in `notebook_editor.rs`.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs verified — all depend on Playwright infrastructure (T-045+).
- **Constitution Compliance**: No violations.

## 2026-03-07 — T-028 Completed
- **Task**: Cell run button with compile and execute flow
- **Status**: ✅ Done
- **Changes**:
  - Wired the ▶ run button on each cell to call `compile_cell` server fn with current source + Cargo.toml.
  - Added `CellStatus` enum (Idle, Compiling, Success, Error) in `notebook_editor.rs` for reactive cell status tracking.
  - Added reactive status indicator in cell header — shows "idle", "compiling…", "✓ {time}ms", or "✕ error" with matching CSS classes.
  - Run button shows ⏳ while compiling to prevent double-dispatch.
  - Added `CompileResultPanel` component below each cell: shows compilation summary (blob size, timing, cached) on success, and structured diagnostics on error.
  - Added `DiagnosticItem` component for rendering individual diagnostics with severity coloring and span info.
  - Bound Shift+Enter keybinding in the source Monaco editor to trigger the compile flow.
  - Extended Monaco bridge (`public/monaco/bridge.js`) with `addAction()` method for keybinding registration, including pending action queue for editors not yet ready.
  - Extended `MonacoEditorHandle` (`monaco_editor.rs`) with `add_action()` method for registering Monaco actions from Rust.
  - Added CSS for compile result panel and diagnostic rendering in `style/main.scss`.
  - Uses trigger signal pattern (`RwSignal<u64>`) for the run flow, avoiding closure cloning issues.
  - WASM execution (T-036/T-037) is deferred — compile succeeds but execution is a TODO placeholder.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs verified — all depend on Playwright infrastructure (T-045+).
- **Constitution Compliance**: No violations.

## 2026-03-07 — T-030 Completed
- **Task**: Cell output panel
- **Status**: ✅ Done
- **Changes**:
  - Added `ExecutionResult` type to `crates/ironpad-common/src/types.rs` with `display_text`, `output_bytes`, and `execution_time_ms` fields.
  - Added `CellOutputPanel` component to `crates/ironpad-app/src/pages/notebook_editor.rs` — renders below `CompileResultPanel` in each cell.
  - Panel is hidden when no execution result is available; collapsible via toggle when visible.
  - Shows human-readable display text (pre-formatted) and raw output bytes as a hex dump with offset, hex, and ASCII columns (16 bytes per row).
  - Shows byte count and execution timing in the panel header.
  - Added `format_hex_dump()` helper function for hex dump formatting.
  - Added `execution_result: RwSignal<Option<ExecutionResult>>` to `CellItem` component state, ready to be populated by the WASM executor (T-036/T-037).
  - Added CSS styling for output panel in `style/main.scss` (`.ironpad-output-panel`, `.ironpad-output-header`, `.ironpad-output-body`, `.ironpad-output-display`, `.ironpad-output-hex-dump`, etc.).
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs verified — all depend on Playwright infrastructure (T-045+).
- **Constitution Compliance**: No violations.

## 2026-03-07 — T-031 Completed
- **Task**: Cell error panel with formatted compiler diagnostics
- **Status**: ✅ Done
- **Changes**:
  - Added `code: Option<String>` field to `Diagnostic` in `crates/ironpad-common/src/types.rs` for structured error code access (with `serde` skip_serializing_if).
  - Updated diagnostic parser in `crates/ironpad-app/src/compiler/diagnostics.rs` to populate the `code` field instead of appending error codes to the message string.
  - Updated all `Diagnostic` construction sites (`server_fns.rs`, `notebook_editor.rs`) with `code: None`.
  - Created `crates/ironpad-app/src/components/error_panel.rs` — a dedicated `ErrorPanel` component with:
    - Collapsible header showing error/warning/note counts.
    - `ErrorDiagnosticItem` sub-component with color-coded severity badges (error=red, warning=yellow, note=blue).
    - Clickable links to the Rust error index for E-codes (e.g., `E0308` → `https://doc.rust-lang.org/error_codes/E0308.html`).
    - Non-linkable codes (e.g., lint names) displayed as plain text.
    - `SpanItem` sub-component showing line/column locations with optional labels.
  - Updated `CompileResultPanel` to use `ErrorPanel` for both error state and success-with-warnings state, replacing the old inline `DiagnosticItem`.
  - Removed the now-unused `DiagnosticItem` component from `notebook_editor.rs`.
  - Added comprehensive CSS styling for the error panel in `style/main.scss`.
  - Updated diagnostic parser tests to verify `code` field instead of bracket-appended message.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs verified — all depend on Playwright infrastructure (T-045+).
- **Constitution Compliance**: Added `code` field to public `Diagnostic` struct — this is a necessary API change required by T-031's error code linking requirement. Backward-compatible due to `serde(default)`.

## 2026-03-07 — T-033 Completed
- **Task**: Add Cell button between cells and at bottom
- **Status**: ✅ Done
- **Changes**:
  - The `AddCellButton` component and basic add-cell flow already existed from prior tasks. This task completed the missing scroll-to-and-focus behavior for newly added cells.
  - Added `focus()` method to `IronpadMonaco` JS bridge (`public/monaco/bridge.js`).
  - Added `focus` wasm-bindgen binding and `MonacoEditorHandle::focus()` method in `crates/ironpad-app/src/components/monaco_editor.rs`.
  - Added `pending_focus_cell: RwSignal<Option<String>>` to `NotebookState` in `crates/ironpad-app/src/pages/notebook_editor.rs`.
  - Updated add-cell effect to capture the new cell's ID and set `pending_focus_cell`.
  - Added scroll-to-and-focus `Effect` in `CellItem` that triggers when `pending_focus_cell` matches, using `scrollIntoView()` and a 300ms delayed `focus()` call to allow Monaco async initialization.
  - Wrapped cell `Card` in a `<div node_ref=cell_wrapper_ref>` for scroll targeting.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 97 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs verified — all depend on Playwright infrastructure (T-045+).
- **Constitution Compliance**: No violations.

## 2026-03-07 — T-034 Completed
- **Task**: Error span mapping (wrapper offset adjustment)
- **Status**: ✅ Done
- **Changes**:
  - Added 12 focused tests to `crates/ironpad-app/src/compiler/diagnostics.rs` covering error types not previously tested:
    - **Syntax errors**: missing semicolon, unexpected closing brace, unclosed delimiter (multiline span)
    - **Borrow checker errors**: E0382 (use after move), E0505 (move while borrowed)
    - **Lifetime errors**: E0106 (missing lifetime specifier)
    - **Column offset edge cases**: column 1 (start of line), high column offsets (deeply indented code)
    - **Integration test**: mixed error types in a single compilation output
    - **adjust_span edge cases**: closing brace line, single-char column range, span crossing preamble into user code
  - The core `adjust_span` logic (subtracting `WRAPPER_PREAMBLE_LINES` from line numbers, passing columns through unchanged) was already correct from T-011; this task validates it thoroughly.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 109 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: No UATs verified — all depend on Playwright infrastructure (T-045+).
- **Constitution Compliance**: No violations.

## 2026-03-07 — T-035 Completed
- **Task**: Monaco inline error markers via setModelMarkers
- **Status**: ✅ Done
- **Changes**:
  - Extended `public/monaco/bridge.js` with `setMarkers(id, markers)` and `clearMarkers(id)` methods on the `IronpadMonaco` bridge, including pending-marker queuing for editors still loading.
  - Added `set_markers` and `clear_markers` wasm-bindgen extern declarations in `crates/ironpad-app/src/components/monaco_editor.rs`, and corresponding methods on `MonacoEditorHandle`.
  - In `crates/ironpad-app/src/pages/notebook_editor.rs`:
    - Clear markers at compile start (before dispatching the server call).
    - Added a reactive `Effect` that watches `last_compile` and converts `Diagnostic` spans to Monaco marker objects (`startLineNumber`, `startColumn`, `endLineNumber`, `endColumn`, `message`, `severity`).
    - Severity mapping: `Severity::Error` → 8, `Severity::Warning` → 4, `Severity::Note` → 2 (matching Monaco `MarkerSeverity` constants).
    - Uses span label when available, falls back to diagnostic message.
  - `cargo make uat` ✅ passes (fmt-check ✅, clippy ✅, 109 tests pass ✅, playwright skipped).
- **Opportunistic UAT**: uat-006 ("Compiler errors render inline in Monaco with span highlighting") is functionally implemented but cannot be verified without Playwright (T-045+).
- **Constitution Compliance**: No violations.
