# account-notebooks Specification

## Purpose
Server-stored notebooks for signed-in users, each at one stable address, `/mutable/{id}`. A notebook in an account is private to its owner until published; the owner edits a server-side draft from any signed-in browser, and an explicit Publish or Push promotes it to the copy readers see. Covers Save to Account, Share Mutable, the reader-versus-owner page, the draft lifecycle, privacy and READ grants, denial on every read surface, storage quotas, and the owner's listing.

## Requirements
### Requirement: Save to Account
For a signed-in user, the local editor's notebook menu SHALL offer "Save to Account", which after a confirm uploads the notebook into the user's account unpublished, deletes the browser's local copy, and hard-navigates to `/mutable/{id}`. The item SHALL NOT appear for anonymous users. (PRD-0064)

#### Scenario: Move, never copy
- **WHEN** a signed-in user saves a local notebook to their account and accepts the confirm
- **THEN** the local IndexedDB record is gone and the page is at `/mutable/{id}` in the owner's editor
- **AND** a "Saved to Your Account" toast says "Only you can see it until you publish it."
- **AND** the content survives a reload after the browser's whole `ironpad` IndexedDB database is deleted

#### Scenario: The confirm names the cost
- **WHEN** the user picks Save to Account
- **THEN** the confirm says the browser copy is deleted, that its local version history does not follow it, and that it stays private until published
- **AND** dismissing it leaves the URL, the local record, and the server untouched, with no "Saving…" toast

#### Scenario: Anonymous menu
- **WHEN** an anonymous user opens the notebook menu once auth has resolved
- **THEN** "Share Immutable" is listed and "Save to Account" is not
- **AND** after signing in, the same notebook's menu lists "Save to Account"

#### Scenario: Local delete fails
- **WHEN** the upload succeeds but the local record survives the delete
- **THEN** the toast after navigation is "Saved, With a Leftover Copy", asking the user to delete the local copy from the home page

#### Scenario: No local history in the account editor
- **WHEN** the owner opens the notebook menu of an account notebook
- **THEN** no "History" item is listed, since version history is a local-notebook feature that does not follow the notebook to the server

### Requirement: Share Mutable
The local editor's menu SHALL offer "Share Mutable", which saves the notebook into the signed-in user's account and publishes it in one act, deletes the local copy, copies the `/mutable/{id}` URL to the clipboard, and hard-navigates there. If the publish half fails, the server SHALL keep nothing and the local copy SHALL remain. (PRD-0049, PRD-0054, PRD-0056, PRD-0064)

#### Scenario: Publish from local
- **WHEN** a signed-in user picks Share Mutable
- **THEN** a "Publishing…" toast appears, then after navigation a "Published! Link Copied" toast with the URL
- **AND** the editor's button reads "Published" and is disabled

#### Scenario: Outputs are published with it
- **WHEN** a user runs a cell and then picks Share Mutable
- **THEN** the published copy carries that cell's output panels as its saved output, subject to the 256 KiB per-cell limit

#### Scenario: Anonymous attempt
- **WHEN** an anonymous user picks Share Mutable
- **THEN** a toast says "sign in with GitHub to save or publish notebooks" and the local notebook is untouched

#### Scenario: Publish failure rolls back
- **WHEN** the save half succeeds and the publish half fails
- **THEN** the call fails with "publish failed; the notebook was not saved"
- **AND** the owner's account lists nothing new, no bytes are charged to either quota, and retrying does not accumulate orphans

### Requirement: One stable address
Each account notebook SHALL get a server-minted random id of 16 lowercase hex characters, and `/mutable/{id}` SHALL stay its address through save, publish, push, and unpublish until it is deleted. The owner's metadata panel SHALL show that address with a copy button, labelled "Published at" when published and "Notebook link (not published)" otherwise. (PRD-0049, PRD-0064)

#### Scenario: Whole lifecycle at one URL
- **WHEN** an owner saves a notebook to their account, publishes it, unpublishes it, and deletes it
- **THEN** every step before the delete happens at the same `/mutable/{id}`
- **AND** after the delete that address answers 404

