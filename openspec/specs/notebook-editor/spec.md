# notebook-editor Specification

## Purpose
The editing surface for a notebook its author owns: the `/local/{id}` editor over a browser-local notebook and the owner's editor at `/mutable/{id}` over an account notebook's server draft. It covers adding, editing, reordering and removing cells, the toolbar, menus and keyboard shortcuts, the Edit/Preview toggle, the metadata panel, the app-wide theme, icon set, status bar and toasts, and the cell outline rail that every read-only notebook view (Preview included) carries.

## Requirements
### Requirement: Editor loads a local notebook
The system SHALL serve the editor for a browser-local notebook at `/local/{id}`, showing a two-cell loading skeleton until the notebook is read from the browser's store, then its cells, its title in the header, and its cell count in the status bar. When the store has no such notebook the page SHALL say "This notebook was not found." with the hint that local notebooks live only in the browser that created them, and when the read fails or the record is not a notebook it SHALL say "This notebook could not be read.", each with a link home; a notebook with no cells shows "This notebook is empty" and the hint "Add a Code or Markdown cell above to get started." (PRD-0048, PRD-0034)

#### Scenario: A new notebook opens empty
- **WHEN** a user clicks "New Notebook" on the home page
- **THEN** the browser navigates to `/local/{uuid}` and the editor shows zero cell cards
- **AND** the empty-notebook message is shown below an add-cell row

#### Scenario: Switching notebooks without a page load
- **WHEN** the URL changes from `/local/a` to `/local/b` by client-side navigation
- **THEN** the editor remounts and shows notebook `b`, never notebook `a` under `b`'s URL

#### Scenario: A notebook from another browser
- **WHEN** a `/local/{uuid}` link is opened in a browser that has no notebook with that id
- **THEN** the page says "This notebook was not found." and shows no skeleton

### Requirement: Adding cells
The system SHALL render an add-cell row with Code, Markdown and Linux buttons above the first cell and after every cell; a click inserts a cell of that kind at that position labelled "Cell N", where N is the cell count before the insert, scrolls it into view, focuses its code editor if it has one, and persists the notebook. (PRD-0001, PRD-0066)

#### Scenario: First code cell
- **WHEN** the author clicks Code in an empty notebook
- **THEN** one code cell labelled "Cell 0" appears with the source `42` in a Monaco editor
- **AND** its Cargo.toml tab holds `[dependencies]`, the same for every new cell

#### Scenario: Markdown and Linux seeds
- **WHEN** the author clicks Markdown
- **THEN** the new cell holds `# New Section` followed by `Add your notes here.` and shows it rendered
- **AND** clicking Linux instead (tooltip "A whole Rust program run as a Linux process in the browser") seeds a `fn main()` that prints "Hello from Linux."

#### Scenario: Inserting between cells
- **WHEN** the add-cell row after the second of three cells is used
- **THEN** the new cell becomes the third cell and the order badges of the cells after it shift by one

### Requirement: Cell header
The system SHALL show in each cell's header a collapse chevron, an order badge `[N]` giving the cell's live position, a label input committed on blur when its text changed, and a type badge on markdown (`¶ markdown`), shared (`shared`) and Linux (`linux`) cells. (PRD-0001, PRD-0032)

#### Scenario: Badges follow a reorder
- **WHEN** the first two cells swap places
- **THEN** their badges read `[0]` and `[1]` in the new order without a reload

#### Scenario: Renaming a cell
- **WHEN** the author edits a cell's label and clicks elsewhere
- **THEN** the new label is saved with the notebook

### Requirement: Cell action rail
The system SHALL place each cell's actions in a narrow rail beside its card: a "Drag to reorder" handle on every cell except markdown, "Run cell" and "Cell settings (Cargo.toml)" buttons on code cells that run in the editor, and a "Cell menu" button; the rail is dimmed until its row is hovered or its cell is active. (PRD-0005, PRD-0034)

