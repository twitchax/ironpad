# local-notebooks Specification

## Purpose
Notebooks that live only in the user's browser: how they are stored and saved, how the store survives bad records and failed writes, the per-notebook version history and its undoable Restore, and the `.ironpad` JSON file format with the Download, Import and HTML export paths that move notebooks in and out of the browser.

## Requirements
### Requirement: Browser-local storage
The system SHALL keep local notebooks only in the browser's IndexedDB, keyed by notebook id, with no server-side copy unless the user shares, publishes or saves one to an account; each browser profile therefore holds its own private set. (PRD-0054)

#### Scenario: Another browser
- **WHEN** a user creates a local notebook in one browser and opens `/` in another
- **THEN** the second browser lists no such notebook

#### Scenario: Upgrading an old store
- **WHEN** a browser that last ran a release storing local working copies of published notebooks opens the app
- **THEN** that retired store is deleted and its local notebooks are kept

### Requirement: Saving local notebooks
The system SHALL write a local notebook to the browser store after every change, stamping its `updated_at`: cell source and Cargo.toml edits one second after typing stops or at once when the author leaves the editor, and every other change (adding, deleting, reordering, title, labels, collapse flags, Reactive Mode, metadata, shared code) at once. Leaving writes only edits no save has captured yet. (PRD-0001, PRD-0032)

#### Scenario: Source survives navigation
- **WHEN** the author adds a code cell, presses Ctrl+S, goes home through the brand link and opens the notebook's card
- **THEN** the cell's editor still shows `42`

#### Scenario: Collapse flag reaches the store
- **WHEN** the author collapses a cell's code with its default toggle
- **THEN** the stored notebook's first cell has `collapsed: true` within seconds, with no explicit save

#### Scenario: Leaving inside the save window
- **WHEN** the author types into a cell, or into an open markdown editor, and clicks "Back to notebook list" within the second
- **THEN** the stored notebook carries the typed text

#### Scenario: Leaving an untouched notebook
- **WHEN** the author opens a notebook and leaves without changing anything
- **THEN** the stored notebook's `updated_at` is unchanged

### Requirement: Listing tolerates bad records
The system SHALL list local notebooks most recently updated first, sorting records whose `updated_at` cannot be read last, and SHALL skip, with a console warning, any record that does not parse as a notebook while still listing every other one. (PRD-0033)

#### Scenario: One malformed record
- **WHEN** the store holds three notebooks and one of them is missing required fields
- **THEN** the home page lists the other two

### Requirement: Storage failures end cleanly
The system SHALL treat a rejected browser-store operation, such as a write over quota or an open blocked by another tab, as a failure that is logged to the console and ends the operation, never leaving a control stuck in a busy state. (PRD-0033)

#### Scenario: Blocked database open
- **WHEN** another tab holds the database at an older version and blocks the upgrade
- **THEN** the storage call fails instead of waiting forever

#### Scenario: Failed write behind a Save button
- **WHEN** the store rejects the write behind a shared-code or metadata Save
- **THEN** the button returns from "Saving…" to "Save"

#### Scenario: New notebook cannot be saved
- **WHEN** "New Notebook" is clicked and the store rejects the write
- **THEN** the home page stays where it is and logs the error

### Requirement: Version history capture
The system SHALL, each time a local notebook is saved, add the saved version to that notebook's history unless its newest snapshot is under five minutes old, keep at most 30 snapshots per notebook by dropping the oldest, keep history on this device only, and never fail a save because the snapshot failed. (PRD-0058)

#### Scenario: The ring holds 30
- **WHEN** a notebook is saved once and then force-snapshotted 32 times
- **THEN** its history holds exactly 30 snapshots, strictly newest first, and the first snapshot is gone

#### Scenario: Saves inside the bucket
- **WHEN** a notebook is saved repeatedly within five minutes of its newest snapshot
- **THEN** its history gains no snapshots

### Requirement: History panel
The system SHALL offer "History" in a local notebook's menu, opening a "Version History" overlay that explains snapshots are taken at most every five minutes and kept on this device, lists snapshots newest first with an age ("just now", "N min ago", "N h ago", "N d ago"), title and cell count, and otherwise reads "No snapshots yet. They appear as you edit." (PRD-0058)

