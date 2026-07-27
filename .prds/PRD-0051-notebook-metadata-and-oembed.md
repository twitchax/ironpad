---
id: PRD-0051
title: "Editable notebook metadata, oEmbed, and social-preview hardening"
status: done
owner: "Aaron Roney"
created: 2026-07-27
updated: 2026-07-27

depends_on:
- PRD-0039
- PRD-0049
- PRD-0050

principles:
- "A field the product reads is a field the product must let you write. PRD-0050 shipped description/tags/og_image as unfurl inputs with no UI to set them."
- "Validate attacker-controlled values at the single point of use, never at the point of write. og_image_path() is the model; og_image_dimensions() follows it."
- "One definition per wire concept. The mutation and its mirrored event carry the same struct, so they cannot drift."
- "A CSP that needs 'unsafe-inline' to work is theatre. Ship the directives that cost nothing and defer script-src to real nonce work."
- "Unfurl assertions are made against raw response bodies. Crawlers run no JavaScript, so a hydrated-DOM test passes on markup nobody can see."

references:
- name: "oEmbed 1.0 specification"
  url: https://oembed.com/
- name: "Open Graph image dimension hints"
  url: https://ogp.me/#structured
- name: "CSP base-uri and form-action"
  url: https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Content-Security-Policy

acceptance_tests:
- id: uat-001
  name: "The metadata panel writes description, tags, and og_image to IndexedDB and they survive a reload"
  command: npx playwright test notebook-metadata
  uat_status: verified
- id: uat-002
  name: "A description set in the editor reaches og:description on the shared copy's raw response body"
  command: npx playwright test notebook-metadata
  uat_status: verified
- id: uat-003
  name: "An og_image override advertises its own declared dimensions rather than 1200x630"
  command: cargo make test
  uat_status: verified
- id: uat-004
  name: "og_image_dimensions rejects a missing axis, an implausible size, and a rejected override path"
  command: cargo make test
  uat_status: verified
- id: uat-005
  name: "GET /oembed returns a rich response embedding the chrome-less route, and refuses foreign URLs"
  command: npx playwright test oembed
  uat_status: verified
- id: uat-006
  name: "Flattening NotebookMetaPatch leaves the collaboration wire format byte-identical"
  command: cargo make test
  uat_status: verified
- id: uat-007
  name: "An explicit null in a metadata mutation clears the field; an absent key leaves it alone"
  command: cargo make test
  uat_status: verified
- id: uat-008
  name: "Every response carries the CSP, and it does not restrict framing"
  command: npx playwright test social-preview
  uat_status: verified

tasks:
- id: T-001
  title: "Add og_image_width/og_image_height to IronpadNotebook with an og_image_dimensions() gate"
  priority: 1
  status: done
  notes: "Mirrors og_image_path(): both axes required, bounded by OG_IMAGE_MIN_PX..=OG_IMAGE_MAX_PX, and only meaningful alongside a usable override."
- id: T-002
  title: "Extract NotebookMetaPatch and flatten it into both the mutation and the event"
  priority: 1
  status: done
  notes: "Carries the five presentation fields. apply_to() replaces the CLI daemon's hand-written mirror of the model's logic."
- id: T-003
  title: "Fix the doubled-option clear semantics on the wire"
  priority: 1
  status: done
  notes: "explicit_null_is_a_clear: serde collapsed Some(None) to None on decode, so a clear crossing the WebSocket was read as unchanged. Latent until clearable fields existed."
- id: T-004
  title: "Notebook metadata panel below the cell list"
  priority: 1
  status: done
  notes: "Own wrapper div, not a third child of the shared appendix, whose e2e spec indexes positionally. Inline validation mirrors og_image_path's rule."
- id: T-005
  title: "Thread image_size through SocialMeta and the three notebook pages"
  priority: 2
  status: done
  notes: "og:image:width/height were hardcoded 1200x630 even when og_image overrode the generated card."
- id: T-006
  title: "oEmbed provider at /oembed plus discovery links on /public and /shared"
  priority: 2
  status: done
  notes: "Maps a canonical URL to an /embed/* iframe. Origin-locked. /mutable is excluded because it has no embed route."