#### Scenario: Unpublished address row
- **WHEN** the owner opens the metadata panel of an unpublished notebook
- **THEN** the row reads "Notebook link (not published)" with the hint "Only you can open this. Publish to make the link work for anyone else." and the panel never says "Published at"
- **AND** after publishing, the same row reads "Published at" and the hint is gone

### Requirement: Reader for everyone, editor for the owner
`/mutable/{id}` SHALL server-render the view-only reader of the published copy for every request, never the draft, and after hydration SHALL replace it with the editor over the server draft only for the holder of the OWNER grant. `?view=reader` SHALL pin the reader, with an "Edit" link back for the owner. (PRD-0054, PRD-0053)

#### Scenario: Owner, stranger, anonymous
- **WHEN** the owner, a different signed-in user, and an anonymous visitor open the same published `/mutable/{id}`
- **THEN** only the owner ends up in the editor with the toolbar's publish button
- **AND** the other two see the reader with no publish button

#### Scenario: Drafts never reach the SSR body
- **WHEN** the owner has renamed a published notebook "Lifecycle One" to "Lifecycle One v2" and the draft has saved
- **THEN** a fresh anonymous reader sees "Lifecycle One" and the raw response's `og:title` is "Lifecycle One"

#### Scenario: Attribution
- **WHEN** anyone reads a published notebook
- **THEN** the page shows "Published by @{login}" linking to `https://github.com/{login}`, with the owner's avatar when one exists

#### Scenario: View Published
- **WHEN** the owner of a published notebook picks "View Published" from the menu
- **THEN** the page moves to `/mutable/{id}?view=reader` and shows the reader with no publish button
- **AND** its "Edit" link returns the owner to the editor

#### Scenario: Never cached
- **WHEN** any client fetches `/mutable/{id}`
- **THEN** the response carries `Cache-Control: no-cache`, since the address resolves to whatever was last published

### Requirement: The owner's first paint of an unpublished notebook
A notebook with no published copy SHALL answer `/mutable/{id}` with HTTP 404 for everyone, its owner included. A request that carries a session cookie SHALL paint a neutral "Loading notebook..." until ownership resolves; anyone else SHALL see "This mutable notebook was not found." with the hint "The link may be wrong, or the author may have unpublished it." (PRD-0064)

#### Scenario: Owner
- **WHEN** the owner loads their unpublished notebook's URL
- **THEN** the raw response is 404 and contains "Loading notebook" but not the not-found copy
- **AND** the editor swaps in without the not-found copy ever appearing in the DOM

#### Scenario: Anonymous
- **WHEN** an anonymous visitor loads the same URL
- **THEN** the raw response is 404 with the not-found copy, never the placeholder, and it does not change after hydration

#### Scenario: Signed-in stranger
- **WHEN** a different signed-in user loads the same URL
- **THEN** they see the placeholder until the ownership check fails, then the not-found copy, and never the editor

#### Scenario: Pinned reader
- **WHEN** the owner loads the unpublished notebook with `?view=reader`
- **THEN** the not-found copy shows immediately

### Requirement: Draft autosave
In the owner's editor, edits SHALL autosave to the server draft after a 1.5 s debounce, coalescing a burst into one write, or at once when the owner leaves the editor before it fires, and SHALL not change what readers see until Publish or Push. Leaving without unsaved edits SHALL save nothing. The indicator after the publish button SHALL read "Saving draft…" during a save and "Draft not saved; retrying" after a failed one, which retries after 5 s. Concurrent drafts are last-write-wins. (PRD-0054)

#### Scenario: Same draft on every device
- **WHEN** the owner renames a published notebook to "Device One WIP" on one device and opens its URL on a second device signed in to the same account
- **THEN** the second device's editor shows "Device One WIP" with Push armed
- **AND** another account still reads the published title until a Push

#### Scenario: Leaving inside the debounce
- **WHEN** the owner edits a cell and clicks "Back to notebook list" before the draft autosave fires
- **THEN** the draft is saved anyway, and reopening the notebook shows the edit

