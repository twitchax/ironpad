# public-notebooks Specification

## Purpose
The bundled showcase catalog: first-party `.ironpad` notebooks shipped with the server, listed and fetched with no index file, each carrying the author's saved outputs so a reader sees finished results before any compile. Covers where the catalog is served from, the listing and fetch contracts, the content invariants every notebook must hold, and the capture tool and freshness guard that keep saved outputs honest against the code beside them.

## Requirements
### Requirement: Catalog location
The system SHALL ship the catalog as the `*.ironpad` files in `public/notebooks/`, bundled into `{site_root}/notebooks/`, and SHALL also serve each file raw at `/notebooks/{name}.ironpad` with `Cache-Control: no-cache`; a deploy is the only way the catalog changes. (PRD-0020)

#### Scenario: Raw file
- **WHEN** a client requests `/notebooks/cannon.ironpad`
- **THEN** it receives the notebook JSON with `Cache-Control: no-cache`

### Requirement: Listing without an index
The system SHALL list the catalog through the `list_public_notebooks` server function by enumerating `{site_root}/notebooks/*.ironpad` and reading each file's own metadata, returning per notebook its `filename` (also its `id`), `title`, `description` (empty when absent), `cell_count`, and `tags`, sorted by title. (PRD-0020)

#### Scenario: Two notebooks and a stray file
- **WHEN** the directory holds `alpha.ironpad` titled "Test Notebook" with 1 cell, `zeta.ironpad` titled "Zeta Notebook", and `readme.txt`
- **THEN** the listing has two entries, "Test Notebook" (`alpha.ironpad`, 1 cell) then "Zeta Notebook" (`zeta.ironpad`)

#### Scenario: Unreadable entries
- **WHEN** a `.ironpad` file cannot be read or does not parse as a notebook
- **THEN** it is left out of the listing and the rest still list

#### Scenario: No catalog directory
- **WHEN** `{site_root}/notebooks/` is empty or missing
- **THEN** the listing is empty rather than an error

### Requirement: Listing is fixed for the life of the process
The system SHALL compute the catalog listing once per server process and reuse it for the home page listing, `/sitemap.xml`, and the site card's notebook count; a catalog file added or edited while the server runs SHALL appear only after a restart. (PRD-0051)

#### Scenario: File added at runtime
- **WHEN** a new `.ironpad` file is copied into `{site_root}/notebooks/` while the server runs
- **THEN** neither the listing nor the sitemap includes it until the server restarts

### Requirement: Fetching one notebook
The system SHALL return one catalog notebook through the `get_public_notebook` server function given its name with or without the `.ironpad` suffix, resolving any other name to `{name}.ironpad`, so the function serves only notebook files. (PRD-0048)

#### Scenario: Both name forms
- **WHEN** `demo.ironpad` exists and a caller asks for `demo` and for `demo.ironpad`
- **THEN** both return the notebook titled as stored

#### Scenario: A non-notebook file
- **WHEN** `evil.json` exists beside the notebooks and a caller asks for `evil.json`
- **THEN** the call fails, because it looks for `evil.json.ironpad`

#### Scenario: Missing notebook
- **WHEN** a caller asks for `nope.ironpad` and no such file exists
- **THEN** the call fails with an error containing "not found"

### Requirement: Fetch rejects path escapes
The system MUST reject a `get_public_notebook` name that is empty or contains `/`, `\`, or `..`, with the error "invalid filename: must be non-empty, without separators or '..'", before touching the filesystem. (PRD-0048)

#### Scenario: Traversal attempts
- **WHEN** a caller asks for `../secret.ironpad`, `sub/file.ironpad`, `a\b`, `..`, or an empty name
- **THEN** each call fails with "invalid filename"

### Requirement: Every catalog notebook is well-formed
The project SHALL keep every catalog file a valid notebook: it deserializes as a notebook of version 1, has at least one cell and a `description`, numbers its cells' `order` 0, 1, 2, … in file order, and gives every cell a non-empty `id`, `label`, and `source`; `cargo make test` fails otherwise. (PRD-0011, PRD-0020)

#### Scenario: A notebook missing its description
- **WHEN** a catalog file omits `description`
- **THEN** the schema conformance test fails naming that file and "missing description field"

#### Scenario: Out-of-order cells
- **WHEN** a catalog file's second cell carries `order: 2`
- **THEN** the test fails naming that file and the expected order 1

### Requirement: Every catalog cell compiles unless it says otherwise
The project SHALL type-check every compiling cell of every catalog notebook against its cell type's target in `cargo make test-integration`, and SHALL require a cell carrying the marker `// ironpad-test: compile-fail` to fail type-checking, so a deliberate teaching failure that starts compiling is flagged. (f96cfe4)

