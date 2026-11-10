# embedding Specification

## Purpose
Live, runnable notebooks inside third-party pages: chrome-less `/embed/{public,shared,mutable}/…` iframe routes, the dependency-free `embed.js` loader with auto-resizing frames, the Embed snippet button on view pages, and an oEmbed provider that hands discovery-capable consumers the running notebook instead of a picture of it. For bloggers and docs authors who want a notebook inline rather than a screenshot.

## Requirements
### Requirement: Embed routes
The system SHALL serve `/embed/public/{name}` (name with or without `.ironpad`), `/embed/shared/{hash}`, and `/embed/mutable/{id}` as iframe-ready pages that render the notebook through the same read-only renderer as the full pages, with cells runnable and saved outputs shown. (PRD-0039, PRD-0048, PRD-0057)

#### Scenario: Public embed renders
- **WHEN** a client opens `/embed/public/welcome.ironpad`
- **THEN** the view-only notebook renders with "Run All" visible

#### Scenario: Published notebook embed
- **WHEN** an author publishes a notebook and a client opens `/embed/mutable/{id}`
- **THEN** the published copy renders, never the author's unpushed draft

### Requirement: Embeds are chrome-less
The system SHALL render embed routes with no app header, no status bar, no outline rail, no breadcrumb crumb, no "read-only" pill, no "Fork" button, and no "Embed" button, while keeping "Run All" and the Cached/Fresh toggle, and SHALL let the document grow to its content height. (PRD-0039, PRD-0065)

#### Scenario: Nothing but the notebook
- **WHEN** a client opens `/embed/public/charts-with-plot`
- **THEN** there is no `Notebook outline` navigation, no breadcrumb crumb, no "read-only" pill, no header, and no status bar

#### Scenario: Fork is absent
- **WHEN** a client opens `/embed/public/welcome.ironpad`
- **THEN** no Fork button renders, since browser-local storage is partitioned inside a cross-origin iframe

### Requirement: Open-in-ironpad badge
The system SHALL end every embed with a "Powered by ironpad · Open" badge linking to the notebook's canonical full page in a new tab: `/public/{name}` without the suffix, `/shared/{hash}`, or `/mutable/{id}`. (PRD-0039, PRD-0048, PRD-0057)

#### Scenario: Legacy public spec links to the canonical page
- **WHEN** a client opens `/embed/public/welcome.ironpad`
- **THEN** the badge's `href` is `/public/welcome`

#### Scenario: Mutable embed badge
- **WHEN** a client opens `/embed/mutable/{id}`
- **THEN** the badge's `href` is `/mutable/{id}`

### Requirement: Click-to-run by default
The system SHALL leave embeds idle on load unless the embedder opts in: `/embed/public/{name}` SHALL run every runnable code cell on load when its query carries `autorun=1` or `autorun=true`, and `/embed/shared` and `/embed/mutable` SHALL ignore that parameter. A notebook's saved Reactive Mode SHALL NOT start an embed. (PRD-0040, PRD-0057)

#### Scenario: Default public embed stays idle
- **WHEN** a client opens `/embed/public/welcome.ironpad` and waits 3 seconds
- **THEN** no timing badge appears

#### Scenario: Embedder opts in
- **WHEN** a client opens `/embed/public/welcome.ironpad?autorun=1`
- **THEN** cells compile and run without a click and timing badges appear

#### Scenario: Shared and published embeds refuse autorun
- **WHEN** a client opens `/embed/shared/{hash}?autorun=1` or `/embed/mutable/{id}?autorun=1`
- **THEN** no cell runs until the reader clicks a run control

#### Scenario: Reactive notebooks wait like any other
- **WHEN** a client opens `/embed/public/reactive-demo.ironpad`, `/embed/shared/{hash}` or `/embed/mutable/{id}` for a notebook saved in Reactive Mode, without `autorun`
- **THEN** the embed neither compiles a cell nor fetches a share blob on load
- **AND** clicking Run All starts that traffic

### Requirement: Compact error panels in embeds
The system SHALL render an embed whose notebook cannot be shown as one terse error panel inside the frame, with no app chrome: "Public notebook not found", "Shared notebook not found or expired", or "Published notebook not found", each followed by the reason. (PRD-0039, PRD-0057)

#### Scenario: Missing public notebook
- **WHEN** a third-party page frames `/embed/public/no-such-notebook`
- **THEN** the frame shows "Public notebook not found:" and the reason, with no header or status bar

#### Scenario: Unknown published-notebook id
- **WHEN** a client opens `/embed/mutable/{id}` for an id with no published copy
- **THEN** the frame shows the "Published notebook not found" panel

### Requirement: embed.js snippet styles
The system SHALL serve `/embed.js`, which mounts an iframe for each `.ironpad-embed[data-notebook]` placeholder on the page (each at most once, marked `data-ironpad-mounted`) and, when its own script tag carries `data-notebook`, for a holder inserted just before that tag, deriving the ironpad origin from the script's own `src`. (PRD-0039)

