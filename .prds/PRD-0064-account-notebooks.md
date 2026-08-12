---
id: PRD-0064
title: "Save to Account: private server-stored notebooks, with publish as a flag"
status: done
owner: "Aaron Roney"
created: 2026-08-12
updated: 2026-08-12

depends_on:
- PRD-0053
- PRD-0054
- PRD-0061

principles:
- "Publishing is a flag on a row, not a storage class. An account notebook IS a mutable_share whose notebook_json is None; the content lives in draft_json, which is where the editor already writes it. Content resolves as draft_json.or(notebook_json), which is exactly what get_share_for_edit computes today. A second table, a second route, and a second editor mode were considered and rejected: PRD-0054 deleted the local mutable store specifically to stop reconciling two copies of one notebook, and adding a storage class back would reintroduce the same class of bug in a new place."
- "An account notebook always has draft_json. Both fields None is corruption, not a state. This invariant is what lets the unpublished case reuse the Push machinery unchanged: an unpublished notebook is permanently dirty by construction, so the button arms itself with no new state and no new flag."
- "Unpublished is indistinguishable from private-with-no-grants, on purpose. mutable_access_core (PRD-0061) is still the one gate, and get_mutable_notebook_core returning None still 404s the OG card, oEmbed, the embed route and the manifest. A visibility rule that already exists and is already tested beats a second rule that means the same thing."
- "Move, never copy. Save to Account uploads, deletes the local IndexedDB record, and hard-navigates, the same discipline Share Mutable already follows. Two copies of a notebook with no reconciliation is the failure PRD-0054 removed; a feature whose whole purpose is durable storage must not reintroduce it as a convenience."
- "The URL does not change when you publish. /mutable means server-stored and mutable, which is a statement about storage rather than audience, and PRD-0048 fixed the prefixes so they would keep meaning one thing. A link the owner saved before publishing keeps resolving after, and nothing in OG, sitemap, oEmbed or the embed specs moves."
- "Unpublish stops making the local copy the only copy. Today it writes to IndexedDB, deletes the share, and navigates away, with a load-bearing flush because for one moment the IndexedDB write is the sole surviving copy of the notebook. Clearing notebook_json in place removes that moment entirely. The cost is stated plainly: Unpublish no longer hands back a /local notebook, and Download .ironpad is how you get one."
- "Widening a SCHEMAFULL field is invisible to CI. Every DEFINE FIELD in define_schema carries IF NOT EXISTS, so changing notebook_json to option<string> is a no-op against an existing database. A fresh database is what dev boxes and CI open, so the whole suite passes while production keeps the old definition and rejects the first NONE write after deploy. DEFINE FIELD OVERWRITE is the fix, and the test that proves it MUST build a database with the pre-0064 schema first: a test that starts fresh cannot fail."
- "Storing notebooks is the point, so meter it per user. Drafts already count toward the global MAX_TOTAL_MUTABLE_BYTES, which means unpublished notebooks are metered for free, but a global cap alone lets one account consume the whole instance. The volume is at 4.0G of 9.9G today. A per-user cap alongside the global one is a few lines in the same check."

references:
- name: "Mutable share draft/published split, Push and Unpublish"
  url: .prds/PRD-0054-server-authoritative-mutable-shares.md
- name: "Private shares and the one access gate"
  url: .prds/PRD-0061-private-mutable-shares.md
- name: "Accounts DB, schema DDL, share rows"
  url: crates/ironpad-app/src/db.rs
- name: "Mutable server fns, quota constant, access cores"
  url: crates/ironpad-app/src/server_fns.rs
- name: "Share, publish, push, discard, unpublish and download flows"
  url: crates/ironpad-app/src/pages/notebook_editor/sharing.rs
- name: "Storage seam: Local vs ServerDraft persistence"
  url: crates/ironpad-app/src/pages/notebook_editor/state.rs
- name: "Home listing, search and filter chips"
  url: crates/ironpad-app/src/pages/home_page.rs
- name: "Access UI, rendered only for a published share"
  url: crates/ironpad-app/src/pages/notebook_editor/share_access.rs

acceptance_tests:
- id: uat-001
  name: "A signed-in user saves a local notebook to their account: the IndexedDB record is gone, the URL is /mutable/{id}, and the content survives a reload with the browser store cleared"
  command: cargo make uat
  uat_status: verified