#### Scenario: A teaching cell starts compiling
- **WHEN** a toolchain change makes a cell marked `// ironpad-test: compile-fail` type-check
- **THEN** the public-notebook compile test fails for that notebook

### Requirement: Catalog notebooks carry saved outputs
The project SHALL ship each catalog notebook with a `saved_output` on every runnable code cell that produced output in a real browser run, and with none on cells that failed, on shared cells, on markdown, or on Linux cells, so readers see results before compiling anything. (PRD-0056, PRD-0066)

#### Scenario: Deliberate compile-fail cell
- **WHEN** a catalog notebook's teaching cell fails to compile on purpose
- **THEN** that cell carries no `saved_output` while its independent siblings do

#### Scenario: Reader of a showcase notebook
- **WHEN** a reader opens `/public/cannon` before any cell runs
- **THEN** its code cells show the saved outputs committed in `cannon.ironpad`

### Requirement: Capture tool
The project SHALL provide `cargo make capture-outputs [-- name …]`, which runs each named catalog notebook (all by default) in a real browser against a server at `IRONPAD_BASE` (default `http://localhost:3111`), one runnable code cell at a time with a per-cell budget of `CELL_BUDGET_MS` (default 180000), and writes back only the `saved_output` fields captured by the app's own Download flow. (PRD-0056, PRD-0060, PRD-0066)

#### Scenario: Cells never clicked
- **WHEN** the tool captures a notebook containing Linux cells and shared cells
- **THEN** it never runs them and they gain no `saved_output`

#### Scenario: Stale snapshots are not carried forward
- **WHEN** a cell that captured output before now fails during a recapture
- **THEN** the rewritten file drops that cell's old `saved_output`

#### Scenario: Notebook with no runnable cells
- **WHEN** the tool reaches a markdown-only notebook
- **THEN** it reports "skipped (no runnable cells)" and still records a freshness entry for it

### Requirement: Capture refuses to strip a corpus on an environmental failure
The capture tool MUST keep a notebook file unchanged and exit non-zero when a run captures nothing for a notebook whose committed file holds snapshots, unless `--allow-empty` is passed. (PRD-0056)

#### Scenario: Broken server
- **WHEN** every cell of a notebook with committed snapshots errors because the server lacks its toolchain
- **THEN** the tool reports "SUSPECT: 0 captured but the committed file holds N snapshot(s)", leaves the file as it was, and exits 1

### Requirement: Capture freshness manifest
The project SHALL record, in `public/notebooks/.capture-manifest.json` at capture time, each catalog notebook's `cells` map (each non-shared code cell's id to the first 16 hex characters of the sha256 of its source, a NUL, and its own `Cargo.toml`) and a `shared` digest of the notebook-level shared source, every shared cell's source in order, and the shared `Cargo.toml`. (PRD-0056)

#### Scenario: Entry shape
- **WHEN** `async-http` is captured
- **THEN** its manifest entry holds a `cells` map keyed by its code cell ids with 16-hex digests and a 16-hex `shared` digest

### Requirement: Freshness guard
The project SHALL fail `cargo make capture-outputs-check` when any catalog notebook's recomputed manifest entry differs from the committed one, when the manifest names a notebook that no longer exists, when the manifest is missing, or when a Linux cell carries a `saved_output`, and SHALL pass with "capture manifest is fresh ({n} notebooks)" otherwise. (PRD-0056, PRD-0066)

#### Scenario: Code edited without a recapture
- **WHEN** a maintainer edits a code cell's source in `cannon.ironpad` and runs the check
- **THEN** it prints "stale saved outputs: cannon (compiled text changed since last capture)" and exits 1

#### Scenario: Shared source edited
- **WHEN** a maintainer edits only the notebook-level shared source of a catalog notebook
- **THEN** the check reports that notebook stale

#### Scenario: Deleted notebook
- **WHEN** a catalog file is deleted but its manifest entry remains
- **THEN** the check prints "manifest entry for deleted notebook: {name}" and exits 1

#### Scenario: Snapshot on a Linux cell
- **WHEN** a Linux cell in a catalog notebook carries a `saved_output`
- **THEN** the check names the notebook and cell and exits 1
