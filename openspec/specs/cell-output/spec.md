# cell-output Specification

## Purpose
How the browser renders what a cell yields, in the editor and on read-only pages: the output panel and its timing, each panel type, sanitization of untrusted HTML, SVG, and Markdown, KaTeX math, copy buttons, height limits, compile and runtime error display, playback controls for animations, simulations, and live views, theme-aware charts, and the badged saved-output snapshot readers see before they run anything.

## Requirements
### Requirement: Editor output panel
The system SHALL show, below an editor cell that has run, an output panel whose header reads `Output` and the piped byte count and runtime as `N bytes · X.Xms` (`1 byte` when singular), whose body renders the cell's display panels in order, and whose header click collapses and expands the body; a cell that has not run shows no output panel. (PRD-0001, PRD-0014)

#### Scenario: A finished run
- **WHEN** a cell ending with `42` runs to success
- **THEN** an output panel appears whose header reads `Output` and `1 byte · ` followed by the runtime in milliseconds to one decimal
- **AND** its body shows a text panel reading `42`

#### Scenario: Collapsing the output
- **WHEN** the reader clicks the output header
- **THEN** the panels are hidden while the header stays, and a second click shows them again

### Requirement: Read-only output panel
The system SHALL show a live result on a read-only page under a caption reading `N bytes · X.X ms` (plus the main-thread badge when it applies) that collapses and expands the output when clicked; a cell whose author saved its output as collapsed shows an `Output` button instead, which reveals the output. (PRD-0014, PRD-0065)

#### Scenario: Collapsing a read-only output
- **WHEN** a reader runs a cell and clicks its output caption
- **THEN** the panels are hidden while the caption stays, and a second click shows them again

#### Scenario: An author-collapsed output
- **WHEN** a cell saved with its output collapsed is run on a read-only page
- **THEN** only an `Output` button shows until the reader clicks it

### Requirement: Raw output bytes
The system SHALL show a non-empty piped value in the editor output body as a `Raw output (N bytes)` disclosure, closed by default, holding a hex dump of 16 bytes per row (8-digit hex offset, hex bytes, printable ASCII with `.` for the rest) covering at most the first 1024 bytes; an empty piped value shows no disclosure. (PRD-0001, PRD-0008)

#### Scenario: A large value
- **WHEN** a cell pipes 4000 bytes and the reader opens `Raw output (4000 bytes)`
- **THEN** 64 rows of 16 bytes are shown followed by `... truncated (1024 of 4000 bytes shown)`

#### Scenario: Nothing piped
- **WHEN** a cell's output is `CellOutput::text("hi")`
- **THEN** the output body shows the text panel and no raw-output disclosure

### Requirement: Panel decoding and text panels
The system SHALL decode a cell's display output as a JSON array of panels and render display output that is not such an array as one text panel; a text panel SHALL show its string verbatim in a preformatted, line-wrapping block with no HTML interpretation, and a widget panel of an unknown kind SHALL render as `[unknown widget: <kind>]`. (PRD-0001)

#### Scenario: Plain text is not HTML
- **WHEN** a cell outputs the text `<b>not html</b>`
- **THEN** the panel shows the literal characters `<b>not html</b>`

#### Scenario: Legacy display text
- **WHEN** a cell's display output is the plain string `hello, world` rather than a panel array
- **THEN** it renders as a single text panel reading `hello, world`

### Requirement: Sanitized HTML and SVG output
The system SHALL sanitize every HTML panel, SVG panel, rendered Markdown panel, and HTML or Markdown live-view update against an allowlist before inserting it, removing scripts, event-handler attributes, `javascript:` and `data:` URLs, `foreignObject`, `use`, `image`, `animate`, and `set` elements, `xmlns`, and `id` except on SVG gradient, `clipPath`, `marker`, `mask`, and `pattern` elements, and keeping of a `class` attribute only the tokens `math`, `math-inline`, `math-display`, `ironpad-tooltips`, and `language-*`, so output cannot borrow a rule from the app's own stylesheet. (PRD-0036, PRD-0038)

#### Scenario: Script and handlers stripped
- **WHEN** a cell outputs `Html(r#"<p onclick="steal()">hi</p><script>steal()</script><img src=x onerror="steal()">"#.into())`
- **THEN** the panel shows the paragraph `hi` and contains no `<script`, `onclick`, `onerror`, or `steal`

#### Scenario: Drawing survives
- **WHEN** a cell outputs an SVG with `path`, `circle`, a `linearGradient id="g"`, and `fill="url(#g)"`
- **THEN** the drawing, the gradient id, and the `url(#g)` reference all survive, while an `id` on a `rect` or `div` is removed

