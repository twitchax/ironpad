# notebook-viewer Specification

## Purpose
The read-only notebook pages: the bundled showcase at `/public/{name}`, immutable shares at `/shared/{hash}`, and the published-copy reader at `/mutable/{id}`. Readers see every cell, read the author's saved outputs, run cells live in their own browser, fork a copy, and navigate with an outline rail, all without any way to edit the notebook they are reading.

## Requirements
### Requirement: Canonical read-only routes
The system SHALL serve read-only notebooks at a route whose first segment names the storage class: `/public/{name}` for a bundled notebook (extension-less), `/shared/{hash}` for an immutable share, and `/mutable/{id}` for the reader of a published notebook. `/public/{name}` SHALL also accept the name with its `.ironpad` suffix. (PRD-0048, PRD-0054)

#### Scenario: Canonical public route renders directly
- **WHEN** a reader opens `/public/welcome`
- **THEN** the URL stays `/public/welcome`
- **AND** the view-only notebook renders

#### Scenario: Suffixed public name still resolves
- **WHEN** a reader opens `/public/welcome.ironpad`
- **THEN** the page answers 200 and renders the same notebook as `/public/welcome`

### Requirement: Legacy routes redirect forever
The system SHALL redirect the pre-canonical routes to their canonical form: `/notebook/public/{file}` to `/public/{name}` with one `.ironpad` suffix stripped, and `/notebook/{id}` to `/local/{id}`. (PRD-0048)

#### Scenario: Legacy public link
- **WHEN** a reader opens `/notebook/public/welcome.ironpad`
- **THEN** the URL bar ends in `/public/welcome`
- **AND** the view-only notebook renders

#### Scenario: Legacy local link
- **WHEN** the owner of local notebook `{id}` opens `/notebook/{id}`
- **THEN** the URL bar ends in `/local/{id}` and the editor renders

### Requirement: Missing public notebooks answer 404
The system SHALL answer a `/public/{name}` request for a notebook that is not in the catalog with HTTP status 404 and an error panel in the body, never a 200 page carrying an error that unfurlers would cache as a real preview. (PRD-0050 T-005)

#### Scenario: Unknown public name
- **WHEN** a client requests `/public/no-such-notebook`
- **THEN** the response status is 404
- **AND** the body shows "Failed to load public notebook:" followed by the load error

### Requirement: Read-only rendering of every cell
The system SHALL render every cell of the notebook in notebook order with no way to edit, add, delete, or reorder cells: markdown as rendered prose, and code, shared, Linux, and unrecognized cells inside a frame whose source is a read-only editor. (PRD-0065, PRD-0066)

#### Scenario: Showcase notebook renders its cells
- **WHEN** a reader opens `/public/welcome`
- **THEN** at least four `.view-only-cell` elements render with read-only sources

#### Scenario: Empty markdown cell
- **WHEN** a notebook contains a markdown cell whose source renders to nothing
- **THEN** the cell shows the placeholder "(empty markdown cell)"

#### Scenario: Shared cell
- **WHEN** a notebook contains a cell marked shared
- **THEN** it renders framed with a "shared" badge and no run control or output

### Requirement: Collapse state follows the notebook
The system SHALL open each cell's source collapsed or expanded according to its saved `collapsed` flag, and a live result's output according to its `output_collapsed` flag, and SHALL let the reader toggle a cell's source with the frame chevron without persisting the change. (PRD-0065, effd30e)

#### Scenario: Story-style notebook loads collapsed
- **WHEN** a reader opens `/public/welcome`, whose code cells are saved with `collapsed: true`
- **THEN** at least one cell body carries the collapsed state

#### Scenario: Blog-style notebook loads expanded
- **WHEN** a reader opens `/public/dynosaur`, whose cells carry no collapse flags
- **THEN** no cell body is collapsed and a code cell's source is visible immediately

#### Scenario: Chevron toggles a code cell and a shared cell
- **WHEN** a reader on `/public/shared-code` clicks the chevron of a collapsed code cell, then clicks it again
- **THEN** the source expands and then collapses again
- **AND** the same holds for a collapsed shared cell

