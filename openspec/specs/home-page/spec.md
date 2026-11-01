# home-page Specification

## Purpose
The landing page at `/`: one searchable, filterable grid of every notebook a visitor can open from here, drawn from three storage classes (local notebooks in this browser, the signed-in user's account notebooks, and the bundled public showcase), plus the entry points for creating a notebook and importing one from a file.

## Requirements
### Requirement: Home page layout
The system SHALL serve `/` with the "ironpad" brand link in the header, the heading "Notebooks", the tagline "Interactive Rust notebooks: compile to WebAssembly, run in the browser.", a primary "New Notebook" button, an "Import Notebook" button, a "Search notebooks..." box and filter chips, and no footer status bar. (PRD-0028, PRD-0010)

#### Scenario: Loading the home page
- **WHEN** a visitor requests `/`
- **THEN** the response is 200 and the page shows the "ironpad" brand link, the "Notebooks" heading and the tagline
- **AND** no status bar is rendered at the foot of the page

### Requirement: Creating a notebook
The system SHALL, when "New Notebook" is clicked, create a local notebook titled "Untitled Notebook" with no cells and the default shared dependencies, save it in this browser, and open it at `/local/{id}`; if the save fails the page stays on `/`. (PRD-0016)

#### Scenario: New notebook
- **WHEN** a visitor clicks "New Notebook"
- **THEN** the browser navigates to `/local/{uuid}` and the header shows "Untitled Notebook"
- **AND** returning to `/` lists a card titled "Untitled Notebook" with "0 cells"

### Requirement: Three storage classes in one grid
The system SHALL list in one grid, in this order, the local notebooks in this browser, the signed-in user's account notebooks whether published or not, and the bundled public notebooks, without merging or reconciling the three sources. (PRD-0054, PRD-0064)

#### Scenario: All three present
- **WHEN** a signed-in user with one local notebook and one account notebook opens `/`
- **THEN** the local card comes first, then the account card, then the public cards

#### Scenario: Anonymous visitor
- **WHEN** a visitor who is not signed in opens `/`
- **THEN** no account cards are listed

#### Scenario: Account listing fails
- **WHEN** the account-notebook listing request fails
- **THEN** the account group renders empty and the failure is logged to the browser console

### Requirement: Ordering within each class
The system SHALL order local notebooks by last update, newest first; account notebooks by their last publish or, if never published, their creation, newest first; and public notebooks by title. (PRD-0064)

#### Scenario: Unpublished account notebooks are not sunk
- **WHEN** an account notebook created today has never been published and another was last published a week ago
- **THEN** the unpublished one is listed first

### Requirement: Filter chips
The system SHALL offer chips All, Local, Account and Public, with All selected on load and each other chip limiting the grid to its storage class; the Local and Public chips wear a diamond and the Account chip a cloud, distinct from both account card badges. (PRD-0064)

#### Scenario: One chip, one class
- **WHEN** the visitor selects Account
- **THEN** only account cards remain, published and unpublished alike

### Requirement: Search
The system SHALL filter the grid as the visitor types in the search box, case-insensitively and across every storage class at once, matching public notebooks on title, description or any tag and local and account notebooks on title or any tag. (PRD-0064)

#### Scenario: Tag query spans classes
- **WHEN** a local notebook, an account notebook and a public notebook are each tagged `blog` and the visitor searches "blog"
- **THEN** all three cards are listed, in class order

#### Scenario: Description match is public-only
- **WHEN** the visitor searches "derivatives" and only a public notebook's description contains it
- **THEN** that public card is listed

### Requirement: Local notebook cards
The system SHALL render a local notebook card linking to `/local/{id}` with a filled diamond badge, the title, the cell count ("1 cell" or "N cells"), the last-updated date such as "Aug 12, 2026", and a "Delete notebook" button whose confirmed delete removes the card without a reload. (PRD-0028, PRD-0062)

#### Scenario: Badges tell local from public
- **WHEN** the grid shows a local card beside public cards
- **THEN** the local badge's diamond is filled and the public badge's diamond is an outline

### Requirement: Account notebook cards
The system SHALL render an account notebook card linking to `/mutable/{id}` with the title, cell count and last-activity date (YYYY-MM-DD); a published notebook wears a globe badge and a "published" hint, an unpublished one a muted lock badge and no hint, and account cards carry no delete button. (PRD-0064)

#### Scenario: Unpublished card
- **WHEN** a notebook has just been saved to the account and never published
- **THEN** its card wears the lock badge and shows no "published" hint
- **AND** no card links to its former `/local/{id}`

### Requirement: Public notebook cards
The system SHALL render a public notebook card linking to `/public/{name}` with the file's `.ironpad` extension dropped, an outline diamond badge, the title, the description, the cell count and one pill per tag. (PRD-0048, PRD-0062)

#### Scenario: Extension-less link
- **WHEN** the bundled file `charts-with-plot.ironpad` is listed
- **THEN** its card links to `/public/charts-with-plot`

### Requirement: Loading and empty states
The system SHALL show three skeleton cards while the public list loads, and the lines "No notebooks found." and "Create one to get started!" when the current chip and search leave no card. (PRD-0001)

#### Scenario: Search with no match
- **WHEN** the visitor searches "quaternions" and nothing matches
- **THEN** the grid is replaced by "No notebooks found." and "Create one to get started!"

### Requirement: Home page skips the markdown libraries
The system SHALL not fetch the markdown rendering libraries (KaTeX and Prism) when loading `/`, and SHALL fetch them after a client-side navigation from `/` to a notebook route. (14cc484)

#### Scenario: Navigating to a public notebook
- **WHEN** `/` finishes loading and the visitor clicks a public notebook card
- **THEN** no KaTeX or Prism script was fetched for `/`
- **AND** the KaTeX script is fetched after the navigation