#### Scenario: Leaving a clean notebook
- **WHEN** the owner opens a clean published notebook and leaves
- **THEN** no draft is saved and the button still reads "Published"

#### Scenario: Network failure
- **WHEN** draft saves fail at the network layer
- **THEN** the indicator reads "Draft not saved; retrying" and the save is retried

#### Scenario: Two writers
- **WHEN** two devices edit the same draft
- **THEN** whichever autosave lands last is the draft, with no merge

### Requirement: Publish and Push
The owner's toolbar SHALL carry one editorial button: "Publish" (enabled) while no published copy exists, "Push" (enabled) while the draft differs from the published copy, and "Published" (disabled) when they match. Clicking it SHALL promote the server-held draft to the published copy without re-uploading the notebook. (PRD-0054, PRD-0064)

#### Scenario: First publish
- **WHEN** the owner of an unpublished notebook clicks "Publish"
- **THEN** a "Published" toast says "Your notebook is readable at its link now."
- **AND** the button reads "Published" and is disabled, and an anonymous visitor gets 200 and the notebook at its URL

#### Scenario: Push an edit
- **WHEN** the owner edits a published notebook and clicks "Push"
- **THEN** a "Pushed" toast appears, the button disables again, and the raw response's `og:title` becomes the new title

#### Scenario: Nothing left to promote
- **WHEN** another device promotes the draft between this device's pre-push save and its promote
- **THEN** the promote reports nothing to do and an "Up to Date" toast says "The published copy already matches your draft."

### Requirement: Push never publishes stale content
Before promoting, Publish and Push SHALL flush in-progress cell edits and durably save the current draft, embedding each cell's latest-run output panels (debounced autosaves never embed them). When that save does not land, the action SHALL publish nothing and show a "Push Failed" toast saying why. (PRD-0054, PRD-0056)

#### Scenario: A keystroke inside the debounce window
- **WHEN** the owner types into a cell and clicks Push immediately
- **THEN** the published copy shown by View Published contains that keystroke

#### Scenario: Draft save fails
- **WHEN** draft saves are failing and the owner clicks Push
- **THEN** a "Push Failed" toast appears, the button stays armed as "Push", and readers still get the old title
- **AND** once saves succeed again the same Push publishes the edit

#### Scenario: Out of storage
- **WHEN** the draft save is refused for storage and the owner clicks Push
- **THEN** the toast says the account is out of storage, nothing was published, and to delete a notebook from the account to free space

### Requirement: Blob snapshot on publish
Each Publish or Push SHALL copy the compile-cache hits of the promoted draft into the shared `/share-blobs/` store and SHALL replace the notebook's blob manifest with exactly those entries; draft autosaves do no blob work. A notebook whose snapshot finds no hits or fails SHALL still publish, with no manifest. (PRD-0049, PRD-0054)

#### Scenario: Cold cache
- **WHEN** a notebook is published while none of its cells is in the compile cache
- **THEN** it publishes and `get_mutable_manifest` returns none, so readers compile live

#### Scenario: Replace, not merge
- **WHEN** a notebook is pushed again after a cell changed
- **THEN** the manifest names only the blobs of the cells as just promoted

### Requirement: Discard Draft
For a published notebook, the owner's menu SHALL offer "Discard Draft", which after the confirm "Discard your draft and return to the published copy? Unpushed changes will be lost." clears the draft and reloads the editor on the published copy. For a notebook with no published copy it SHALL NOT be offered, and the server SHALL decline it. (PRD-0054, PRD-0064)

#### Scenario: Discard an edit
- **WHEN** the owner drafts a rename and discards it
- **THEN** after the reload a "Draft Discarded" toast says "Back to the published copy."
- **AND** the editor shows the published title with the button disabled as "Published"

#### Scenario: Nothing drafted
- **WHEN** the owner picks Discard Draft with no pending edits
- **THEN** a "Nothing to Discard" toast appears and no confirm is shown

#### Scenario: Unpublished notebook
- **WHEN** the owner opens the menu of a notebook that has never been published
- **THEN** neither "Discard Draft" nor "View Published" is listed
- **AND** a discard request to the server for it is declined and leaves its content intact