#### Scenario: Collapsed output after a live run
- **WHEN** a cell saved with `output_collapsed: true` produces a live result
- **THEN** an "Output" reveal button shows in place of the output panels until the reader clicks it

### Requirement: Readers run cells in their own browser
The system SHALL give each runnable code cell on a read-only page an icon-only run control whose tooltip and accessible name is "Run this cell", alongside the toolbar's "Run All", so a reader can compile and execute the notebook in their own browser without leaving the page. (PRD-0007, PRD-0065)

#### Scenario: Run replaces a saved snapshot with a live result
- **WHEN** a reader of a shared notebook clicks the button named "Run this cell" on a cell showing a saved output
- **THEN** the saved-output block disappears
- **AND** a live output with its caption renders the same value

#### Scenario: Run control while compiling
- **WHEN** a cell is compiling
- **THEN** its run control is disabled, its accessible name is "Compiling…", and its frame header shows a "compiling" pill

### Requirement: Autorun follows the route's trust boundary
The system SHALL queue every runnable code cell once on page load on `/public/{name}`, whose notebooks ship with ironpad, and SHALL NOT run any cell on load on `/shared/{hash}` or the `/mutable/{id}` reader, whatever the notebook's saved Reactive Mode: only the route grants autorun, never the notebook. (PRD-0040)

#### Scenario: Showcase notebook runs on load and continues past a failure
- **WHEN** a reader opens `/public/dynosaur` and clicks nothing
- **THEN** its two compiling cells produce timing badges
- **AND** its deliberate compile-fail teaching cell shows its error inline

#### Scenario: Shared notebook stays idle
- **WHEN** a reader opens a `/shared/{hash}` notebook
- **THEN** no cell compiles or runs until the reader clicks a run control

#### Scenario: A reactive notebook cannot opt itself in
- **WHEN** a notebook saved in Reactive Mode is opened in a fresh browser at `/shared/{hash}` or the `/mutable/{id}` reader
- **THEN** the page neither compiles a cell nor fetches a share blob on load
- **AND** clicking Run All starts that traffic

### Requirement: Saved outputs are the initial state
The system SHALL render each code cell's saved output (the panels captured from the author's last run) as its initial output, marked as a serialized snapshot, until that cell's first live result or error replaces it; showing it SHALL require no compile or run. (PRD-0056)

#### Scenario: Cold anonymous reader sees the author's output
- **WHEN** an author runs a cell that outputs `42`, shares it immutably, and a fresh anonymous browser opens the share
- **THEN** the cell shows a saved-output block containing `42` with no click
- **AND** the block's badge says "Saved output" and "Run to execute live"

#### Scenario: Published notebook carries outputs to readers
- **WHEN** an author publishes, then pushes a renamed version of, a notebook with a cell that outputs `42`
- **THEN** an anonymous reader of `/mutable/{id}` sees the saved output `42` after each

#### Scenario: A snapshot is not the reader's run
- **WHEN** a cell shows only its saved output
- **THEN** its run glyph stays in the not-run state rather than the ran state

### Requirement: Fork copies a notebook into local storage
The system SHALL offer a "Fork" button on non-embedded read-only pages that saves a copy of the notebook to the reader's browser-local storage under a new id, titled `{title} (fork)`, toasts "Forked", and navigates to `/local/{id}` only after the save succeeds. (PRD-0033, PRD-0034, PRD-0048)

#### Scenario: Fork a showcase notebook
- **WHEN** a reader on `/public/welcome` clicks "Fork" after hydration
- **THEN** a toast titled "Forked" appears
- **AND** the URL becomes `/local/{uuid}` with the editor showing the copied cells

#### Scenario: Double click
- **WHEN** a reader clicks "Fork" twice in quick succession
- **THEN** the button reads "Forking…" and is disabled after the first click, so one copy is made

#### Scenario: Local save fails
- **WHEN** the browser-local save rejects
- **THEN** a toast titled "Fork Failed" says "Could not save the fork locally. Try again."
- **AND** the page stays put with the button enabled again

