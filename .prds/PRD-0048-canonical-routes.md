---
id: PRD-0048
title: "Canonical notebook routes: /local, /public (extension-less), /shared"
status: done
owner: "Aaron Roney"
created: 2026-07-22
updated: 2026-07-22

principles:
- "Three peers, three prefixes: /local/{uuid}, /public/{name}, /shared/{hash}. The URL names the storage class, nothing else."
- "Old URLs never break: legacy routes redirect to canonical, and the server accepts both public-name forms forever (third-party embed specs carry .ironpad and cannot be updated)."
- "Ids keep their native shapes: local ids stay dashed UUIDs (IndexedDB keys them that way), share ids stay 16-hex content hashes. Cosmetic id unification is explicitly out of scope."

references:
- name: "Router"
  url: crates/ironpad-app/src/lib.rs
- name: "Public notebook server fns"
  url: crates/ironpad-app/src/server_fns.rs

acceptance_tests:
- id: uat-001
  name: "Legacy /notebook/{id} and /notebook/public/{name}.ironpad redirect to canonical /local/{id} and /public/{name} (URL bar shows canonical, page renders)"
  command: cargo make uat
  uat_status: verified
- id: uat-002
  name: "All emitted links (home cards, new-notebook navigate, fork, embed badge) use canonical forms; old embed specs with .ironpad still resolve"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "Canonical routes + legacy redirects"
  priority: 1
  status: done
  notes: "lib.rs: /local/{id} and /public/{filename} routes; legacy /notebook/{id} and /notebook/public/{filename} render Redirect components (SSR 3xx via leptos_router::components::Redirect), stripping .ironpad on the public form."
- id: T-002
  title: "Accept extension-less public names server-side"
  priority: 1
  status: done
  notes: "get_public_notebook_core appends .ironpad when missing (after path-segment validation). Old embed specs (public/x.ironpad) and new clean specs both resolve. Unit tests both forms."
- id: T-003
  title: "Emit canonical links everywhere"
  priority: 1
  status: done
  notes: "home_page: card hrefs + import navigate -> /local/{id}, public card href -> /public/{name}; view_only_notebook: fork navigate -> /local/{id}, canonical_path -> /public/{name} (strip .ironpad from specs); public page embed_spec becomes extension-less, which embed.js already accepts."
- id: T-004
  title: "e2e: regex updates + explicit redirect coverage"
  priority: 2
  status: done
  notes: "Update /notebook/ URL assertions to /local/ across specs and helpers; seed.spec asserts /public/welcome. New routes.spec.ts: legacy URLs land on canonical with the page rendered. Legacy gotos elsewhere stay as-is, doubling as implicit redirect coverage."
- id: T-005
  title: "Docs"
  priority: 3
  status: done
  notes: "CLAUDE.md routes list + Last Updated; DEVELOPMENT.md if it names routes."
---

# Summary

Standardize the URL scheme on the three storage classes: `/local/{uuid}` (private, IndexedDB), `/public/{name}` (bundled showcase notebooks, no `.ironpad` extension), `/shared/{hash}` (content-addressed shares, unchanged). Legacy routes redirect forever.

# Problem

The three notebook kinds live at inconsistent paths: `/notebook/{uuid}`, `/notebook/public/{file}.ironpad`, `/shared/{hash}`. The public URL leaks a file extension, and the prefixes do not name the thing they serve.

# Technical Approach

New routes plus two tiny redirect components in the router; `get_public_notebook_core` normalizes extension-less names so both public-name forms resolve (embed specs on third-party pages carry `.ironpad` and must work forever); every link-emission point switches to canonical forms. Local ids remain dashed UUIDs: IndexedDB keys notebooks by the hyphenated string, and reformatting ids for cosmetics is not worth the normalization layer.

# Non-Goals (MVP)

- Dash-less or unified id formats.
- Renaming embed routes (`/embed/shared`, `/embed/public` are iframe-internal, not URL-bar surface).
- A mutable "living share" alias.

# History

- 2026-07-22: Created from session discussion; scope reduced from full id unification to prefix + extension cleanup.
- 2026-07-22: T-001..T-003, T-005 implemented (routes + SSR redirects, server-side name normalization + unit tests, canonical link emission incl. in-notebook links, docs). T-004 regex sweep + routes.spec.ts done; full Playwright run pending.
- 2026-07-22: Full Playwright run surfaced two sweep misses (embed.spec still asserted .ironpad snippet forms; session.spec clicked a home card by its old /notebook href — template-literal interpolation dodged the regex). Fixed, both specs green; ci 668/668. T-004 done, uat-001/uat-002 verified. PRD complete.