- id: uat-002
  name: "An unpublished account notebook is 404 for anonymous visitors and signed-in non-owners on every anonymous surface: reader page, embed, OG card, oEmbed and manifest"
  command: cargo make uat
  uat_status: verified
- id: uat-003
  name: "Publish promotes the draft and the reader renders it; Unpublish clears the published copy, leaves the notebook in the account, and keeps it editable at the same URL"
  command: cargo make uat
  uat_status: verified
- id: uat-004
  name: "The widened schema applies to a database created by the pre-0064 DDL, and shares written under the old schema still load and still push"
  command: cargo make test
  uat_status: verified
- id: uat-005
  name: "A save that would exceed the per-user cap is rejected with a message naming the limit, and other users are unaffected"
  command: cargo make test
  uat_status: verified
- id: uat-006
  name: "Home lists account notebooks whether published or not, badges the published ones, and the Local/Account/Public chips filter to the right storage class"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "Widen notebook_json and pushed_at to option, with an OVERWRITE migration"
  priority: 1
  status: done
  notes: "DEFINE FIELD OVERWRITE on both fields; every other field keeps IF NOT EXISTS. The regression test must define the pre-0064 schema against a temp SurrealKV path, insert a published row, then run define_schema and assert the row still reads AND that a NONE write is now accepted. A test opening a fresh database cannot fail and proves nothing."
- id: T-002
  title: "Db layer: create_account_notebook, content resolution, unpublish in place, listing with published"
  priority: 1
  status: done
  notes: "create_account_notebook writes notebook_json NONE, draft_json Some, no manifest, no pushed_at. unpublish_share moves published into draft when draft is None, then clears notebook_json, manifest_json and pushed_at. OwnedShareRow gains published: bool and carries draft-or-published content so the home title is right for both."
- id: T-003
  title: "Server fns: one create path, first publish, listing shape, per-user quota"
  priority: 1
  status: done
  notes: "create_mutable_share becomes a wrapper over create_account_notebook plus promote so there is ONE upload path. push_mutable sets pushed_at on first publish and drops its already-clean short circuit when notebook_json is None. Per-user byte sum keyed by the OWNER grant, checked in the same place as MAX_TOTAL_MUTABLE_BYTES."
- id: T-004
  title: "Verify every anonymous surface 404s for an unpublished notebook"
  priority: 1
  status: done
  notes: "Reader page, /embed/mutable, OG card, oEmbed and get_mutable_manifest all funnel through get_mutable_notebook_core / mutable_access_core, so this should be free. Assert it rather than assume it, and assert the reader's HTTP STATUS against the raw response body, not the hydrated DOM: PRD-0050 and PRD-0063 both shipped a status race that is invisible in the DOM."
- id: T-005
  title: "Editor: Save to Account action, Publish/Push labels, gated access section"
  priority: 2
  status: done
  notes: "Save to Account is the shareMutable flow minus the promote: flush, serialize, upload, delete the local record, hard-navigate. Button reads Publish when notebook_json is None, then the existing Push / Published states. ShareAccessSection renders only when published, since a private toggle on something nobody can reach has no effect."
- id: T-006
  title: "Unpublish in place, deleting the save-to-local dance"
  priority: 2
  status: done
  notes: "Replaces unpublish_current_notebook. No IndexedDB write, no navigation, no confirm about losing the only copy. Keep Download .ironpad as the way back to a local file, and keep Delete for removing it from the account."
- id: T-007
  title: "Home: Local / Account / Public chips, published badge, unpublished rows listed"
  priority: 2
  status: done
  notes: "FilterMode Private becomes Local and Published becomes Account. Expect Playwright selector churn: the icon-sweep rename broke 52 specs through one literal filter, so grep the specs for the chip labels before changing them."
- id: T-008
  title: "e2e coverage across the account lifecycle"
  priority: 2
  status: done
  notes: "save to account, reload, publish, read as anonymous, unpublish, confirm 404 returns and the owner still edits, delete. Uses the existing test-login helper. Kill any stale server on :3111 first; reuseExistingServer will silently test the old binary."