#### Scenario: SVG escape hatches removed
- **WHEN** an SVG panel contains `<foreignObject><img src=x onerror="steal()"></foreignObject>`, `<use xlink:href="data:…">`, `<image href="data:…">`, or `<animate attributeName="href" to="javascript:steal()">`
- **THEN** each of those elements and its payload is absent from the page

#### Scenario: Data URLs
- **WHEN** an HTML panel contains `<a href="data:text/html,<script>steal()</script>">` or an `<img>` with a `data:` source
- **THEN** the `data:` URL is removed, so the image renders with no source

#### Scenario: A class naming an app overlay
- **WHEN** a Markdown cell on a shared notebook contains `<a href="https://evil.example" class="ironpad-history-overlay">` or `# T {.ironpad-history-overlay}`
- **THEN** the class is removed, and a click at the header or any viewport corner does not land on the link
- **AND** `class="math math-inline"` and a fenced block's `language-rust` survive

### Requirement: Inline style filtering
The system SHALL keep only presentation properties in sanitized `style` attributes (color, background, fonts, text, borders, spacing, sizing, flex and grid layout, transforms), dropping `position`, `z-index`, `inset`, `top`, `right`, `bottom`, `left`, `content`, and any property not on the allowlist, so cell output cannot overlay the page; and SHALL clip markdown, HTML, and live-view output to the container it is inserted into, scrolling wide content horizontally, so a `transform` or a negative margin cannot paint over the page either. (PRD-0038)

#### Scenario: A click-jacking overlay
- **WHEN** rendered output contains `<a class="hijack" style="position:fixed;inset:0;z-index:99999;background:rgb(255,0,0);color:rgb(0,0,255)">`
- **THEN** the link's computed `position` is `static` and `z-index` is `auto`
- **AND** its computed background color is still `rgb(255, 0, 0)`

#### Scenario: A transformed or negative-margin overlay
- **WHEN** a Markdown cell contains `<a style="display:block;width:40px;height:40px;transform:scale(80)">` or a 6000px block with `margin:-3000px`
- **THEN** it is clipped to its cell, and a click at the header or any viewport corner does not land on it

#### Scenario: Dashboard styling survives
- **WHEN** an HTML panel uses `style="background:#111;color:#eee;padding:8px;border-radius:6px;display:flex;gap:8px;width:200px"`
- **THEN** every one of those declarations is kept

### Requirement: Markdown panels
The system SHALL render a Markdown panel with tables, strikethrough, task lists, heading attributes, and math enabled, sanitize the result, and typeset `$…$` inline and `$$…$$` display math with KaTeX in the browser, leaving the source visible if a formula fails to typeset. (PRD-0003)

#### Scenario: Markdown with math
- **WHEN** a cell outputs `Md("## Results\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n$e^{i\\pi}+1=0$".into())`
- **THEN** the panel shows a heading, a two-column table, and a typeset formula

#### Scenario: Raw HTML in Markdown
- **WHEN** a Markdown panel contains `<script>steal()</script>`
- **THEN** the script is removed and math classes on the same panel still typeset

### Requirement: Table panels
The system SHALL render a table panel as an HTML table with one header row and one body row per data row, HTML-escaping every header and cell. (PRD-0003)

#### Scenario: Escaped cells
- **WHEN** a table has headers `a`, `<b>` and a row `1`, `&2`
- **THEN** the header reads the literal text `<b>` and the cell reads the literal text `&2`

### Requirement: Copy to clipboard
The system SHALL give each text, HTML, SVG, Markdown, and table panel a `Copy to clipboard` button that copies the raw text, the unsanitized markup or Markdown source, or tab-separated table rows respectively, and shows `Copied!` for 1.5 seconds; image, animation, simulation, live-view, and widget panels have no copy button. (PRD-0003)

#### Scenario: Copying a table
- **WHEN** the reader clicks the copy button on a table with headers `a`, `b` and rows `1`, `2` and `3`, `4`
- **THEN** the clipboard receives `a\tb\n1\t2\n3\t4` and the button shows `Copied!`

#### Scenario: Copying a chart
- **WHEN** the reader copies a `Plot` panel
- **THEN** the clipboard receives the chart's SVG markup

### Requirement: Image panels
The system SHALL render an image panel as an `<img>` at the panel's declared width and height with pixelated scaling, sourced from an in-memory object URL that is released when the panel leaves the page; in the editor, an image wider than the output scales down to fit. (PRD-0034)

#### Scenario: A canvas output
- **WHEN** a cell outputs an 800×600 `Canvas`
- **THEN** the panel holds an `<img width="800" height="600">` drawn with `image-rendering: pixelated`