#### Scenario: Script tag style
- **WHEN** a host page on another origin includes `<script src="{origin}/embed.js" data-notebook="public/welcome.ironpad">`
- **THEN** an iframe with an id beginning `ironpad-embed-` appears and the notebook renders inside it

#### Scenario: Several placeholders, one script
- **WHEN** a host page has two `.ironpad-embed` divs naming `public/welcome.ironpad` and `public/tutorial.ironpad` and one `embed.js` tag
- **THEN** two iframes mount with distinct ids

### Requirement: Loader input and frame shape
The system SHALL accept a `data-notebook` value only when it matches `{shared|public|mutable}/{id}` with an id of letters, digits, `.`, `_`, or `-`, ignoring any other value, and SHALL create the frame at `{origin}/embed/{class}/{id}?embed_id={frame id}`, 100% wide, borderless, 480px tall until resized, lazy-loaded, titled "ironpad notebook", allowing `clipboard-write`. (PRD-0039, PRD-0057)

#### Scenario: Malformed spec
- **WHEN** a placeholder carries `data-notebook="local/abc"` or `data-notebook="public/../x"`
- **THEN** no iframe is mounted for it

#### Scenario: data-autorun is forwarded
- **WHEN** a script tag or placeholder carries `data-autorun`
- **THEN** its iframe `src` gains `&autorun=1`

### Requirement: Auto-resize height protocol
The system SHALL size embed frames to their content: a framed embed page opened with `embed_id` SHALL post `{type: "ironpad:embed:height", id, height}` to its parent whenever its content height changes (checked every 400ms, plus 4px), and the loader SHALL apply only messages from the ironpad origin naming that frame's id with a finite positive height. (PRD-0039)

#### Scenario: Placeholder height is replaced
- **WHEN** an `embed.js` frame finishes rendering its notebook on a host page
- **THEN** the iframe's style height changes from `480px` to the reported content height, rounded up

#### Scenario: Foreign or cross-talk messages
- **WHEN** the host window receives a height message from another origin, or one carrying a different embed id
- **THEN** no frame is resized by it

### Requirement: Threaded-cell limitation notice
The system SHALL show, above the cells of an embed that is not cross-origin isolated, the notice "This notebook uses threaded (rayon) cells, which need a cross-origin-isolated page and can't run inside an embed. Plain and async cells run fine; use the badge below to open the full notebook." when any non-shared code cell's dependencies include `rayon`. (PRD-0039)

#### Scenario: Rayon dependency in a cell or the shared manifest
- **WHEN** an embedded notebook has a code cell whose `Cargo.toml`, or the notebook's shared `Cargo.toml`, declares `rayon = "1"`
- **THEN** the notice shows

#### Scenario: Things that are not a rayon dependency
- **WHEN** the only mention of rayon is a feature named `rayon` on another crate, a TOML comment, or a dependency of a Linux cell
- **THEN** no notice shows

### Requirement: Third parties may frame embeds
The system SHALL let any origin frame the embed routes: responses for `/embed/*`, `/embed.js`, and `/embed-frame.js` SHALL carry `Cross-Origin-Resource-Policy: cross-origin`, and no response SHALL carry a `frame-ancestors` policy or `X-Frame-Options`. (PRD-0039, PRD-0051)

#### Scenario: Embed response headers
- **WHEN** a client requests `/embed/public/welcome`
- **THEN** the response carries `Cross-Origin-Resource-Policy: cross-origin`
- **AND** its `Content-Security-Policy` contains no `frame-ancestors`

#### Scenario: Non-embed paths
- **WHEN** a client requests `/shared/{hash}` or `/embedx`
- **THEN** no `Cross-Origin-Resource-Policy: cross-origin` header is added

### Requirement: Embed snippet button
The system SHALL show an "Embed" button on `/public`, `/shared`, and `/mutable` reader pages whose popover offers an "iframe" snippet and a "script (auto-resizing)" snippet for that notebook, built from the page's own origin and the canonical extension-less spec, each with a copy button. (PRD-0039, PRD-0048, PRD-0057)

#### Scenario: Snippets on a public notebook
- **WHEN** a reader on `/public/welcome` opens the Embed popover
- **THEN** it shows `<iframe src="{origin}/embed/public/welcome" style="width:100%;border:0;" height="600" loading="lazy" title="ironpad notebook"></iframe>`
- **AND** `<script src="{origin}/embed.js" data-notebook="public/welcome" async></script>`

#### Scenario: Copy
- **WHEN** the reader clicks the first snippet's copy button
- **THEN** the clipboard holds the iframe snippet containing `/embed/public/welcome"`