- id: T-007
  title: "Mutable-share unfurl e2e coverage"
  priority: 2
  status: done
  notes: "PRD-0049's lifecycle tests already existed; what PRD-0050 uat-002/uat-005 claimed and lacked was the UNFURL assertion. Added to tests/e2e/mutable-shares.spec.ts."
- id: T-008
  title: "Content-Security-Policy header"
  priority: 2
  status: done
  notes: "object-src, base-uri, form-action. No script-src (would need 'unsafe-inline'), no frame-ancestors (would break /embed)."
- id: T-009
  title: "Cache the public-notebook scan and single-flight card rendering"
  priority: 3
  status: done
  notes: "The scan read and parsed 45 notebooks per home page, sitemap, and site-card request."
- id: T-010
  title: "Wire AppConfig::absolute_url and fix stale docs"
  priority: 3
  status: done
  notes: "absolute_url had zero non-test callers; social_meta.rs and crawl.rs each re-implemented the trim."
---

# Summary

Makes the notebook metadata that PRD-0050 reads writable from the editor, adds
an oEmbed provider so consumers can embed the live notebook instead of a
picture of it, and closes the review findings left over from the v0.13.0
social-preview work.

# Problem

PRD-0050 shipped link unfurls that read `description`, `tags`, and `og_image`
off `IronpadNotebook`. Nothing could write them. `NotebookUpdateMeta` carried
only `title`, `shared_cargo_toml`, `shared_source`, and `reactive_mode`, and
there was no UI, so those three fields were reachable only by hand-editing a
`.ironpad` file on disk.

The result: every notebook authored *in* ironpad unfurled with the fallback
line "An interactive Rust notebook on ironpad." and no tags. Only the bundled
public notebooks, whose JSON was written by hand, had real descriptions. Tag
search on the home page already matched private notebooks by tag, against tags
no one could set.

Four smaller findings from the same review:

1. `og:image:width` and `og:image:height` were hardcoded to 1200x630 even when
   a notebook overrode the generated card. Unfurlers reserve layout from the
   declared size, so an override of any other shape came out letterboxed.
2. PRD-0050 marked two acceptance tests `verified` on claims that included
   `/mutable`, with nothing asserting a mutable share's unfurl. The PRD-0049
   lifecycle tests did exist; the preview assertion did not.
3. `AppConfig::absolute_url` had zero non-test callers; `social_meta.rs` and
   `crawl.rs` each re-implemented the trailing-slash trim inline.
4. No response carried a `Content-Security-Policy`.

# Goals

1. Make description, tags, and the preview-image override editable, and carry
   them over the collaboration protocol so an agent can set them too.
2. Declare an override image's real dimensions.
3. Serve oEmbed so discovery-capable consumers embed the running notebook.
4. Clear the review findings above, and make the PRD-0050 UAT claims true.

# Technical Approach

## NotebookMetaPatch

The mutation and its mirrored event carried two hand-maintained copies of the
same field list, and a metadata handler taking nine positional arguments was
the immediate consequence of adding five more. Both variants now flatten a
single `NotebookMetaPatch`:

```
Mutation::NotebookUpdateMeta { #[serde(flatten)] meta: NotebookMetaPatch }
Event::NotebookMetaUpdated   { #[serde(flatten)] meta: NotebookMetaPatch }
```

`Mutation` is internally tagged (`#[serde(tag = "action")]`), so flattening
leaves the fields sitting directly beside `action` and the wire format is
unchanged. A test asserts exactly that, since the alternative would silently
break every peer.

`NotebookMetaPatch::apply_to` replaces two copies of the same field-by-field
application: one in the browser model, one in the CLI daemon. Those copies
drifting is precisely how an agent's cached notebook stops matching the
browser's.

### The clear that never crossed the wire

Every clearable field is `Option<Option<T>>`: `None` unchanged, `Some(None)`
clear, `Some(Some(v))` set. Serde's default decode for that type collapses an
explicit `null` to `None`, so a clear serialized as `null` and arrived as
"unchanged". Nothing sent `Some(None)` before this PRD, so the bug was latent;
clearing a description is the first thing that would have hit it.
`explicit_null_is_a_clear` fixes the decode. Paired with `skip_serializing_if`,
an absent key still means unchanged because it never reaches the deserializer.