### Requirement: Toolbar title row and controls row
The system SHALL head each read-only page with a title row (a crumb naming the storage class, the notebook title as the page heading, and a "read-only" pill) above a separate controls row holding the Cached/Fresh compile toggle, "Run All", "Fork", and "Embed", all sharing one button shape. (PRD-0065)

#### Scenario: Breadcrumb and pill on a public notebook
- **WHEN** a reader opens `/public/charts-with-plot`
- **THEN** the crumb reads "public" and the "read-only" pill is visible

#### Scenario: Controls sit below a long title
- **WHEN** a reader opens `/public/borrows` at a 700px-wide viewport
- **THEN** the top of the controls row is at or below the bottom of the title
- **AND** "Run All" stays a single-line button under 45px tall

#### Scenario: One button shape
- **WHEN** a reader opens `/public/welcome`
- **THEN** "Run All", "Fork", and "Embed" all have a 6px border radius and one measured height of at least 34px, matched by the Cached/Fresh toggle

### Requirement: Studio cell frames
The system SHALL number framed cells `[1]` to `[n]` with no gaps, skipping markdown, and SHALL show in each runnable frame header its compile and run timing and a run glyph that is muted until the reader's own run, then marks success or failure. (PRD-0065)

#### Scenario: Contiguous frame numbers
- **WHEN** a reader opens `/public/charts-with-plot`
- **THEN** the frame indices read `[1]`, `[2]`, … `[n]` in order with no gaps

#### Scenario: Timing after a run
- **WHEN** a cell compiles and runs cleanly
- **THEN** its header shows `{compile}ms compile · {run}ms run` and its run glyph switches to the ran state

#### Scenario: Failure
- **WHEN** a cell fails to compile or run
- **THEN** its run glyph switches to the error state and no timing badge shows

### Requirement: Notebook outline rail
The system SHALL show beside the cells of every read-only notebook view, Preview included but never an embed, a "Notebook outline" navigation listing every cell by label in order with a status marker, then a Runtime group (toolchain and `wasm32-unknown-unknown`) and, when any exist, a Deps group of the notebook's merged dependencies. (PRD-0065)

#### Scenario: Rows match cells
- **WHEN** `/public/charts-with-plot` loads
- **THEN** the rail lists one row per cell, markdown rows carry a prose marker and no timing, and a shared cell's row is described as "Shared cell, never runs"

#### Scenario: Run state and totals
- **WHEN** a code cell has run
- **THEN** its row shows a "ran" marker and a timing such as `7.4ms` or `125ms`
- **AND** the Runtime group shows "compile" and "total run" totals, which are absent before anything has run

#### Scenario: Dependencies
- **WHEN** the shared Cargo.toml declares `serde = "1"` and a cell declares `serde = "1.0.200"`
- **THEN** the Deps group lists `serde` once with version `1.0.200`

### Requirement: Rail navigation
The system SHALL keep exactly one rail row selected, following the cell nearest the top of the cell list as the reader scrolls, and SHALL scroll a clicked row's cell to the top of the cell list without moving the page shell; at widths of 1024 pixels or less the rail collapses into an "Outline" button with the cell count that opens it as a dropdown. (PRD-0065)

#### Scenario: Scroll-spy
- **WHEN** the reader scrolls the cell list down 1500 pixels
- **THEN** the selection moves off the first row and exactly one row stays selected

#### Scenario: Click to scroll
- **WHEN** the reader clicks the fifth row
- **THEN** that row is selected, stays selected after the scroll settles, and the header and rail do not move

### Requirement: Same-route navigation follows the URL
The system SHALL re-render a read-only page for the new notebook when a navigation changes only the route parameter, as a markdown link from one public notebook to another does. (6ff1af4)

#### Scenario: Markdown cross-link between public notebooks
- **WHEN** a reader on `/public/cannon` clicks its in-prose link to `/public/autodiff`
- **THEN** the URL ends in `/public/autodiff`
- **AND** the page heading becomes "std::autodiff: Differentiation in the Compiler"
