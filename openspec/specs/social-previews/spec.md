# social-previews Specification

## Purpose
What a pasted ironpad link looks like on Reddit, X, Slack, Discord, and similar unfurlers, none of which run JavaScript: title, Open Graph, and Twitter Card metadata in the first server response, a generated 1200x630 preview card per notebook, an optional author-supplied preview image, and the crawler files (`robots.txt`, `sitemap.xml`) that decide what search engines index.

## Requirements
### Requirement: Preview metadata is in the first response
The system SHALL emit each page's title, Open Graph, and Twitter Card tags in the server-rendered `<head>` of the first response for `/`, `/public/{name}`, `/shared/{hash}`, and a visible `/mutable/{id}` reader, so a client that runs no JavaScript reads them. (PRD-0050 T-005)

#### Scenario: Raw body of a public notebook
- **WHEN** a client fetches `/public/welcome` without running scripts
- **THEN** the raw body carries `og:title` "Welcome to ironpad", `og:type` "article", `og:site_name` "ironpad", and an `og:description` containing "Learn the basics"
- **AND** it contains `<title>Welcome to ironpad · ironpad</title>`

#### Scenario: Pushed title reaches the unfurl
- **WHEN** the owner of a published notebook renames it to "Lifecycle One v2" and pushes
- **THEN** the raw body of `/mutable/{id}` carries `og:title` "Lifecycle One v2"

### Requirement: Preview tag set
The system SHALL emit `<title>{title} · ironpad</title>`, a canonical link, `description`, `og:type`, `og:site_name`, `og:title`, `og:description`, `og:url`, `og:image`, `og:image:type`, `og:image:width`, `og:image:height`, `og:image:alt` "Preview card for the ironpad notebook \"{title}\"", and `twitter:card` "summary_large_image" with `twitter:title`, `twitter:description`, and `twitter:image`. (PRD-0050 T-005, PRD-0051)

#### Scenario: Generated card announced at its real size
- **WHEN** a client fetches the raw body of `/public/welcome`
- **THEN** `og:image` and `twitter:image` are `{origin}/og/public/welcome.png`
- **AND** `og:image:width` is "1200", `og:image:height` is "630", and `twitter:card` is "summary_large_image"
- **AND** a `rel="canonical"` link points at `{origin}/public/welcome`

### Requirement: Absolute URLs from the configured public origin
The system SHALL make `og:url`, `og:image`, `twitter:image`, the canonical link, sitemap entries, and the `robots.txt` sitemap pointer absolute against `--public-url` / `IRONPAD_PUBLIC_URL`, which defaults to `http://localhost:{port}`, tolerating a trailing slash on the configured value. (PRD-0050 T-001, PRD-0051)

#### Scenario: Default origin follows the port
- **WHEN** the server starts with `--port 8080` and no public URL
- **THEN** the public origin is `http://localhost:8080`

#### Scenario: Configured origin
- **WHEN** the server starts with `--public-url https://ironpad.twitchax.com/`
- **THEN** the card for `cannon` is advertised as `https://ironpad.twitchax.com/og/public/cannon.png`, with no doubled slash

### Requirement: Canonical URL for a legacy public name
The system SHALL advertise the extension-less canonical URL in `og:url`, `og:image`, and the canonical link even when `/public/{name}` was requested with the `.ironpad` suffix. (PRD-0048, PRD-0050 T-005)

#### Scenario: Suffixed request
- **WHEN** a client fetches the raw body of `/public/welcome.ironpad`
- **THEN** `og:url` is `{origin}/public/welcome` and `og:image` is `{origin}/og/public/welcome.png`

### Requirement: Description fallback and clamp
The system SHALL use the notebook's description for `description`, `og:description`, and `twitter:description`, falling back to "An interactive Rust notebook on ironpad." when it has none, and SHALL cut a description over 200 characters at a word boundary, drop trailing punctuation, and end it with "…". (PRD-0050 T-005)

#### Scenario: Long description
- **WHEN** a notebook's description is 100 repetitions of "word "
- **THEN** the emitted description is at most 201 characters, ends with "…", and does not cut through a word

#### Scenario: Editor description reaches a share
- **WHEN** an author sets a description in the metadata panel and shares the notebook immutably
- **THEN** the share's raw body carries that exact text as `og:description`

### Requirement: Titles cannot inject markup
The system MUST strip `<`, `>`, and every control character except tab from a notebook title before it reaches the page title or any preview tag, and trim surrounding whitespace, since titles on `/shared` and `/mutable` are attacker-controlled. (PRD-0050 T-005, PRD-0051)