#### Scenario: Discoverable on touch
- **WHEN** a notebook is shown and no cell is hovered
- **THEN** each rail is visible at reduced opacity
- **AND** hovering a row or clicking its cell raises that rail to full opacity

#### Scenario: Cell settings switches tabs
- **WHEN** the author clicks "Cell settings (Cargo.toml)" on a code cell
- **THEN** the cell body shows its Cargo.toml tab, and a second click returns it to the Code tab

#### Scenario: Markdown rail
- **WHEN** the cell is a markdown cell
- **THEN** its rail holds only the "Cell menu" button

### Requirement: Cell menu
The system SHALL offer in each cell's menu Move Up and Move Down (disabled at the list's ends), Duplicate and Delete, plus, on code cells only, Run All Below (hidden while the cell is shared) and Mark as Shared or Unmark Shared. (PRD-0001, PRD-0044)

#### Scenario: Boundary moves
- **WHEN** the menu is opened on the first cell
- **THEN** Move Up is disabled and Move Down is enabled

#### Scenario: Duplicate
- **WHEN** the author duplicates a cell labelled "Setup"
- **THEN** a cell labelled "Setup (copy)" with the same type, source, Cargo.toml and shared flag appears directly below it and takes focus

#### Scenario: Cells that cannot run here
- **WHEN** the menu is opened on a markdown or Linux cell
- **THEN** it offers neither Run All Below nor Mark as Shared

### Requirement: Deleting a cell
The system SHALL delete a cell only after the browser confirm "Delete this cell? This cannot be undone.", and SHALL then drop the cell's outputs and any queued run of it and persist the notebook. (PRD-0001, PRD-0032)

#### Scenario: Dismissed confirm
- **WHEN** the author chooses Delete and dismisses the confirm
- **THEN** the cell remains

#### Scenario: Deleting a queued cell
- **WHEN** a cell waiting in a Run All queue is deleted
- **THEN** the cells queued behind it still run instead of sticking on "queued"

#### Scenario: Deleting a cell mid-check
- **WHEN** a cell is deleted while its live check or compile request is still in flight
- **THEN** the page logs no "already been disposed" error and keeps working

### Requirement: Drag-and-drop reordering
The system SHALL let the author reorder cells by dragging a cell's handle within the cell list in edit mode, persisting the new order on drop; markdown cells, which carry no handle, move through their menu. (PRD-0008, PRD-0034)

#### Scenario: Drag a cell to the top
- **WHEN** the third cell's handle is dragged above the first cell and released
- **THEN** the cells read in the order third, first, second, the order badges renumber, and the new order is saved

#### Scenario: Appendix is not a drop target
- **WHEN** a cell is dragged toward the shared and metadata sections below the list
- **THEN** it can only be dropped among cells

### Requirement: Structural edits keep other cells' state
The system SHALL keep every other cell's run status, output and diagnostics when a cell is added, deleted or reordered, and SHALL never move focus out of the editor being typed in when an autosave lands. (PRD-0032, PRD-0002, PRD-0006)

#### Scenario: Adding a cell keeps an output
- **WHEN** a code cell with source `42` has run successfully and shows `42`, and the author adds a second code cell
- **THEN** the first cell still shows its success status and the output `42`

#### Scenario: Typing through an autosave
- **WHEN** the author keeps typing past the one-second save debounce
- **THEN** keystrokes continue into the same editor with no lost focus

### Requirement: Code and Cargo.toml panes
The system SHALL give every non-markdown cell a "Code" tab with a Rust editor and a "Cargo.toml" tab with a TOML editor, and SHALL mark a tab with an "Unsaved changes" dot from the first keystroke until its text reaches the notebook one second after typing stops. (PRD-0001, PRD-0005)

#### Scenario: Dirty dot clears after the debounce
- **WHEN** the author types into a cell's Code tab and stops
- **THEN** the Code tab shows the unsaved-changes dot
- **AND** about one second later the dot disappears and the edit is part of the saved notebook

