# shared-notebooks Specification

## Purpose
Immutable, content-addressed copies of a notebook that anyone can create without an account and anyone holding the link can open at `/shared/{hash}`. Covers the Share Immutable action, the hash scheme, upload limits, and the share-time compiled-blob snapshots (the manifest and the `/share-blobs/` route) that let viewers replay cells without the server compiling anything.

## Requirements
### Requirement: Share Immutable action
The notebook menu SHALL offer "Share Immutable" in the local editor and in the account editor, to anonymous and signed-in users alike. It SHALL flush in-progress cell edits, upload the notebook, copy the resulting `{origin}/shared/{hash}` URL to the clipboard, and report the outcome in a toast. (PRD-0048, PRD-0053)

#### Scenario: Sharing copies the link
- **WHEN** a user picks Share Immutable from the notebook menu
- **THEN** a "Sharing…" toast appears immediately
- **AND** on success a "Link Copied!" toast shows the full `/shared/{hash}` URL, which is also on the clipboard

#### Scenario: No account needed
- **WHEN** a visitor who has never signed in shares a local notebook
- **THEN** the share succeeds exactly as it does for a signed-in user

#### Scenario: Refused upload
- **WHEN** the server refuses the upload (too large, invalid, or the share store is full)
- **THEN** a "Share Failed" toast shows the server's message and no link is copied

### Requirement: Content-addressed, frozen shares
The server SHALL name a shared notebook by the first 16 lowercase hex characters of the blake3 hash of the exact uploaded JSON, and SHALL never alter what a stored hash resolves to. Editing a notebook and sharing again mints a new hash; the old link keeps serving the old content, and there is no way to delete a share. (PRD-0048)

#### Scenario: Identical bytes, identical hash
- **WHEN** the same notebook JSON is shared twice
- **THEN** both calls return the same 16-hex hash and one stored copy exists

#### Scenario: An edit mints a new link
- **WHEN** a shared notebook is edited and shared again
- **THEN** the second share returns a different hash
- **AND** the first `/shared/{hash}` link still resolves to the content it was minted for

#### Scenario: Where shares live on disk
- **WHEN** an operator inspects the data directory after a share
- **THEN** the notebook is at `{data_dir}/shares/{hash}.json`, its blob manifest (if any) at `{data_dir}/shares/{hash}.manifest.json`, and snapshotted blobs under `{data_dir}/shares/blobs/`

### Requirement: Opening a share
`/shared/{hash}` SHALL resolve a stored share for any visitor without a session, and SHALL answer an unknown hash, or one containing a path separator or `..`, with HTTP 404 and the copy "Shared notebook not found or expired" without reading outside the share store. (PRD-0048, PRD-0036)

#### Scenario: Unknown hash
- **WHEN** a visitor opens `/shared/deadbeefdeadbeef` and no such share exists
- **THEN** the response status is 404
- **AND** the page says "Shared notebook not found or expired" with the hint "The share link may have expired, or the notebook may have been removed."

#### Scenario: Traversal attempts
- **WHEN** the shared-notebook lookup receives `../etc/passwd`, `foo/bar`, `foo\bar`, or `..`
- **THEN** it is refused as an invalid name and no file outside the share store is read

### Requirement: Per-upload size and validity
The server SHALL refuse a share upload larger than 4 MiB (4,194,304 bytes) before parsing it, with "notebook too large: {n} bytes (max 4194304)", and SHALL refuse JSON that is not a notebook with "invalid notebook JSON: …"; a refused upload writes nothing. (PRD-0036)

#### Scenario: Oversized upload
- **WHEN** a share upload is 4,194,305 bytes
- **THEN** it is refused with a message starting "notebook too large: "
- **AND** no share directory or file is created

#### Scenario: Garbage upload
- **WHEN** a share upload is the string `not json`
- **THEN** it is refused with a message starting "invalid notebook JSON: "

### Requirement: Aggregate share store cap
The server SHALL refuse a new distinct share once the files already in the share directory (notebooks and manifests, not blobs) plus the new upload would exceed 512 MiB, with "share store full: {n} bytes stored, aggregate cap is 536870912", and SHALL always accept re-sharing a hash that is already stored, since that adds no bytes. (PRD-0038)

#### Scenario: Store at the cap
- **WHEN** the share store is full and a user shares a notebook not already stored
- **THEN** the share is refused with "share store full" and nothing new is written

#### Scenario: Re-share at the cap
- **WHEN** the share store is full and a user re-shares byte-identical JSON that is already stored
- **THEN** the share succeeds and returns the existing hash

### Requirement: Per-client share rate limit
The server SHALL charge each new distinct share's byte length against a per-client budget of `IRONPAD_SHARE_RATE_BURST_BYTES` (default 64 MiB) refilling at `IRONPAD_SHARE_RATE_BYTES_PER_MIN` (default 16 MiB an hour), keyed like the build rate limit, and SHALL refuse a share the budget cannot cover with "rate limited: too many new shares from this client" before anything is written. Re-sharing a stored hash SHALL never be charged.