- id: T-009
  title: "Docs: CLAUDE.md stamp, README storage section, PRD close-out"
  priority: 3
  status: done
  notes: "The README storage story currently says private notebooks are browser-local, full stop. Note the Unpublish behavior change explicitly, since it is the one existing behavior this PRD takes away."
---

# Summary

Private notebooks live in IndexedDB and nowhere else, so signing in with GitHub does not make your own work follow you to another machine. This adds a second storage class for signed-in users: **Save to Account** uploads a notebook to the server, where it stays private and editable until you choose to publish it.

The machinery already exists. A `mutable_share` with a server-side draft, an owner grant, a private flag, debounced autosave and a Push button is, minus the requirement that it be published, exactly an account notebook.

# Problem

An account today buys exactly two things: publishing a mutable share, and being granted READ on someone else's private one. The notebooks a person actually works on are unaffected by whether they are signed in.

Concretely:

- A notebook written on a laptop does not exist on a phone.
- Clearing site data destroys the notebook and PRD-0058's version ring along with it, because the ring lives in the same IndexedDB database.
- The only workaround is Download `.ironpad` and Import on the other machine, by hand, every time.

A user can already reach the destination by hitting Share Mutable and then flipping the private toggle in the metadata panel. That is a publish-then-hide flow standing in for a save, and it is not discoverable as one.

# Goals

1. A signed-in user can move a local notebook into their account in one action, and edit it from any browser they are signed in on.
2. An unpublished account notebook is invisible to everyone else, on every surface, including the anonymous ones (OG cards, oEmbed, embeds).
3. Publishing stays the single explicit editorial act it is today, and does not change the notebook's URL.
4. No notebook is ever briefly the only copy of itself during a state transition.
5. One account cannot consume the instance's storage allowance.

# Technical Approach

## State

```
                    notebook_json   draft_json    reader        button
in account            None          Some          404           Publish
published, clean      Some          None          published     Published (disabled)
published, dirty      Some          Some          published     Push
```

`content = draft_json.or(notebook_json)`. `ShareEditRow` already resolves it that way, so the editor needs no new read path. An unpublished notebook is permanently dirty, which arms Push with no new flag.

## Migration

`notebook_json` and `pushed_at` widen to `option<string>` via `DEFINE FIELD OVERWRITE`. Every other field keeps `IF NOT EXISTS`.

The hazard is that `IF NOT EXISTS` silently declines to change an existing field, and CI opens a fresh database on every run. The failure would therefore appear for the first time in production, on the first Save to Account after deploy, as a rejected write. The regression test builds a database with the pre-0064 DDL, writes a published row, then runs `define_schema` and asserts both that the old row still reads and that a `NONE` write is now accepted.

## Visibility

No new rule. `get_mutable_notebook_core` returns `None` when there is no published copy, which is the same shape as its existing private-share denial, so the OG handler, oEmbed, the embed route and the manifest all 404 without changes. The reader page renders the PRD-0061 denial, and the owner's editor swaps in on hydrate as it does today. `SsrMode::Async` is already on the route, which is what makes the 404 status honest rather than racing the shell flush.

## Quota

Draft bytes are already counted, so unpublished notebooks are metered by the existing global cap. A per-user sum over the rows carrying that user's OWNER grant is checked in the same place.

# Assumptions

- SurrealDB 3.0.5 supports `DEFINE FIELD OVERWRITE` on a SCHEMAFULL table with existing rows, and widening `string` to `option<string>` preserves them. T-001 verifies this before anything else is built.
- Notebook drafts do not embed `saved_output` (only the editorial moments do, per PRD-0056), so per-user storage is dominated by published copies rather than by autosaves.

# Constraints

- Last-write-wins at autosave granularity, unchanged from PRD-0054. Two devices editing one account notebook is documented, not solved.
- Anonymous users keep `/local` exactly as it is. This adds a storage class; it does not take one away.

# References to Code