### Requirement: Editors size to their content
The system SHALL size every Monaco editor in the app to its content, no shorter than 60 and no taller than 600 pixels, scrolling internally only past 600 pixels, with word wrap on and no minimap. (PRD-0006)

#### Scenario: Short and long cells
- **WHEN** one cell holds a single line and another holds 200 lines
- **THEN** the first editor is about 60 pixels tall and the second stops at 600 pixels and scrolls

### Requirement: Markdown cells
The system SHALL render a markdown cell as formatted HTML (tables, strikethrough, task lists, math spans, fenced code with its language class) with raw HTML's active parts removed, switch it to a markdown editor on double-click, save what is typed there one second after typing stops (as a code cell's editor does), and close the editor on Escape, on a click outside it, on Ctrl+S and on entering Preview; Monaco's own find widget, command palette and context menu open inside the editor and do not close it. (PRD-0032)

#### Scenario: Editing a markdown cell
- **WHEN** the author double-clicks a rendered markdown cell
- **THEN** a markdown editor replaces it with the hint "Press Escape or click away to save"
- **AND** an empty markdown cell instead reads "Double-click to edit markdown…"

#### Scenario: Typing saves with the editor still open
- **WHEN** the author types into an open markdown editor and waits two seconds without closing it
- **THEN** the stored notebook holds the typed text, and after a reload the cell shows it rendered

#### Scenario: Find inside a markdown editor
- **WHEN** the author presses Ctrl+F in an open markdown editor
- **THEN** the find widget opens and the editor stays open
- **AND** the first Escape closes the find widget, the second closes the editor, and the cell keeps its text

#### Scenario: Reopening a markdown cell
- **WHEN** the author opens and closes a markdown cell's editor several times
- **THEN** each reopening behaves like the first, and Escape still closes it

#### Scenario: Saving before the editor has loaded
- **WHEN** the author opens a markdown cell and presses Ctrl+S before the editor has finished loading
- **THEN** the cell keeps its text

#### Scenario: Preview commits an open edit
- **WHEN** a markdown cell is open in its editor and the author clicks Preview
- **THEN** the edit is committed and Preview shows it rendered, with no markdown editor left on the page

#### Scenario: Raw HTML is neutralised
- **WHEN** a markdown cell contains `<script>steal()</script>` and `<img src=x onerror="steal()">`
- **THEN** the rendered cell contains neither the script, the `onerror` attribute nor the payload

### Requirement: Per-cell collapse defaults
The system SHALL offer on every non-markdown cell in edit mode a code toggle and, except on shared cells, an output toggle that set whether that part loads collapsed; each is saved on the cell and honoured by every surface, while the header chevron collapses or opens the code for the current view only. (effd30e)

#### Scenario: Collapsed code is saved
- **WHEN** the author clicks a code cell's code toggle (tooltip "Code loads open. Click to load it collapsed.")
- **THEN** the cell body collapses, the stored cell gets `collapsed: true`, and after a reload the cell loads collapsed

#### Scenario: Preview honours the saved state
- **WHEN** that notebook is shown in Preview
- **THEN** the cell loads collapsed, its chevron opens it to a read-only editor, and no collapse toggles are shown

#### Scenario: Collapsed output is saved
- **WHEN** a cell has run and the author clicks its output toggle
- **THEN** the output panel collapses and the stored cell gets `output_collapsed: true`

### Requirement: Keyboard shortcuts
The system SHALL bind, inside a cell's code editor, Ctrl/Cmd+Enter to run the cell and Shift+Enter to run it and move focus to the next cell's code editor, and on the editor page Ctrl/Cmd+Shift+Enter to run all code cells and Ctrl/Cmd+Shift+N to add a code cell after the active cell. Those two page shortcuts SHALL do nothing in Preview. (PRD-0001)

#### Scenario: Run the focused cell
- **WHEN** the author clicks into a code cell's editor and presses Ctrl+Enter
- **THEN** the cell's status turns to compiling and later to success or error