#### Scenario: Scripted share flood
- **WHEN** one client uploads distinct notebooks faster than its budget refills
- **THEN** the next new share is refused with "rate limited" and nothing is written
- **AND** a share from another client, or a re-share of a stored hash, still succeeds

### Requirement: Share-time blob snapshot
When a share is created, the server SHALL copy the compiled blob (and JS glue, if any) of each non-shared Rust or Linux cell whose cache key, built from the sharer's positional type tags, is already in the compile cache into the share blob store. It SHALL never compile at share time, and a skipped or failed snapshot SHALL never fail the share. (PRD-0047, PRD-0066)

#### Scenario: Run, share, replay without compiling
- **WHEN** a user runs a cell that prints 42, shares the notebook, and a fresh browser context opens the link and clicks Run All
- **THEN** the cell shows 42
- **AND** the viewer made at least one `/share-blobs/` request and zero `compile_cell` requests

#### Scenario: Cold and non-code cells
- **WHEN** a notebook is shared with a code cell that was never run, a markdown cell, and a shared cell
- **THEN** none of those cells gets a snapshot entry, and viewers fall back to live compilation for the unrun cell

#### Scenario: Linux cells snapshot under their own key
- **WHEN** a Linux cell's blob is in the compile cache and the notebook is shared
- **THEN** the cell gets a snapshot entry keyed for the Linux target
- **AND** a blob cached for an ordinary cell with the same source never satisfies it

#### Scenario: Missing or mismatched type tags
- **WHEN** a client omits `cell_type_tags`, or sends a tag list whose length differs from the cell count
- **THEN** the notebook is stored and the share succeeds with no new snapshot entries

#### Scenario: A cached blob under a hostile tag chain
- **WHEN** a cell's blob is cached under upstream type tags that a compile would refuse as not being types
- **THEN** that cell gets no snapshot entry and its siblings are snapshotted as usual

#### Scenario: Blob store over budget
- **WHEN** the share blob store already holds more than 2 GiB
- **THEN** new shares succeed with no new snapshot entries

#### Scenario: Shares outlive the compile cache
- **WHEN** an operator clears the compile cache's `blobs` tier after a share was snapshotted
- **THEN** that share's `/share-blobs/` files still serve, because snapshots live in the data directory

### Requirement: Share blob manifest
Each share SHALL have at most one blob manifest of the form `{"version": 1, "cells": {cell_id: {"blob": <64-hex>, "has_js_glue": bool}}}`, readable by anyone through `get_shared_manifest(hash)`. Re-sharing the same hash SHALL add entries for newly cached cells but SHALL never replace an existing entry. (PRD-0047)

#### Scenario: No snapshot
- **WHEN** a share has no cache-hit cells, predates snapshots, or its manifest file is unreadable
- **THEN** `get_shared_manifest` returns none and viewers compile live

#### Scenario: Coverage only grows
- **WHEN** a notebook is shared, more of its cells are run, and identical JSON is shared again
- **THEN** the manifest keeps every earlier entry and gains entries for the newly cached cells

#### Scenario: Existing entries are frozen
- **WHEN** someone re-posts a stored share's exact JSON with different type tags that would key a cell to a different blob
- **THEN** that cell's manifest entry still names the blob recorded by the first share

### Requirement: Immutable share-blob route
The server SHALL serve snapshotted artifacts, without any session, at `/share-blobs/{hash}.wasm` (`application/wasm`) and `/share-blobs/{hash}.js` (`text/javascript`), where `{hash}` is exactly 64 lowercase hex characters, with `Cache-Control: public, max-age=31536000, immutable`. Any other name, and a well-formed name with no file, SHALL get 404 "not found". (PRD-0047, PRD-0061)

#### Scenario: A snapshotted blob
- **WHEN** a client GETs `/share-blobs/{hash}.wasm` for a blob named in a manifest
- **THEN** the response is 200 with `Content-Type: application/wasm` and `Cache-Control: public, max-age=31536000, immutable`

#### Scenario: Malformed names
- **WHEN** a client requests `abc.wasm`, a 64-character uppercase hex name, `{hash}.json`, a bare `{hash}`, `../{hash}.wasm`, or `{hash}.wasm.tmp.x`
- **THEN** each answers 404

#### Scenario: Ungated by design
- **WHEN** a blob belongs to a private account notebook
- **THEN** the route still serves it to anyone who has the hash, because the only place hashes appear is a manifest, which is gated

### Requirement: Outputs travel with the share
Share Immutable SHALL embed each cell's output panels from the sharer's latest run in the uploaded copy, replacing any output larger than 256 KiB with a placeholder, and SHALL keep a saved output already in the notebook for any cell not run in this session. (PRD-0056)

#### Scenario: A run cell
- **WHEN** a user runs a cell and then shares the notebook
- **THEN** the stored share carries that cell's output panels as its saved output

#### Scenario: Oversized output
- **WHEN** a cell's output panels exceed 256 KiB at share time
- **THEN** the shared copy carries a too-large placeholder for that cell instead of the panels