### Requirement: Editor output height limit
The system SHALL cap the editor output body at 480 pixels tall with vertical scrolling inside it, the success summary at 300 pixels, and the diagnostics list at 400 pixels, so one cell's output cannot grow its card without bound. (PRD-0034)

#### Scenario: Very long text
- **WHEN** an editor cell outputs 2,000 lines of text
- **THEN** its output body stops at 480 pixels and scrolls internally

### Requirement: Run timing display
The system SHALL show run timings: in the editor, a successful cell's status badge reads its compile time as `N ms` and its summary reads `Compiled (S KB, Nms compile, Mms runtime[, cached])`; on read-only pages, the cell header reads `Nms compile · Mms run`, or `Nms compile` before the run ends. (PRD-0014, PRD-0065)

#### Scenario: Editor timings
- **WHEN** a cached cell compiles in 122 ms to a 189.7 KB module and runs in 45 ms
- **THEN** its badge reads `122 ms` and its summary reads `Compiled (189.7 KB, 122ms compile, 45ms runtime, cached)`

#### Scenario: Read-only timings
- **WHEN** a reader runs a cell that takes 900 ms to compile and 3 ms to run
- **THEN** the cell header shows `900ms compile · 3ms run`

#### Scenario: A failed compile on a read-only page
- **WHEN** the compile returns errors
- **THEN** no timing badge is shown and the error block is shown instead

### Requirement: Main-thread badge
The system SHALL mark an output produced by re-running the cell on the page's main thread, instead of in the executor worker, with a `main thread` badge in the output header whose tooltip explains that the cell needed DOM access, in the editor and on read-only pages. (PRD-0013)

#### Scenario: A chart renders in the worker
- **WHEN** a cell ending with `Plot::scatter(&data).title("Squares").x_label("x").y_label("x squared")` runs
- **THEN** the chart renders and the output header carries no `main thread` badge

### Requirement: Compile diagnostics in the editor
The system SHALL show a failed compile in the editor as a collapsible `Diagnostics` panel whose header summarizes counts (such as `2 errors, 1 warning`) and whose items show the severity, the code in brackets, the message, and each span as `line L:C1-C2` or `lines L1:C1 – L2:C2`, and SHALL mark the same spans inline in the cell's code editor. (PRD-0001)

#### Scenario: A type error
- **WHEN** a cell containing `let x: i32 = "not a number";` is run
- **THEN** the cell shows the error status, the code editor underlines the offending span, and the `Diagnostics` panel contains `mismatched types`

#### Scenario: Error codes link to the index
- **WHEN** a diagnostic carries the code `E0308`
- **THEN** `[E0308]` links to `https://doc.rust-lang.org/error_codes/E0308.html` in a new tab, while a lint code such as `[unused_variables]` is plain text

#### Scenario: Warnings on success
- **WHEN** a cell compiles with warnings and runs
- **THEN** the success summary is followed by a `Diagnostics` panel listing only the warnings

#### Scenario: The compile service is unreachable
- **WHEN** the compile request itself fails
- **THEN** the cell shows an error diagnostic reading `Server error:` followed by the failure

### Requirement: Runtime and read-only error display
The system SHALL show an editor cell that fails while running as an error status plus an output panel whose single text panel is the failure message, and SHALL show any failed cell on a read-only page as a preformatted error block holding the compile messages one per line, `Compile error:` plus the failure for a failed request, or the runtime failure message. (PRD-0001, 8244543)

#### Scenario: A panic in the editor
- **WHEN** an editor cell panics while running
- **THEN** its status shows `error` and its output panel reads `0 bytes · 0.0ms` above a text panel with the failure message

#### Scenario: A compile error on a read-only page
- **WHEN** a reader runs a cell that does not compile
- **THEN** the cell shows an error block listing each diagnostic message on its own line and no output panel

### Requirement: Animation playback
The system SHALL render an animation panel as a canvas at the animation's size that draws the first frame at once, then auto-plays and loops at its frame rate (1 frame per second when the rate is 0), with a play/pause button and a `Frame i/N` counter. (PRD-0017)

#### Scenario: Pausing an animation
- **WHEN** a 100-frame animation at 30 fps is playing and the reader clicks pause
- **THEN** the canvas holds its current frame, the counter stops, and play resumes from that frame

### Requirement: Simulation playback and sliders
The system SHALL render a live simulation panel as a canvas showing the first frame at once, then requesting a new frame at the simulation's frame rate, never while the previous request is outstanding, with play/pause and step buttons, a `Frame N` count of frames drawn, a `F fps` label, and each declared slider below as `<label>: <value>` and a range input that writes its key on the simulation bus. (PRD-0017, PRD-0018)