#### Scenario: Run all from the keyboard
- **WHEN** focus is on a cell header and the author presses Ctrl+Shift+Enter in a notebook with two code cells
- **THEN** the first cell starts compiling and then reaches success or error

#### Scenario: Add a cell with no active cell
- **WHEN** no cell has been clicked yet and the author presses Ctrl+Shift+N
- **THEN** a code cell with source `42` is inserted at the top of the notebook and focused

#### Scenario: Page shortcuts in Preview
- **WHEN** the author presses Ctrl+Shift+N and Ctrl+Shift+Enter in Preview, then returns to Edit
- **THEN** the stored notebook has the same cells and no cell starts running

### Requirement: Explicit save
The system SHALL treat Ctrl/Cmd+S on the editor page as an explicit save that suppresses the browser's save dialog, commits every cell's unsaved edits, and persists the notebook (without embedding outputs); once the write lands it SHALL record the save time for the status bar and toast "Notebook saved", and when it does not land it SHALL toast "Not Saved" instead, except for a refused draft, whose own toast has already said why. A panel's Save button SHALL report the same way. There is no notebook-level save button. (PRD-0001, PRD-0006)

#### Scenario: Saving after a run
- **WHEN** a cell has run successfully and the author presses Ctrl+S
- **THEN** the toast "Notebook saved" appears and the stored notebook's `updated_at` changes
- **AND** the cell is not marked stale by the save

#### Scenario: The write does not land
- **WHEN** the browser store refuses writes, or an account notebook's draft saves fail, and the author presses Ctrl+S or a panel's Save
- **THEN** a "Not Saved" toast appears and neither "Notebook saved" nor the panel's saved toast does

#### Scenario: Autosaves the browser refuses
- **WHEN** the browser store refuses a local notebook's autosaves
- **THEN** one "Not Saved" toast appears for the first refusal, and the next appears only after a write has landed again

### Requirement: Flush before reading the notebook
The system SHALL commit every cell's unsaved editor text into the notebook before Share, Save to Account, Push, Export HTML, Download .ironpad, Restore or Preview reads it, so each includes text typed inside the last one-second save window, and SHALL persist the notebook whenever that commit moved text. (PRD-0032)

#### Scenario: Restore captures the last second of typing
- **WHEN** the author types `let typed_inside_the_debounce = 2;` and immediately restores a history snapshot
- **THEN** the pre-restore snapshot in history contains `typed_inside_the_debounce`

#### Scenario: Preview saves the last second of typing
- **WHEN** the author types into a code cell and clicks Preview within a second
- **THEN** the stored notebook holds the typed text half a second later

#### Scenario: Previewing a clean notebook writes nothing
- **WHEN** the owner of a clean published notebook switches to Preview and back
- **THEN** the Push button stays disabled

### Requirement: Notebook title editing
The system SHALL show the open notebook's title in the header and, when clicked, an input with the title selected, where Enter or blur saves a changed title and Escape restores the previous one; at widths of 480 pixels or less the title stays in the header, truncated. (PRD-0001, PRD-0032, PRD-0034)

#### Scenario: Rename survives a reload
- **WHEN** the author clicks the title, enters "Persisted Rename" and presses Enter
- **THEN** after reloading `/local/{id}` the header still reads "Persisted Rename"

#### Scenario: Typing accumulates
- **WHEN** the author clears the title input and types "My Great Notebook" one key at a time
- **THEN** the input holds "My Great Notebook"

### Requirement: Editor toolbar
The system SHALL show above the cells a toolbar holding, in edit mode only, a "Run All" button (tooltip "Run all code cells (Ctrl+Shift+Enter)"), the agent-session button and a "Notebook settings" menu, and in both modes a "Notebook menu" and a "Back to notebook list" button that navigates to `/`. (PRD-0005, PRD-0007)