### Requirement: oEmbed endpoint
The system SHALL answer `GET /oembed?url={notebook URL}` with an oEmbed 1.0 `rich` JSON response whose `html` is an iframe of the notebook's `/embed/*` route, with `provider_name` "ironpad", the notebook's title, `width` 800, and `cache_age` 86400, served as `application/json` with `Cache-Control: public, max-age=3600`. (PRD-0051, PRD-0057)

#### Scenario: Public notebook
- **WHEN** a consumer requests `/oembed?url={origin}/public/welcome`
- **THEN** the response is 200 with `version` "1.0", `type` "rich", and `title` "Welcome to ironpad"
- **AND** `html` starts with `<iframe ` and contains `{origin}/embed/public/welcome`

#### Scenario: Published notebook
- **WHEN** a consumer requests `/oembed?url={origin}/mutable/{id}` for a notebook published as "Embeddable Published"
- **THEN** `html` contains `{origin}/embed/mutable/{id}` and `title` is "Embeddable Published"

### Requirement: oEmbed accepts only this origin's notebook URLs
The system SHALL resolve only URLs that begin with the configured public origin, scheme included, followed by `/public/`, `/shared/`, or `/mutable/` and one safe path segment, ignoring any query or fragment and one `.ironpad` suffix; anything else SHALL be a 404 "not an embeddable ironpad URL". (PRD-0051, PRD-0057)

#### Scenario: Foreign and look-alike origins
- **WHEN** a consumer passes `https://evil.example/public/welcome`, `{origin}.evil.example/public/welcome`, an `http://` form of an `https://` origin, `//host/public/welcome`, or `/public/welcome`
- **THEN** the response is 404

#### Scenario: Non-notebook routes and traversal
- **WHEN** a consumer passes `{origin}/local/some-uuid`, `{origin}/`, `{origin}/publicity/x`, `{origin}/public/a/b`, or `{origin}/mutable/../etc`
- **THEN** the response is 404

#### Scenario: Query, fragment, and legacy suffix
- **WHEN** a consumer passes `{origin}/public/cannon.ironpad`, `{origin}/public/cannon?utm_source=reddit`, or `{origin}/public/cannon#cell-3`
- **THEN** the embed iframe points at `{origin}/embed/public/cannon`

### Requirement: oEmbed resolves only readable notebooks
The system SHALL answer 404 "no such notebook" when a well-formed URL maps to no notebook a reader may see, giving the same answer for a missing, private, or unpublished one, and SHALL take a published notebook's title from its published copy, never the owner's draft. (PRD-0051, PRD-0057)

#### Scenario: Missing notebook
- **WHEN** a consumer requests `/oembed?url={origin}/public/no-such-notebook` or `/oembed?url={origin}/mutable/aaaa1111bbbb2222`
- **THEN** the response is 404

#### Scenario: Draft rename not yet pushed
- **WHEN** the owner of a notebook published as "Lifecycle One" renames its draft to "Lifecycle One v2" without pushing
- **THEN** oEmbed for `{origin}/mutable/{id}` returns `title` "Lifecycle One"

### Requirement: oEmbed format and size parameters
The system SHALL answer 501 "only json is supported" when `format` is anything other than `json` (case-insensitive), and SHALL set the iframe and response `height` from `maxheight` clamped to 150 through 4000, defaulting to 600. (PRD-0051)

#### Scenario: XML requested
- **WHEN** a consumer adds `&format=xml`
- **THEN** the response status is 501

#### Scenario: maxheight
- **WHEN** a consumer passes `maxheight` of 900, 99999, and 1
- **THEN** `height` is 900, 4000, and 150 respectively

### Requirement: oEmbed HTML is escaped
The system MUST entity-escape the notebook title and embed URL inside the returned `html`, so a hostile title cannot break out of the iframe tag, while the JSON `title` field carries the raw title. (PRD-0051)

#### Scenario: Hostile shared title
- **WHEN** a shared notebook titled `"><script>alert(1)</script>` is resolved
- **THEN** `html` contains no `<script>` and ends with `></iframe>`
- **AND** `title` still contains `<script>`

### Requirement: oEmbed discovery link
The system SHALL advertise the provider in the server-rendered head of `/public/{name}`, `/shared/{hash}`, and a visible `/mutable/{id}` reader as `<link rel="alternate" type="application/json+oembed" title="ironpad notebook">` whose `href` is `{origin}/oembed?url={percent-encoded canonical page URL}&format=json`. (PRD-0051, PRD-0057)

#### Scenario: Discoverable without JavaScript
- **WHEN** a consumer fetches the raw body of `/public/welcome`
- **THEN** it contains the `application/json+oembed` link with `href` containing `/oembed?url=` and the percent-encoded `{origin}/public/welcome`
- **AND** following that `href` returns a `rich` response

#### Scenario: Published notebook reader
- **WHEN** a consumer fetches the raw body of `/mutable/{id}` for a published notebook
- **THEN** it contains the `application/json+oembed` link
