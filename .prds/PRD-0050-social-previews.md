---
id: PRD-0050
title: "Social previews: Open Graph metadata and generated preview cards"
status: done
owner: "Aaron Roney"
created: 2026-07-26
updated: 2026-07-26

depends_on:
- PRD-0048

principles:
- "Crawlers do not run JavaScript: metadata must be in the first SSR response or it does not exist."
- "Every storage class gets a card, including /shared and /mutable, which is why the card is generated at runtime rather than screenshotted at build time."
- "Deterministic rendering: fonts are compiled into the binary so a card looks the same on a dev box, in CI, and on the fonts-free runtime image."
- "Unlisted is not the same as secret: shared links unfurl, but they carry noindex rather than a robots.txt Disallow, because several unfurlers honour robots.txt and would refuse to build a preview."
- "Notebook content is attacker-controlled on /shared and /mutable; it is escaped before it reaches an SVG and validated before it reaches an og:image URL."

references:
- name: "The Open Graph protocol"
  url: https://ogp.me/
- name: "X: summary_large_image card"
  url: https://developer.x.com/en/docs/twitter-for-websites/cards/overview/summary-card-with-large-image
- name: "resvg"
  url: https://github.com/linebender/resvg

acceptance_tests:
- id: uat-001
  name: "A public notebook serves og:title, og:description, and an absolute og:image in the raw SSR response, with no JavaScript run"
  command: npx playwright test social-preview
  uat_status: verified
- id: uat-002
  name: "/og/{class}/{id}.png returns a 1200x630 PNG for public, shared, and mutable notebooks"
  command: npx playwright test social-preview mutable-shares
  uat_status: verified
- id: uat-003
  name: "Card routes 404 on unknown storage classes, missing notebooks, and path traversal"
  command: npx playwright test social-preview
  uat_status: verified
- id: uat-004
  name: "robots.txt and sitemap.xml resolve; the sitemap lists public notebooks by canonical extension-less route"
  command: npx playwright test social-preview
  uat_status: verified
- id: uat-005
  name: "Shared notebooks unfurl but carry noindex"
  command: npx playwright test social-preview mutable-shares
  uat_status: verified
- id: uat-006
  name: "A hostile notebook title cannot inject markup into the card SVG"
  command: cargo make test
  uat_status: verified

tasks:
- id: T-001
  title: "Add public_url to AppConfig and IRONPAD_PUBLIC_URL to the CLI"
  priority: 1
  status: done
  notes: "Defaults to http://localhost:{port} derived from the resolved port. Set in .hidden/fly.toml."
- id: T-002
  title: "Add the og_image override field to IronpadNotebook"
  priority: 1
  status: done
  notes: "Root-relative paths only, enforced by og_image_path() so a share cannot point a crawler at another origin."
- id: T-003
  title: "Vendor Inter and JetBrains Mono under crates/ironpad-server/assets/fonts"
  priority: 1
  status: done
  notes: "Embedded with include_bytes!. The runtime image is rust:slim and ships no fonts, so discovery would work on a dev box and find nothing in prod."
- id: T-004
  title: "Build the card renderer: SVG layout, text metrics, resvg rasterization, disk cache"
  priority: 1
  status: done
  notes: "og/text.rs measures advance widths with ttf-parser because SVG has no text wrapping; og/svg.rs is pure and unit-testable; og/mod.rs owns extraction, caching, and the handlers."
- id: T-005
  title: "Emit per-page metadata and switch the notebook routes to SsrMode::Async"
  priority: 1
  status: done
  notes: "The SsrMode change is load-bearing: under streaming SSR the head is flushed before the notebook Resource resolves and leptos_meta patches the tags in with a script no crawler runs."
- id: T-006
  title: "Add robots.txt and sitemap.xml"
  priority: 2
  status: done
  notes: "Both were 404s. Sitemap enumerates site_root/notebooks at request time, matching how the home page lists them."
- id: T-007
  title: "Test and document"
  priority: 2
  status: done
  notes: "Playwright asserts against raw response bodies rather than the hydrated DOM, since that is what an unfurler sees."
---

# Summary

Give every ironpad URL a real link preview: a title, a description, and a
generated 1200x630 card image, served in the first SSR response so that
Reddit, X, Slack, and Discord can render it.

# Problem

Before this, every URL on the site unfurled as the bare word **ironpad** with
no description and no image:

```
$ curl -A "Twitterbot/1.0" https://ironpad.twitchax.com/public/cannon | grep -c 'og:'
0
<title>ironpad</title>
```

There was a single global `<Title text="ironpad"/>` and no Open Graph tags at
all, so a posted link showed the domain and nothing else. `robots.txt` and
`sitemap.xml` both 404'd, leaving the 45 showcase notebooks discoverable only
by crawling the home page's list.

The body was already server-rendered, so search engines could read the prose.
This is narrowly a `<head>` problem.

# Goals