#### Scenario: Toolbar in Preview
- **WHEN** the author switches to Preview
- **THEN** the "Notebook menu" and "Back to notebook list" buttons remain
- **AND** "Notebook settings" and "Run All" are gone, and Share Immutable from the menu still surfaces a `/shared/` link

### Requirement: Toolbar menus
The system SHALL keep at most one toolbar menu open, close it on any click outside it, and keep it open when one of its toggle items is clicked; the settings menu lists "Force Recompile" and "Reactive Mode" toggles that show a trailing check while on, and Reactive Mode is saved with the notebook. (PRD-0005, PRD-0025, PRD-0062)

#### Scenario: Toggling keeps the menu open
- **WHEN** the author opens "Notebook settings" and clicks "Force Recompile"
- **THEN** the menu stays open with a check after the item
- **AND** a click elsewhere in the editor closes it

### Requirement: Notebook menu contents
The system SHALL list in the notebook menu, for a local notebook, Share Immutable, Export HTML, Download .ironpad, History and Delete, plus Save to Account and Share Mutable for signed-in users; for an account notebook, Share Immutable, Export HTML, Download .ironpad and Delete, plus View Published, Discard Draft and Unpublish once a published copy exists. (PRD-0005, PRD-0054, PRD-0058, PRD-0064)

#### Scenario: Anonymous local notebook
- **WHEN** a visitor who is not signed in opens a local notebook's menu
- **THEN** it offers no Save to Account item

#### Scenario: Unpublished account notebook
- **WHEN** the owner opens the menu on an account notebook that has never been published
- **THEN** it offers neither View Published, Discard Draft, Unpublish nor History

### Requirement: Edit and Preview toggle
The system SHALL show a fixed bottom-left toggle with "Edit" and "Preview" segments marking the active mode; Preview commits pending edits, cancels the editor's run queue (reporting it to agents as cancelled, the running cell included), then renders a snapshot of the notebook with the read-only renderer the public pages use, without a Fork button, and runs every runnable cell. (PRD-0005, PRD-0006, PRD-0032)

#### Scenario: Entering Preview
- **WHEN** the author clicks Preview
- **THEN** the add-cell rows, cell rails, collapse toggles and appendix editors disappear and every code editor shown is read-only
- **AND** every runnable cell runs

#### Scenario: A run in flight when Preview opens
- **WHEN** the author clicks Preview while Run All is compiling the first of two cells, then returns to Edit
- **THEN** neither cell starts again and no cell shows "queued"

#### Scenario: Preview is a snapshot
- **WHEN** an agent edits a cell while the author is in Preview
- **THEN** the change appears in Preview only after switching to Edit and back

#### Scenario: Preview of an account notebook
- **WHEN** the owner of a published account notebook clicks Preview
- **THEN** it renders the current draft, not the published copy (tooltip "Preview: how this notebook looks to a reader, using your current content")

### Requirement: Publish button for account notebooks
The system SHALL show, in the owner's editor of an account notebook in edit mode, one editorial button reading "Publish" (never published, armed), "Push" (published, with draft changes, armed) or "Published" (draft matches the published copy, disabled), with the draft-save indicator after it. (PRD-0054, PRD-0064)

#### Scenario: Never published
- **WHEN** the owner opens an account notebook saved from a local one
- **THEN** the button reads exactly "Publish" and is enabled

#### Scenario: Edit after publishing
- **WHEN** a published, clean notebook ("Published", disabled) is renamed
- **THEN** the button reads "Push" and is enabled

#### Scenario: Local notebooks have no such button
- **WHEN** a `/local/{id}` notebook is open
- **THEN** no Publish, Push or Published button is shown

### Requirement: Draft autosave indicator
The system SHALL autosave an account notebook's draft 1.5 seconds after the last edit and label the indicator "Saving draft…" during a save, "Draft not saved; retrying" after a failure (retried after 5 seconds), and nothing once synced. When the server refuses the save the indicator SHALL name why: "Draft not saved: storage full" for a storage cap, "Draft not saved: notebook too large" for the 4 MiB upload cap, "Draft not saved: not in your account" when the account no longer owns the notebook, and "Draft not saved: signed out" without a session. Only the signed-out refusal is retried. (PRD-0054, PRD-0064)