#### Scenario: Opening the panel
- **WHEN** the author opens History on a notebook with saved snapshots
- **THEN** an opaque panel lists entries such as "3 min ago" beside "My Notebook · 2 cells", each with a Restore button
- **AND** clicking outside the panel or its close button dismisses it

### Requirement: Undoable restore
The system SHALL restore a snapshot only after the confirm "Restore this snapshot? Your current version is saved to history first, so this can be undone.", saving the current version (including unsaved typing) to history first, then making the snapshot the notebook's content at the same URL, reloading, and toasting "Restored". (PRD-0058)

#### Scenario: Restore and undo
- **WHEN** the author snapshots `let version_a = 1;`, edits it to `let version_b = 2;`, and restores the first snapshot
- **THEN** the editor shows `version_a` after the reload
- **AND** the newest history entry contains `version_b`

#### Scenario: Restoring from a full ring
- **WHEN** the history holds 30 snapshots and the author restores the oldest
- **THEN** that snapshot is restored, not pruned by the pre-restore save

#### Scenario: Unreadable snapshot
- **WHEN** the chosen snapshot cannot be read
- **THEN** an error toast "Restore Failed" says "That snapshot could not be read." and nothing changes

### Requirement: Deleting a local notebook
The system SHALL delete a local notebook, from its home card or from its editor menu, only after the confirm "Delete this notebook? This cannot be undone.", removing its record and its whole version history; deleting from the editor then navigates to `/`. (PRD-0002, PRD-0058)

#### Scenario: Delete from the editor
- **WHEN** the author chooses Delete in a local notebook's menu and accepts the confirm
- **THEN** the browser is at `/`, the notebook's record is gone, and its history is empty

### Requirement: The .ironpad notebook format
The system SHALL read and write notebooks as `.ironpad` JSON: an object with a numeric `version` (1), a UUID `id`, `title`, RFC 3339 `created_at` and `updated_at`, and a `cells` array, plus optional `description`, `tags`, `shared_cargo_toml`, `shared_source`, `reactive_mode`, `og_image`, `og_image_width` and `og_image_height`, each omitted when unset; unknown fields are ignored. (PRD-0010, PRD-0051)

#### Scenario: Legacy field
- **WHEN** a notebook file carries the retired notebook-level `expand_code` field
- **THEN** it opens normally and the field is dropped on the next save

#### Scenario: Unset fields stay out
- **WHEN** a notebook with no preview image is saved
- **THEN** its JSON contains no `og_image` key and reads back unchanged

### Requirement: Cell records in the format
The system SHALL encode each cell with `id`, `order`, `label`, `cell_type` (`Code`, `Markdown` or `Linux`, defaulting to `Code`), `source`, an optional `cargo_toml`, `shared`, `collapsed` and `output_collapsed` flags omitted while false, a `version` defaulting to 0, and an optional `saved_output` holding the cell's last-run output panels. (PRD-0044, PRD-0056)

#### Scenario: An old cell
- **WHEN** a cell record has only `id`, `order`, `label`, `cell_type` and `source`
- **THEN** it opens as open, unshared and at version 0, and re-serializes without any flag keys

#### Scenario: Collapse flags round trip
- **WHEN** a cell is saved with code and output collapsed
- **THEN** its JSON holds `"collapsed":true` and `"output_collapsed":true` and reads back collapsed

### Requirement: Unknown cell types survive
The system SHALL open a notebook containing a cell type it does not recognise, render that cell inert rather than as code, and write the cell's type tag back unchanged on every save. (PRD-0066)

#### Scenario: A cell from a newer release
- **WHEN** a notebook holds a `Code` cell and a cell with `"cell_type":"Wasi"`
- **THEN** the notebook opens with both cells, the `Wasi` cell never compiles
- **AND** after any save the stored JSON still contains `"cell_type":"Wasi"`