### Requirement: Unpublish in place
For a published notebook, the owner's menu SHALL offer "Unpublish", which after a confirm removes the published copy and its manifest in place, keeping the notebook, its latest content, and its URL in the owner's account, and returns the button to "Publish". (PRD-0064)

#### Scenario: Unpublish
- **WHEN** the owner unpublishes and accepts "Unpublish this notebook? Its link stops working for readers. It stays in your account, and you can publish it again."
- **THEN** an "Unpublished" toast says "The link no longer resolves. Your notebook is still here."
- **AND** the page stays at the same URL in the editor with its content and an enabled "Publish" button
- **AND** the reader URL and `/og/mutable/{id}.png` answer 404

#### Scenario: A pending draft survives
- **WHEN** a published notebook with a newer draft is unpublished
- **THEN** the newer draft is what remains, and unpublishing again changes nothing

### Requirement: Delete from the account
The owner's menu SHALL offer "Delete", which after the confirm "Delete this notebook from your account? Its link stops working and this cannot be undone. Download .ironpad first if you want to keep a copy." removes the notebook, its draft, its published copy, and every grant on it, then navigates home. (PRD-0064)

#### Scenario: Delete
- **WHEN** the owner deletes an account notebook
- **THEN** the browser lands on `/` with a "Deleted" toast, the home page no longer links to it, and its URL answers 404

#### Scenario: Shared blobs stay
- **WHEN** a published notebook whose cells were snapshotted is deleted
- **THEN** the content-addressed blobs it referenced stay in the share blob store

### Requirement: Owner-only operations
Draft save, edit load, Publish/Push, Discard Draft, Unpublish, Delete, the private flag, and grants SHALL require a signed-in session holding the notebook's OWNER grant. A caller who does not own a notebook SHALL get the same answer as for an id that does not exist. (PRD-0053, PRD-0061)

#### Scenario: Not signed in
- **WHEN** an anonymous caller attempts any save or publish operation
- **THEN** it fails with "sign in with GitHub to save or publish notebooks"

#### Scenario: Not the owner
- **WHEN** a signed-in non-owner pushes, deletes, or saves a draft to someone else's notebook
- **THEN** it fails with "unauthorized: you do not own this share" and the notebook is unchanged
- **AND** the same message comes back for an id that does not exist

### Requirement: Private notebooks and READ grants
For a published notebook, the owner's metadata panel SHALL show an Access section with a "Private" toggle and, while private, a list of READ grants with revoke and a "GitHub username" field to grant more. A private notebook SHALL be readable only by its owner and READ grantees, and the section SHALL NOT render for an unpublished notebook. (PRD-0061, PRD-0064)

#### Scenario: Make private
- **WHEN** the owner checks the private toggle
- **THEN** a "Share is now private" toast says "Only you and people you grant below can view it."

#### Scenario: Grant and revoke by handle
- **WHEN** the owner grants `carol`, who has signed in once, and later revokes her
- **THEN** `@carol` appears in the grant list and she can read the notebook
- **AND** after the revoke she is denied again

#### Scenario: Handle input rules
- **WHEN** the owner enters `@Carol` with surrounding spaces
- **THEN** it resolves to `carol`, since the leading `@` and spaces are stripped and logins match case-insensitively
- **AND** granting the same user twice leaves one grant

#### Scenario: Grant refusals
- **WHEN** the owner grants a handle that has never signed in, their own handle, or an empty value
- **THEN** a "Could not grant access" toast shows, respectively, "no ironpad user named \"{login}\": they need to sign in once before you can grant them access", "you already own this share", or "enter a GitHub username"

### Requirement: Denial of private notebooks on every read surface
A published private notebook SHALL be denied to anyone without OWNER or READ: the reader page answers 404 with "This notebook is private." and no title metadata, the embed shows a private notice, the blob manifest is withheld, and the OG card and oEmbed answer 404 to every caller. (PRD-0061)