- `crates/ironpad-app/src/db.rs` — schema DDL, share rows, `get_share_for_edit`, `promote_draft`, `total_mutable_bytes`
- `crates/ironpad-app/src/server_fns.rs` — the mutable server-fn set, `MAX_TOTAL_MUTABLE_BYTES`, the access cores
- `crates/ironpad-app/src/pages/notebook_editor/sharing.rs` — flush-before-serialize, share, push, unpublish, download
- `crates/ironpad-app/src/pages/notebook_editor/state.rs` — the Local vs ServerDraft persistence seam
- `crates/ironpad-app/src/pages/home_page.rs` — listing, search, filter chips
- `crates/ironpad-app/src/pages/notebook_editor/share_access.rs` — private toggle and READ grants

# Non-Goals (MVP)

- Server-side version history. PRD-0058's ring stays a `/local` feature.
- READ grants on unpublished notebooks. Publishing privately already covers sharing a work in progress.
- Two-way sync between `/local` and an account notebook. Save to Account moves; it does not mirror.
- Any change to immutable shares, public notebooks, or anonymous use.

# History

- 2026-08-12: Drafted from a brainstorming session. Storage model, URL, Unpublish semantics, quota and home-chip naming decided with Aaron before writing.
- 2026-08-12: Implemented and closed. Gate green: 907 unit, 13 integration, 144 Playwright.
  The spike (T-001) held: `DEFINE FIELD OVERWRITE` widens a SCHEMAFULL field on a table with
  rows, and the negative control confirmed `IF NOT EXISTS` is a silent no-op, so the trap the
  design was built around is real. The spike also measured four Rust read paths still typed
  `String` that would have failed on a NONE row, `mutable_share_exists` among them, which is
  the id-collision check used when minting share ids.

  One fact about this workspace, with two consequences, and one claim I got wrong.

  **`ironpad-frontend` enables `ironpad-app/hydrate`, so a workspace-wide cargo invocation
  unifies `hydrate` on beside `ssr`.** Verified by injecting a type error into a
  `#[cfg(feature = "hydrate")]` function and watching `cargo clippy --all-targets` fail with
  exit 101, which is the gate's own clippy step.

  Consequence one: **a single-crate test run is not evidence.** The T-004 integration test
  passed under `cargo nextest run -p ironpad-server` and failed under `cargo make test`,
  which runs the workspace with no `-p`. With `hydrate` unified in, rendering `App` reaches
  `js-sys` statics on a non-wasm target, so `generate_route_list` dies before the first
  assertion. Rendering `App` from a Rust test is structurally wrong here. The reader page and
  embed moved to Playwright (which already covered both denied identities against raw
  bodies); OG, oEmbed and the sitemap stayed in Rust so the denial keeps coverage at `ci`
  speed, proven by publishing the notebook mid-test and watching every denial fail.

  Consequence two, and **the correction**: an implementing agent reported, and this entry
  originally repeated, that `cargo make ci` never compiles `hydrate` and would therefore go
  green with the cross-cluster `discard_mutable_draft` signature change unfixed. That is
  FALSE. It was never observed: the caller was adapted inside the same workflow run, so CI
  never saw the broken state, and the claim was reasoning rather than measurement. The
  injection above is the measurement, and the gate catches it. No `ci` change is warranted.
  What remains genuinely uncovered by `ci` is only wasm-target-specific breakage (linking,
  `wasm-bindgen` codegen, `target_arch` paths), which `cargo make build` under `uat` covers.

    Findings worth carrying forward. Share Mutable had silently stopped being atomic once
  create and promote split: a failed promote left a `/local` copy plus an invisible orphan,
  which is the "move, never copy" violation this PRD exists to forbid, and each retry minted
  another. `mutable_access_core` checked `private` before "no published copy", so a
  published-private notebook that was then unpublished told a stranger it existed; the order
  is now reversed. The per-user quota's refusal was permanent but the autosave path treated
  it as transient and retried forever with the explanation only in the console.

  Deliberately not fixed, recorded so the next reader does not re-derive them: the owner's
  SSR placeholder keys on "someone is signed in" rather than "owns this share", because the
  exact signal would pull draft content onto the one path whose invariant is that it is not
  there; `MutableShareSummary.pushed_at` keeps its `Option` because no reachable shape fixes
  the stale-client case and the alternatives trade a diagnosable failure for a quiet wrong
  one (pinned with a test); and the OG card's one-hour TTL after an in-place Unpublish
  carries over the PRD-0061 review decision, now reachable on purpose rather than once.