#### Scenario: Network failure
- **WHEN** draft saves are blocked at the network and the owner renames the notebook
- **THEN** the indicator reads "Draft not saved; retrying" and stays to the right of the Push button

#### Scenario: Storage cap refusal
- **WHEN** the server refuses a draft save because the account is full
- **THEN** the indicator reads "Draft not saved: storage full", the save is not retried, and one "Draft Not Saved" toast (shown 15 seconds) repeats the server's limit and suggests deleting a notebook after downloading it
- **AND** later edits that are refused again keep the indicator on "Draft not saved: storage full" while they save, and show no further toast until a save succeeds

#### Scenario: Signed out in another tab
- **WHEN** the owner signs out in another tab and keeps typing
- **THEN** the indicator reads "Draft not saved: signed out" and one "Draft Not Saved" toast says to sign in again from a new tab without closing this one
- **AND** after signing back in, the next retry saves the draft and the indicator clears

### Requirement: Status bar
The system SHALL show a status bar at the foot of every page except `/` and `/embed/*` reading "Ready", the cell toolchain (tooltip "Cell compiler toolchain") and "Cells: N", plus "Saved: <age>" after an explicit save, refreshed every 30 seconds. (PRD-0028, PRD-0065)

#### Scenario: Age after a save
- **WHEN** the author presses Ctrl+S
- **THEN** the status bar shows "Saved: just now", later "Saved: 12s ago", "Saved: 3m ago", "Saved: 2h ago" or "Saved: 1d ago"

#### Scenario: Leaving the editor
- **WHEN** the author navigates from the editor to a read-only notebook page without a reload
- **THEN** that page's status bar shows no "Saved:" item

#### Scenario: Cell count on a read-only page
- **WHEN** a reader opens `/public/charts-with-plot`
- **THEN** the status bar shows "Ready" and "Cells:" with a non-zero count

### Requirement: Light and dark theme
The system SHALL default to the dark theme and offer "Dark mode" and "Light mode" buttons in the header of every non-embed page; the choice is stored in browser local storage under `ironpad-theme`, applied before first paint on later loads, and applied to every Monaco editor, including editors created later; applying it never loads the Monaco editor bundle on a page that has no editor. (PRD-0008, PRD-0034)

#### Scenario: Light theme survives a reload
- **WHEN** the user clicks "Light mode" and reloads
- **THEN** the page renders light from first paint and the code editors use the light editor theme

#### Scenario: New editors keep the light theme
- **WHEN** the light theme is stored and the author opens a notebook with code cells and adds another
- **THEN** every editor, old and new, uses the light editor theme

#### Scenario: The home page loads no editor
- **WHEN** the light theme is stored and the user opens `/`
- **THEN** Monaco's `editor.main` bundle is not requested

#### Scenario: No system-preference detection
- **WHEN** a first-time visitor's operating system prefers a light appearance
- **THEN** the app still opens in the dark theme

### Requirement: Shipped icon set
The system SHALL draw every UI affordance icon as shipped inline SVG sized to the surrounding text and painted in its colour, never as a Unicode symbol or emoji character, hiding icons from assistive technology and naming icon-only controls through the control's `title` or `aria-label`. (PRD-0062)

#### Scenario: Icon-only control
- **WHEN** a screen reader reaches the notebook menu button
- **THEN** it announces "Notebook menu" from the button, not from the icon

#### Scenario: Status badge
- **WHEN** a cell is compiling
- **THEN** its status badge shows a spinner icon beside the text "compiling…"

### Requirement: Notebook metadata panel
The system SHALL offer below the cells, in edit mode, a collapsed "Notebook Metadata (link previews)" section with Description, comma-separated Tags and an optional preview-image path, saved together by a Save button that reads "Saving…" for at least 500 ms and then toasts "Notebook metadata saved"; a blank field clears its value. (PRD-0051)

