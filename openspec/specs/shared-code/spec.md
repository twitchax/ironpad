# shared-code Specification

## Purpose
Code and dependencies a notebook shares across all of its cells: the notebook-level shared source (`shared.rs`) and shared Cargo.toml, and shared cells, which narrate pieces of that library inline among the other cells. It covers how authors edit them, how they reach every cell's compilation, how shared cells look and why they never run, and how changing shared code invalidates the cells that use it.

## Requirements
### Requirement: Shared source is the `shared` module
The system SHALL compile a notebook's shared source into every code cell as the module `shared`, so an item it declares `pub` is callable from any code cell as `shared::name`. (PRD-0007, PRD-0044)

#### Scenario: A cell calls a shared function
- **WHEN** a shared cell holds `pub fn double(x: i32) -> i32 { x * 2 }` and a later code cell returns `CellOutput::text(format!("doubled={}", shared::double(21)))`
- **THEN** running the later cell shows `doubled=42`

### Requirement: Shared appendix editors
The system SHALL show below the cells in edit mode two collapsed sections, "Shared Source (shared.rs)" and "Shared Dependencies (Cargo.toml)", each expanding to an editor (Rust, TOML) with a Save button that reads "Saving…" for at least 500 ms, then toasts "Shared source saved" or "Shared dependencies saved" with "Changes will apply on next cell compile." Text typed without Save SHALL be committed by the notebook's own saves and flushes (Ctrl+S, Share, Push, Preview) and by collapsing the section, and never when nothing was typed; while nothing is typed, the editor SHALL follow changes made elsewhere, such as an agent's. (PRD-0007)

#### Scenario: Sections and the settings menu
- **WHEN** a new notebook opens in edit mode
- **THEN** the two sections appear below the cell list, Shared Source first
- **AND** the "Notebook settings" menu mentions neither of them

#### Scenario: Saved shared source survives a reload
- **WHEN** the author expands Shared Source, enters `pub fn from_appendix() -> i32 { 7 }` and clicks Save
- **THEN** the button reads "Saving…" before the toast "Shared source saved" appears
- **AND** after a reload the expanded section shows `from_appendix`

#### Scenario: Ctrl+S saves typed shared source
- **WHEN** the author types in Shared Source and presses Ctrl+S without clicking Save
- **THEN** the stored notebook's shared source contains the typed text

#### Scenario: Preview or collapse keeps typed shared source
- **WHEN** the author types in Shared Source, then switches to Preview and back, or collapses the section, and expands it again
- **THEN** the editor still shows the typed text

#### Scenario: Opening the section writes nothing
- **WHEN** the author expands and collapses Shared Source without typing
- **THEN** the stored notebook still has no shared source

### Requirement: Default shared dependencies
The system SHALL create every new notebook with a shared Cargo.toml declaring `ironpad-cell = "0.1"` and a release profile of `opt-level = 1`, `lto = false` and `codegen-units = 16`, and SHALL prefill an empty shared editor with a commented template (for shared source, an example of a `shared::*` function). (PRD-0016)

#### Scenario: New notebook dependencies
- **WHEN** a notebook is created from the home page
- **THEN** its stored `shared_cargo_toml` holds the `ironpad-cell` dependency and the release profile above, and it has no `shared_source`

### Requirement: Shared changes stale every code cell
The system SHALL mark every runnable code cell stale when shared source or shared dependencies are saved, when a shared cell's source changes, and when a cell is marked or unmarked shared, and SHALL stale nothing when a shared cell is sent the text it already holds. (PRD-0044)

#### Scenario: Editing a shared cell
- **WHEN** a shared cell's source changes from `42` to `pub fn helper() {}`
- **THEN** every runnable code cell in the notebook is marked stale, upstream of the shared cell or not

#### Scenario: A no-op update
- **WHEN** a shared cell is sent the source it already has
- **THEN** no cell is marked stale