### Requirement: Download .ironpad
The system SHALL download the open notebook from its menu, in local and account editors alike, as pretty-printed `.ironpad` JSON named after the title with every character other than letters, digits, `-` and `_` replaced by `-`, embedding each cell's last output from this session within 256 KiB per cell. (PRD-0010, PRD-0056)

#### Scenario: Downloading a new notebook
- **WHEN** the author downloads "Untitled Notebook" after adding one cell
- **THEN** the file is `Untitled-Notebook.ironpad` and parses to an object with a numeric `version`, a string `title` and a one-cell `cells` array

#### Scenario: Oversized output
- **WHEN** a cell's last output exceeds 256 KiB
- **THEN** its `saved_output` is a text panel reading "(output too large to embed; run the cell to regenerate it)"

#### Scenario: Cell not run this session
- **WHEN** a cell arrived with a `saved_output` and has not run since the page loaded
- **THEN** the downloaded file keeps that `saved_output`

### Requirement: Importing a notebook file
The system SHALL import a `.ironpad` or `.json` file chosen through the home page's "Import Notebook" button as a new local notebook with a fresh id and creation time, never overwriting an existing one, then refresh the list in place and toast "Notebook Imported" with `"<title>" has been added to your notebooks.` (PRD-0010)

#### Scenario: Importing a valid file
- **WHEN** a file titled "E2E Import Test Notebook" with one code cell is chosen
- **THEN** the toast appears, a card with that title appears without a reload, and opening it shows one cell at a new `/local/{uuid}`

#### Scenario: Picker
- **WHEN** "Import Notebook" is clicked
- **THEN** a single-file picker opens that accepts `.ironpad` and `.json`

### Requirement: Import validation
The system SHALL reject, with an "Import Failed" toast naming the problem and without adding a notebook, a file that is not JSON, not an object, or lacks a numeric `version`, a string `title` or a `cells` array, or that has a cell missing a string `id`, `source` or `label`, or that otherwise does not parse as a notebook. Nothing SHALL be stored for a rejected file. (PRD-0010)

#### Scenario: Not JSON
- **WHEN** the chosen file contains `this is not valid json {{{`
- **THEN** an "Import Failed" toast appears and the number of local notebooks is unchanged

#### Scenario: Missing cells
- **WHEN** the chosen file is `{"version": 1, "title": "Missing Cells"}`
- **THEN** the toast reads "Not an ironpad notebook: missing field `cells`" and nothing is added

#### Scenario: A cell without an order
- **WHEN** the chosen file's only cell has an `id`, `source` and `label` but no `order`
- **THEN** an "Import Failed" toast appears and the browser store holds no new record

### Requirement: Export HTML
The system SHALL export the open notebook from its menu as one self-contained `.html` file named after the title: the title, each cell's label, code cells' source with their last outputs from this session, rendered markdown, inline styles and icons, the footer "Exported from ironpad", and a Content-Security-Policy forbidding scripts and remote loads. (PRD-0003, PRD-0062)

#### Scenario: Nothing is fetched
- **WHEN** a notebook whose cell output a checked checkbox widget is exported
- **THEN** the file holds an inline `<svg>` painted with `currentColor`, no `<link>`, no `@font-face`, and no `http://` or `https://` reference other than the SVG namespace

#### Scenario: Every output is styled in the file
- **WHEN** a notebook whose outputs hold a table panel, a Markdown panel, and every widget kind is exported
- **THEN** every class the file uses has a rule in its own inline stylesheet (the app's stylesheet never reaches it), a Markdown panel is styled like a markdown cell, an icon is sized to its text, and a button or progress widget shows its label rather than `[button widget]`

#### Scenario: Untrusted output is escaped
- **WHEN** a cell's output carries an image whose MIME type is `x" onerror="alert(1)`
- **THEN** the export contains `onerror=&quot;alert(1)` and never the live attribute
- **AND** the document carries `default-src 'none'`

#### Scenario: Outputs that cannot be static
- **WHEN** a cell's output is an animation of 30 frames at 10 fps sized 200×100
- **THEN** the export shows "Animation: 30 frames at 10 fps (200×100)" in its place