1. Per-page `<title>`, `og:*`, and `twitter:*` tags on `/`, `/public/{name}`,
   `/shared/{hash}`, and `/mutable/{id}`.
2. A preview image for each, generated from notebook metadata.
3. `robots.txt` and `sitemap.xml`.
4. An escape hatch for notebooks whose subject is visual, where a text card
   undersells the content.

# Technical Approach

## The streaming trap

The notebook title comes from a `Resource`, and Leptos defaults to
out-of-order streaming SSR: the `<head>` goes out at first byte, long before
the resource resolves. `leptos_meta` then patches the tags in afterwards with
a script. That is correct for a browser and useless for a crawler, none of
which run JavaScript, so the tags would look right in devtools and be
invisible to every unfurler.

The three server-backed notebook routes therefore render with
`SsrMode::Async`. All three read from local disk, so the cost is a file read
before the first byte. `/local/{id}` stays streaming: it loads from IndexedDB
in the browser, so there is nothing to await and nothing to crawl.

## The card

```
GET /og/public/cannon.png
  -> load the notebook (same _core loaders the server functions use)
  -> build a Card: title, description, tags, cell count, code excerpt
  -> lay out an SVG, measuring with real font advance widths
  -> rasterize with resvg
  -> cache at {data_dir}/og/{blake3}.png
```

Three modules, split by concern:

- **`og/text.rs`** — the embedded faces plus advance-width measurement. SVG
  has no text wrapping, so every line is positioned explicitly and the layout
  has to know how wide a string is before writing it.
- **`og/svg.rs`** — pure `Card` to SVG, including a small Rust highlighter for
  the code excerpt. No filesystem, no rasterizer, so the whole layout is
  unit-testable.
- **`og/mod.rs`** — extraction from a notebook, the disk cache, and the axum
  handlers.

The cache key is a blake3 of the finished SVG plus the release version, so
every input that can change a pixel is covered (notebook text, layout
constants, palette) and a font or `resvg` bump invalidates through the
version.

## Indexing

`robots.txt` disallows only `/embed/`, which is duplicate content. Shared and
mutable notebooks are unlisted rather than secret, and they get
`<meta name="robots" content="noindex, follow">` on the page instead of a
`Disallow`. The distinction matters: several unfurlers (Twitterbot among them)
honour `robots.txt` and would decline to build a preview at all, whereas
`noindex` is read by search engines and ignored by unfurlers. The result is a
share that previews when pasted but does not land in Google.

# Assumptions

- `IRONPAD_PUBLIC_URL` is set correctly in production. Client-side the origin
  comes from `window.location.origin`, which matches on any correct deploy.
- Unfurlers cache aggressively at first fetch. Reddit in particular snapshots
  a preview when a link is first submitted, so this does not retroactively fix
  links already posted.

# Constraints

- The runtime image (`rust:1.93.0-slim`) has no system fonts, which is why the
  faces are embedded rather than discovered.
- `resvg` is built with `default-features = false, features = ["text"]`: no
  raster-image decoders, and explicitly no `system-fonts`.
- Cards are text. A notebook whose point is a picture uses `og_image`.

# References to Code

- `crates/ironpad-server/src/og/` — the renderer
- `crates/ironpad-server/src/crawl.rs` — robots.txt and sitemap.xml
- `crates/ironpad-app/src/components/social_meta.rs` — the meta tag block
- `crates/ironpad-app/src/lib.rs` — `SsrMode::Async` on the notebook routes
- `crates/ironpad-common/src/config.rs` — `public_url` and `absolute_url`
- `crates/ironpad-common/src/types.rs` — `og_image` and `og_image_path`
- `tests/e2e/social-preview.spec.ts`

# Non-Goals (MVP)

- Screenshotting real notebook output. It needs a headless browser in the
  image and cell execution per card, and it cannot serve `/shared` or
  `/mutable` at all. `og_image` covers the handful of notebooks where it
  would matter.
- Per-notebook card design. One layout, adapting to how much title and
  description there is.
- `lastmod` in the sitemap: the public-notebook summary does not carry
  `updated_at`, and inventing a timestamp is worse than omitting the field.

# History

- 2026-07-26: PRD created; T-001 through T-007 implemented. Full gate green:
  `cargo make ci` 725 tests, `cargo make test-integration` 12 tests,
  `cargo make playwright` 80 passed / 1 skipped. Verified against a live
  server that `/public/cannon` now serves 16 `og:`/`twitter:` tags in the
  first response where it previously served zero.
- 2026-07-27: Review follow-up (shipped as v0.13.1, see PRD-0051). A five-agent
  review found a stored XSS in the `<title>` splice, a quadratic `ellipsize`
  giving a one-GET remote DoS, C0 controls turning a card into a permanent 500,
  a card cache that refused writes at capacity and never evicted, and soft-404s
  on the three async routes. All fixed with regression tests. uat-002 and
  uat-005 named `/mutable` but ran only `social-preview`, which does not touch
  it; their commands now also run `mutable-shares`, which gained the missing
  unfurl assertion.