#### Scenario: Description and tags persist
- **WHEN** the author enters "Numerically integrating a cannonball." and "simulation, autodiff" and saves
- **THEN** the stored notebook has that description and the tags `["simulation", "autodiff"]`
- **AND** reopening the panel after a reload shows both values

#### Scenario: Clearing a field
- **WHEN** a saved description is emptied and the panel is saved again
- **THEN** the stored notebook has no description

### Requirement: Preview image validation
The system SHALL flag, while typing, a preview-image path that does not start with a single "/", and SHALL show width and height fields only while a path is set, requiring both or neither, each a whole number from 200 to 8192 pixels; saving without a path also clears the size. (PRD-0051)

#### Scenario: Off-origin image
- **WHEN** the author types `https://evil.example/x.png`
- **THEN** a problem message explains the path must be root-relative
- **AND** changing it to `/og-custom/mine.png` removes the message and shows two size fields

#### Scenario: No override
- **WHEN** the preview-image field is empty
- **THEN** no size fields are shown

### Requirement: Account notebook address in the metadata panel
The system SHALL show, in an account notebook's metadata panel, its `/mutable/{id}` address with a copy button, labelled "Published at" once published and otherwise "Notebook link (not published)" with the hint "Only you can open this. Publish to make the link work for anyone else."; the access controls appear only once it is published. (PRD-0061, PRD-0064)

#### Scenario: Unpublished account notebook
- **WHEN** the owner opens the metadata panel of a never-published account notebook
- **THEN** the panel shows the full `/mutable/{id}` URL under "Notebook link (not published)", never the words "Published at", and no Access controls

### Requirement: Phone widths and touch targets
The system SHALL keep the home page and read-only notebook pages free of horizontal scrolling at a 390-pixel-wide viewport, and SHALL make every visible button and link at least 44 by 44 pixels on coarse-pointer devices. (b0d9876)

#### Scenario: No sideways scroll on a phone
- **WHEN** `/`, `/public/charts-with-plot` and `/public/welcome` load at 390 by 844 pixels
- **THEN** none of them is wider than the viewport

#### Scenario: Touch targets
- **WHEN** `/public/charts-with-plot` loads on a touch device
- **THEN** no visible button, link or drag handle outside rendered prose is smaller than 44 by 44 pixels

### Requirement: Reduced motion
The system SHALL stop the interface's decorative animations (loading skeletons, the running status, the pending-re-execution marker and toasts) when the user prefers reduced motion, while leaving animations inside cell output running. (b0d9876)

#### Scenario: Chrome stops, output moves
- **WHEN** a user who prefers reduced motion opens `/` and then `/public/double-pendulum`
- **THEN** the skeleton, running-status, pending-marker and toast animations are off
- **AND** the notebook's animated output still animates

### Requirement: Toast notifications
The system SHALL report workflow outcomes as toasts in a bottom-right overlay, each with a success, error or info title and a body, dismissed automatically after its own timeout, and SHALL show a toast queued just before a full page reload once the next page loads. (ea04dcc)

#### Scenario: Toast across a reload
- **WHEN** a history Restore finishes by reloading the page
- **THEN** the reloaded editor shows the "Restored" toast

### Requirement: Agent edits reach the open editor
The system SHALL show a session agent's change to a cell's source or Cargo.toml in that cell's editor pane without a reload, except in a pane holding the author's own unsaved text, which keeps the author's text. (PRD-0035, 25022ad agentic-collaboration/0007)

#### Scenario: Agent edits an idle cell
- **WHEN** an agent updates the source of a cell the author is not editing
- **THEN** the cell's Code tab shows the new source and the next run compiles it

#### Scenario: Agent edits a cell mid-typing
- **WHEN** an agent updates a cell whose Code tab shows the unsaved-changes dot
- **THEN** the Code tab keeps the author's text