#### Scenario: Stepping a paused simulation
- **WHEN** the reader pauses a simulation and clicks step once
- **THEN** exactly one new frame is drawn and the frame count rises by one

#### Scenario: Slider defaults reach the bus
- **WHEN** a simulation declaring a slider `rod` with default 0.5 is shown
- **THEN** `0.5` is written to the bus under `rod` as the panel appears, and every later drag writes the new value

#### Scenario: Paused panels stop ticking
- **WHEN** a simulation is paused
- **THEN** no further frames are requested from the cell until play is pressed or step is clicked

### Requirement: Live view playback
The system SHALL render a live-view panel showing its initial content, then replacing it at the view's frame rate with each new tick's content (plain text as text, HTML sanitized, Markdown rendered and typeset with KaTeX), with the same play/pause, step, `Frame N`, and `F fps` controls as a simulation. (PRD-0019)

#### Scenario: Text content stays text
- **WHEN** a live view's tick returns `LiveContent::Text("<b>not html</b>".into())`
- **THEN** the panel shows those literal characters

#### Scenario: HTML content is sanitized
- **WHEN** a live view's tick returns `LiveContent::Html` containing `<script>` and an `onclick` handler
- **THEN** the panel shows the remaining markup without the script or the handler

### Requirement: Saved-output snapshots on read-only pages
The system SHALL render, on read-only notebook pages, each runnable cell's saved output (the panels from the author's last run) as its initial output with a dashed border, a caption reading `1 panel` or `N panels`, and the badge `Saved output from the author's last run. Press Run to execute live in your browser.`, until the cell's first live result or error replaces it. (PRD-0056)

#### Scenario: A cold reader
- **WHEN** an anonymous reader opens a shared notebook whose author ran a cell outputting `42`
- **THEN** the cell shows a saved output containing `42` and the badge, with no compile or run having happened

#### Scenario: Run replaces the snapshot
- **WHEN** the reader runs that cell
- **THEN** the saved output and its badge disappear and a live output with its `N bytes · X.X ms` caption shows `42`

#### Scenario: The editor shows no snapshots
- **WHEN** a notebook carrying saved outputs is open in the editor's edit mode
- **THEN** cells show no output until they are run

### Requirement: Snapshot rendering is static
The system SHALL render saved outputs through the same panel renderers and sanitizers as live outputs, except that a simulation shows a still of its first frame, a live view shows its captured initial content once, widgets neither change piped values nor run cells, and no WebAssembly module is loaded; animations replay from their embedded frames. (PRD-0056)

#### Scenario: A saved simulation
- **WHEN** a read-only page shows the saved output of a simulation cell
- **THEN** it shows the simulation's first frame and no frames advance until the reader runs the cell

#### Scenario: A hard load hydrates past a saved animation
- **WHEN** a reader hard-loads a share whose first cell's saved output is an animation or a simulation
- **THEN** the server renders the same canvas and controls the client hydrates, the page raises no error, and the later cells' controls work

#### Scenario: A saved image on a hard load
- **WHEN** a reader hard-loads a share whose saved output holds an image panel, such as a small `Canvas`
- **THEN** the image shows before any run, rather than an empty `src`

#### Scenario: Saved widgets
- **WHEN** a reader drags a slider in a saved output
- **THEN** no piped value changes and nothing runs

#### Scenario: An output too large to save
- **WHEN** a cell's panels exceeded 256 KiB when the notebook was shared
- **THEN** its saved output is one text panel reading `(output too large to embed; run the cell to regenerate it)`

### Requirement: Theme-aware charts
The system SHALL draw `Plot` chart colors from `--ip-plot-text`, `-muted`, `-grid`, `-zero`, `-axis`, and `-series-1` custom properties, each with a hex fallback, inside inline SVG, so switching theme repaints every chart on the page, saved snapshots included, with no re-run, and chart text reaches at least 4.5:1 contrast in both themes. (PRD-0065, PRD-0008)

#### Scenario: Both themes are readable
- **WHEN** `/public/charts-with-plot` is shown in the light theme and then the dark theme
- **THEN** the worst chart label contrast against its painted background is at least 4.5:1 in each
- **AND** the resolved label fill differs between the two themes for the same SVG bytes

#### Scenario: A copied chart renders standalone
- **WHEN** a chart's SVG is copied or downloaded
- **THEN** every `var(--ip-plot-…)` in it carries a fallback color, so it renders outside ironpad

#### Scenario: Charts fit and respond
- **WHEN** a chart wider than its output area is shown
- **THEN** it scales down to the available width, and hovering a point or bar highlights it under a crosshair cursor
