---
id: PRD-0057
title: "/embed/mutable + oEmbed: published notebooks become embeddable"
status: done
owner: "Aaron Roney"
created: 2026-08-06
updated: 2026-08-06

depends_on:
- PRD-0054

principles:
- "Behavioral parity with /embed/shared: never autorun (a published notebook is one author's content, not first-party showcase), same chrome-less shell, same height messaging."
- "Live semantics carry through: the embed resolves the PUBLISHED copy no-cache, so a Push updates every consumer's iframe on next load — that is the point of embedding a mutable link."
- "Drafts never reach an embed: the embed path uses the reader resolve (get_mutable_notebook), which serves published only."

references:
- name: "Embed routes (PRD-0039 pattern)"
  url: crates/ironpad-app/src/pages/embed_notebook.rs
- name: "oEmbed provider (PRD-0051)"
  url: crates/ironpad-server/src/oembed.rs
- name: "Loader script"
  url: public/embed.js

acceptance_tests:
- id: uat-001
  name: "/embed/mutable/{id} renders the published notebook chrome-less with saved outputs, and never autoruns"
  command: cargo make uat
  uat_status: verified
- id: uat-002
  name: "GET /oembed?url=<origin>/mutable/{id} returns the iframe payload; offsite and traversal URLs still 404"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "EmbedMutablePage + route + loader + snippet plumbing"
  priority: 1
  status: done
  notes: "embed_notebook.rs gains EmbedMutablePage (reactive param Memo like its siblings; get_mutable_notebook -> .notebook + get_mutable_manifest; autorun never). lib.rs route /embed/mutable/{id}. embed.js accepts the mutable spec class. canonical_path gains the mutable arm (in-embed badge link). MutableReader passes embed_spec so the reader page shows the Embed button."
- id: T-002
  title: "oEmbed: mutable mapping + discovery link"
  priority: 1
  status: done
  notes: "embed_target accepts /mutable/{id}; the handler resolves the title via get_mutable_notebook_core (Extension(Db), same wiring as the OG handler); the mutable page's SocialMeta sets oembed=true so consumers discover the endpoint. Module docs updated (the 'only public and shared' limitation is gone). robots.txt already disallows /embed/ by prefix — no change."
- id: T-003
  title: "Tests + docs"
  priority: 2
  status: done
  notes: "Unit: embed_target mutable cases (accept, reject traversal/offsite). e2e: embed spec renders a published notebook in the iframe shell without autorun; oembed spec maps a /mutable URL and rejects a draft-only expectation (payload title is the PUBLISHED title). CLAUDE.md routes + oEmbed lines."
---

# Summary

`/embed/mutable/{id}` renders a published notebook chrome-less for iframes, and the oEmbed provider maps `/mutable/{id}` URLs to it. Closes the "flagship surface is the only un-embeddable one" gap; embeds track pushes because the resolve is live.

# History

- **2026-08-06** — PRD created as part of the pre-v0.17.0 batch.
- **2026-08-06** — Implemented and closed: EmbedMutablePage (reactive param, published-only resolve, never autoruns), /embed/mutable/{id} route, embed.js mutable spec class, canonical_path arm, reader-page Embed button (embed_spec) + oEmbed discovery link (SocialMeta oembed=true), oEmbed provider maps /mutable with traversal rejection and resolves titles via Extension(Db). Stale "mutable is excluded" tests updated on both unit and e2e sides. Gate: cargo make ci (810), full Playwright 111 passed / 0 failed (new combined e2e: publish -> oEmbed payload -> raw discovery link -> chrome-less no-autorun embed). Unreleased; ships with v0.17.0.