#### Scenario: Anonymous visitor
- **WHEN** an anonymous visitor opens a private notebook
- **THEN** the page says "This notebook is private." with "sign in with GitHub" linking to `/auth/github`
- **AND** the raw body carries no title, and `/og/mutable/{id}.png` and `/oembed?url=…/mutable/{id}` answer 404

#### Scenario: Signed-in non-grantee
- **WHEN** a signed-in user without a grant opens it
- **THEN** the page says "Your account does not have access. Ask the author to grant your GitHub username."

#### Scenario: Embedded
- **WHEN** a private notebook is opened at `/embed/mutable/{id}`
- **THEN** the panel says "This notebook is private; open it on ironpad to sign in", which cross-site iframes always get because they carry no session cookie

### Requirement: Unpublished reads as not found everywhere
A notebook with no published copy SHALL read as not found on every reader and anonymous surface (reader page, embed, OG card, oEmbed, blob manifest, sitemap) for every caller, its owner included, whatever its private flag says; only the owner's editor reaches its content, and no response tells a stranger the id exists. (PRD-0064)

#### Scenario: Anonymous and stranger surfaces
- **WHEN** an anonymous caller and a signed-in stranger request the OG card and oEmbed for an unpublished notebook
- **THEN** both get 404 with none of the notebook's title, description, or source in the body
- **AND** `sitemap.xml` does not mention its id

#### Scenario: Published privately, then unpublished
- **WHEN** a notebook published as private is unpublished
- **THEN** a stranger gets the not-found answer, not the private-notebook denial

#### Scenario: Publishing opens the surfaces
- **WHEN** that notebook is then published publicly
- **THEN** its OG card answers 200 and oEmbed answers 200 with its title

### Requirement: Storage quotas
Every write that stores notebook JSON in an account (Save to Account, Share Mutable, draft autosave) SHALL pass the 4 MiB per-notebook upload limit and SHALL be refused if it would push the instance's stored notebook bytes past 512 MiB or the owner's past 32 MiB, drafts included in both. Promoting an existing draft SHALL never be refused by these caps. (PRD-0064, PRD-0054)

#### Scenario: Per-account cap
- **WHEN** a user's notebooks already fill their allowance and they save another
- **THEN** it fails with "account storage full: {n} bytes stored for this account, cap is 33554432 bytes"
- **AND** another user's save still succeeds

#### Scenario: Instance cap
- **WHEN** the instance's stored notebooks fill the global allowance
- **THEN** saves fail with "mutable share store full: {n} bytes stored, cap is 536870912 bytes"

#### Scenario: Drafts count
- **WHEN** a draft autosave would exceed a cap that the published copy alone fits under
- **THEN** the draft save is refused

#### Scenario: Refused autosave
- **WHEN** a draft autosave is refused for storage
- **THEN** the indicator reads "Draft not saved: storage full", the save is not retried, and one "Draft Not Saved" toast names the limit and says to delete a notebook from the account to free space

### Requirement: The owner's account listing
`list_mutable_shares` SHALL return every notebook the signed-in caller owns, published or not, each with id, title, `published` flag, `pushed_at` (absent until first publish), `created_at`, cell count, and tags, newest activity first (`pushed_at`, else `created_at`); anonymous callers SHALL get an empty list. (PRD-0064, PRD-0053)

#### Scenario: Unpublished notebooks are listed
- **WHEN** an owner has one published and one never-published notebook
- **THEN** both are listed, the latter titled from its draft with `published` false

#### Scenario: Only the owner's
- **WHEN** a different signed-in user lists their notebooks
- **THEN** none of the first owner's notebooks appears

#### Scenario: A broken row
- **WHEN** one owned row has neither a published copy nor a draft
- **THEN** that row is skipped and the rest of the listing still returns

### Requirement: In-place database upgrade
On boot the server SHALL upgrade an existing accounts database so that notebooks stored by versions that predate unpublished account notebooks still load, list, and push, with no operator migration step. (PRD-0064)

#### Scenario: A pre-account-notebook database
- **WHEN** the server boots against a database written by the older schema that holds a published notebook
- **THEN** that notebook still reads and pushes
- **AND** saving a new unpublished account notebook succeeds