## Validation at the point of use

`og_image_dimensions()` follows `og_image_path()` exactly: notebooks arrive
from unauthenticated shares and from IndexedDB, so the numbers are
attacker-controlled. Three conditions, each with a failure mode behind it: both
axes present, both within `OG_IMAGE_MIN_PX..=OG_IMAGE_MAX_PX`, and an override
image actually present. A declared size reserves a layout box in someone's feed
before the image is ever fetched, which is what makes an unbounded value worth
refusing.

## oEmbed

`/oembed?url=…` maps a canonical notebook URL to its `/embed/*` route and
returns a `rich` response wrapping the same iframe the view-only toolbar
already copies. Origin-locked against `public_url`: a provider that embedded
arbitrary URLs would be an open redirect wearing an iframe, because the
consumer trusts the returned HTML on the strength of trusting the provider.
`/mutable` is excluded, having no embed route; resolving it would hand back a
frame pointing at a 404.

## CSP

`object-src 'none'; base-uri 'self'; form-action 'self'`.

No `script-src`. Leptos hydration emits an inline module script and Monaco
ships its own loader, so any script policy today means `'unsafe-inline'`, which
would have permitted the exact injection fixed in v0.13.1. Per-request nonces
through `leptos_meta` and the Monaco bootstrap are the real answer and are
deliberately out of scope. No `frame-ancestors`: `/embed/*` exists to be framed
by third parties.

# Assumptions

- One `site_root` per server process, which is what lets the public-notebook
  scan be cached in a process-lifetime `OnceCell`. The core function stays
  uncached so its unit tests keep pointing at their own `TempDir`.
- Consumers that matter for oEmbed use discovery. X, Reddit, and Slack use
  allowlists, so Open Graph remains the mechanism for those.

# Constraints

- The wire format may not change: the CLI daemon and browser are versioned
  independently and a running daemon must keep working across a deploy.
- The metadata panel must not be a third child of `.ironpad-editor-shared-appendix`,
  because `shared-appendix.spec.ts` indexes that container positionally.
- IndexedDB needs no `DB_VERSION` bump: `DB_VERSION` gates object stores, not
  record shape, and new optional fields ride along inside the structured clone.

# References to Code

- `crates/ironpad-common/src/types.rs` — `og_image_width`/`og_image_height`, `og_image_dimensions()`, `OG_IMAGE_MIN_PX`/`MAX_PX`
- `crates/ironpad-common/src/protocol.rs` — `NotebookMetaPatch`, `apply_to`, `explicit_null_is_a_clear`, `PROTOCOL_VERSION = 2`
- `crates/ironpad-common/src/config.rs` — `absolute_url` free function + method
- `crates/ironpad-app/src/pages/notebook_editor/metadata_panel.rs` — the panel
- `crates/ironpad-app/src/components/social_meta.rs` — `image_size`, `oembed`, `absolute`
- `crates/ironpad-server/src/oembed.rs` — the provider
- `crates/ironpad-server/src/main.rs` — `CONTENT_SECURITY_POLICY`
- `crates/ironpad-server/src/og/mod.rs` — `RENDER_LOCKS` single-flight
- `crates/ironpad-app/src/server_fns.rs` — `list_public_notebooks_cached`
- `tests/e2e/notebook-metadata.spec.ts`, `tests/e2e/oembed.spec.ts`, `tests/e2e/mutable-shares.spec.ts`

# Non-Goals (MVP)

- A `script-src` CSP directive. Needs nonce plumbing through `leptos_meta` and
  Monaco; tracked separately.
- Exposing metadata over the CLI (`ironpad notebook update`). The protocol now
  carries it, but `translate_command` has no `notebook.update` arm and never
  had one for any metadata field, including title.
- An `/embed/mutable` route, and therefore oEmbed for mutable shares.
- Per-notebook `og_image` upload. The field takes a root-relative path to an
  asset that already exists.

# History

- 2026-07-27: Shipped in v0.13.1 alongside the social-preview security patch
  (stored XSS, quadratic `ellipsize`, non-evicting card cache, soft-404s).