#### Scenario: Running inside the save window
- **WHEN** the author edits a shared cell and runs a consuming cell within one second
- **THEN** that run compiles the previous shared source
- **AND** when the edit saves a moment later every code cell is marked stale, so a re-run picks it up

### Requirement: Marking a cell shared
The system SHALL let the author mark a code cell shared, or unmark it, from its cell menu ("Mark as Shared" or "Unmark Shared"), saving the flag with the notebook; markdown and Linux cells cannot be marked shared. (PRD-0044)

#### Scenario: Marking a cell
- **WHEN** the author chooses "Mark as Shared" on a code cell
- **THEN** the cell gains the shared chrome and badge, its "Run cell" button disappears, and the stored cell has `shared: true`

### Requirement: Shared cells never run
The system SHALL never compile or execute a shared cell on its own: it has no Run or Cell settings button, no status badge, stale marker, output toggle or Run All Below item, and Run All, Run All Below, reactive re-runs and page autoruns skip it. (PRD-0044)

#### Scenario: Run All skips the shared cell
- **WHEN** a notebook holds a shared cell followed by a consuming code cell and the author clicks Run All
- **THEN** only the consuming cell runs and reaches success
- **AND** the shared cell never shows a compiling or success status

### Requirement: Shared cell appearance
The system SHALL render a shared cell with an amber left border and tint and a `shared` badge in the editor and in every read-only notebook view, where its source shows in a read-only editor with no run control and no timing, collapsed or open as its saved collapse state says. (PRD-0044)

#### Scenario: Shared cell on a public page
- **WHEN** a public notebook with a shared cell is opened
- **THEN** the cell shows the `shared` badge over a read-only source editor and carries no run button

### Requirement: Shared source assembly order
The system SHALL assemble the shared source a cell compiles against as the notebook-level shared source followed by every shared cell's source in notebook order, separated by one blank line, and SHALL send no shared source at all when neither exists. (PRD-0044)

#### Scenario: Notebook source plus two shared cells
- **WHEN** the notebook-level shared source is `A` and shared cells holding `C` and `B` sit at positions 3 and 1
- **THEN** cells compile against `A`, a blank line, `B`, a blank line, `C`

#### Scenario: Nothing shared
- **WHEN** a notebook has no shared source and no shared cells
- **THEN** cells compile with no shared module content

### Requirement: Shared cells keep their piping slot
The system SHALL count a shared cell in the positional `cellN` numbering with an empty slot, as it does a markdown cell, so marking or unmarking a cell never renumbers the cells after it. (PRD-0044)

#### Scenario: Slot after a shared cell
- **WHEN** cell 0 is shared and cells 1 and 2 are code cells
- **THEN** cell 2 reads cell 1's output as `cell1`, exactly as before cell 0 was marked shared

### Requirement: A shared cell's own manifest is inert
The system SHALL ignore a shared cell's own Cargo.toml when compiling, so dependencies that shared code uses must be declared in the shared Cargo.toml. (PRD-0044)

#### Scenario: Dependency declared on the shared cell
- **WHEN** a shared cell uses a crate declared only in that cell's Cargo.toml tab
- **THEN** a consuming cell that does not declare the crate itself fails to compile until the crate is added to the shared dependencies

### Requirement: Read-only shared appendix
The system SHALL show after the cells of every read-only notebook view, Preview included, a collapsed read-only section for the notebook-level shared source and one for the shared dependencies, each only when its content is not blank, with no Save action. (PRD-0007, e8f295f)

#### Scenario: New notebook in Preview
- **WHEN** a new notebook, which has shared dependencies but no shared source, is shown in Preview
- **THEN** the appendix holds exactly one section, "Shared Dependencies"
- **AND** expanding it shows a read-only editor and no Save button

#### Scenario: Collapsed until opened
- **WHEN** a reader opens `/public/borrows`
- **THEN** two appendix headers read "Shared Source" and "Shared Dependencies" with no body expanded
- **AND** clicking the first mounts a read-only editor