#### Scenario: Title that closes the title element
- **WHEN** a notebook titled `</title><script>window.PWNED=1</script>` is shared and its raw body fetched
- **THEN** the text inside `<title>` contains no `<` or `>` and still contains "ironpad"
- **AND** the document contains no `<script>window.PWNED`

#### Scenario: Ordinary titles pass through
- **WHEN** a title is "Conformal maps: z ↦ z²"
- **THEN** it is emitted unchanged

### Requirement: Home page unfurls as the site
The system SHALL give `/` site-level metadata: `og:type` "website", `og:title` "ironpad", the description "Write Rust in a notebook. Cells compile to WebAssembly and run in your browser.", and `og:image` `{origin}/og/ironpad.png`. (PRD-0050 T-005)

#### Scenario: Home page tags
- **WHEN** a client fetches the raw body of `/`
- **THEN** `og:type` is "website", `og:title` is "ironpad", `og:image` is `{origin}/og/ironpad.png`, and there is no robots meta tag

### Requirement: Unlisted pages unfurl but are not indexed
The system SHALL mark `/shared/{hash}` and `/mutable/{id}` reader pages with `<meta name="robots" content="noindex, follow">` and SHALL NOT mark `/` or `/public/{name}`, so share links still get a preview card but stay out of search results. (PRD-0050 T-005)

#### Scenario: Shared notebook
- **WHEN** a client fetches the raw body of `/shared/{hash}`
- **THEN** `og:title` is the notebook title, `og:url` is `{origin}/shared/{hash}`, and the robots meta contains "noindex"

#### Scenario: Showcase notebook is indexable
- **WHEN** a client fetches the raw body of `/public/welcome`
- **THEN** there is no robots meta tag

### Requirement: Author-supplied preview image
The system SHALL use a notebook's `og_image` as `og:image` only when it is a root-relative path (starts with `/` but not `//`); any other value, including an absolute URL, a protocol-relative URL, a relative path, or a `javascript:` URL, SHALL fall back to the generated card `/og/{class}/{id}.png`. (PRD-0050 T-002)

#### Scenario: Root-relative override
- **WHEN** a notebook's `og_image` is `/og-custom/mandelbrot.png`
- **THEN** `og:image` is `{origin}/og-custom/mandelbrot.png`

#### Scenario: Foreign override is ignored
- **WHEN** a shared notebook's `og_image` is `https://evil.example/x.png` or `//evil.example/x.png`
- **THEN** `og:image` is the generated card `{origin}/og/shared/{hash}.png`

### Requirement: Declared size of an override image
The system SHALL announce `og_image_width` and `og_image_height` as the image size only when the notebook has a usable `og_image` override and both values lie between 200 and 8192 pixels inclusive; otherwise it SHALL announce 1200x630. (PRD-0051)

#### Scenario: Square screenshot
- **WHEN** a notebook overrides `og_image` with `/og-custom/mandelbrot.png` and declares 1024x1024
- **THEN** `og:image:width` and `og:image:height` are "1024"

#### Scenario: Unusable declarations
- **WHEN** a notebook declares only one axis, a size of 199 or 8193, or dimensions without a usable override
- **THEN** the page announces 1200x630

### Requirement: Generated notebook cards
The system SHALL serve `GET /og/{class}/{id}.png` for class `public`, `shared`, or `mutable` as a 1200x630 PNG with `Content-Type: image/png` and `Cache-Control: public, max-age=3600`, and SHALL answer 404 for any other class, a path without the `.png` suffix, an id that is not one safe path segment, or a notebook no reader may see. (PRD-0050 T-004)

#### Scenario: Public card
- **WHEN** a client requests `/og/public/welcome.png`
- **THEN** the response is 200 `image/png`, the body starts with the PNG signature, and its header declares width 1200 and height 630

#### Scenario: Rejected card paths
- **WHEN** a client requests `/og/local/whatever.png`, `/og/public/nope-does-not-exist.png`, `/og/public/welcome`, or `/og/public/%2e%2e%2f%2e%2e%2fetc%2fpasswd.png`
- **THEN** each answers 404

### Requirement: Card content
The system SHALL draw on each card the "ironpad" wordmark, a class label ("PUBLIC NOTEBOOK", or "SHARED NOTEBOOK" for both shared and published notebooks), the title in up to two lines and the description in up to two, up to six lines of the first runnable code cell, a footer such as "12 cells", up to four tag chips that fit, and the public host. (PRD-0050 T-004)

#### Scenario: Code excerpt source
- **WHEN** a notebook's cells in order are markdown, a shared code cell, `let answer = 42;`, and `let later = 1;`
- **THEN** the card's excerpt is `let answer = 42;`

#### Scenario: Excerpt starts on ink
- **WHEN** the first runnable cell opens with three blank lines followed by twenty statements
- **THEN** the excerpt skips the blank lines and starts at the first statement

#### Scenario: Long title
- **WHEN** a title is too long for two lines
- **THEN** the second line ends with an ellipsis and the code panel shrinks rather than overflowing

### Requirement: Cards survive hostile notebook text
The system MUST render a card for any notebook text: markup characters are escaped, XML-illegal control characters are dropped, and each text field is clipped to 512 characters (16 tags considered) before layout. (PRD-0050 T-004, PRD-0051)

#### Scenario: Script in a shared title
- **WHEN** a shared notebook titled `</title><script>window.PWNED=1</script>` has its card requested
- **THEN** `/og/shared/{hash}.png` answers 200 with a PNG

#### Scenario: Control character in a title
- **WHEN** a notebook title contains U+0001
- **THEN** its card renders rather than answering 500

### Requirement: Site card
The system SHALL serve `GET /og/ironpad.png` as a 1200x630 PNG card labelled "INTERACTIVE RUST NOTEBOOKS" with the title "ironpad", the description "Write Rust in a notebook. Cells compile to WebAssembly and run in your browser.", and a footer counting the bundled public notebooks, such as "48 notebooks". (PRD-0050 T-004)

#### Scenario: Site card renders
- **WHEN** a client requests `/og/ironpad.png`
- **THEN** the response is 200 `image/png` with width 1200

### Requirement: Card render cache
The system SHALL cache rendered cards under `{data_dir}/og/`, keyed on the card's drawing and the release version, render one card once when several crawlers request it at the same time, evict oldest-first down to 48 MiB once the cache would pass 64 MiB, and still serve a card whose cache write failed. (PRD-0050 T-004, PRD-0051)

#### Scenario: Repeat request
- **WHEN** the same card is requested twice
- **THEN** both responses carry identical bytes and the second is read from `{data_dir}/og/{key}.png`

#### Scenario: Full cache
- **WHEN** the cache directory already holds more than 64 MiB and a new card renders
- **THEN** the oldest entries are deleted, the new card is cached, and the directory ends at or below 64 MiB

### Requirement: Cards do not depend on host fonts
The system SHALL render card text with fonts built into the server binary (Inter and JetBrains Mono), so a card looks the same on a development machine, in CI, and on a runtime image with no system fonts. (PRD-0050 T-003)

#### Scenario: Fonts-free runtime image
- **WHEN** the server runs on an image that ships no fonts and a card is requested
- **THEN** the card's title, description, and code text render

### Requirement: robots.txt
The system SHALL serve `GET /robots.txt` as `text/plain` containing exactly the rules `User-agent: *`, `Disallow: /embed/`, `Disallow: /admin`, `Allow: /`, and `Sitemap: {origin}/sitemap.xml`, and SHALL NOT disallow `/shared` or `/mutable`, which several unfurlers would then refuse to preview. (PRD-0050 T-006, PRD-0063 T-004)

#### Scenario: Rules
- **WHEN** a crawler fetches `/robots.txt`
- **THEN** the only `Disallow` lines are `/embed/` and `/admin`, in that order
- **AND** it names `Sitemap: {origin}/sitemap.xml`

### Requirement: sitemap.xml
The system SHALL serve `GET /sitemap.xml` as `application/xml` listing `{origin}/` and every bundled public notebook at its extension-less canonical URL, XML-escaped, and SHALL list no `/shared`, `/mutable`, or `/local` URL. (PRD-0050 T-006)

#### Scenario: Showcase notebooks by canonical route
- **WHEN** a crawler fetches `/sitemap.xml`
- **THEN** it contains `<loc>{origin}/</loc>`, `<loc>{origin}/public/welcome</loc>`, and `<loc>{origin}/public/cannon</loc>`
- **AND** no `<loc>` contains `.ironpad`

#### Scenario: Unsafe characters in a file name
- **WHEN** a public notebook file is named `a&b.ironpad`, or its name contains U+0001
- **THEN** its entry reads `/public/a&amp;b`, or drops the control character, and the sitemap stays parseable
