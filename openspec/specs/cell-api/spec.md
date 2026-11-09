# cell-api Specification

## Purpose
The Rust surface a notebook cell author writes against: the `ironpad_cell` prelude every code cell gets, how a cell's final value becomes displayed panels and a piped value, the typed `cellN` and `last` bindings that carry data between cells, and the display types, widgets, live traits, simulation bus, timing, network, blocking, GPU, and parallel, SIMD, and autodiff facilities available inside a cell.

## Requirements
### Requirement: Prelude in scope
The system SHALL compile every browser-run Rust code cell with `ironpad_cell::prelude::*` in scope, so a cell can name `CellOutput`, the display types, `Canvas`, `Animation`, `Plot`, `ui`, `sim`, `blocking`, `http`, `Stopwatch`, `GpuCanvas`, `Simulation`, `LiveView`, and `bincode` with no `use` line and no `ironpad-cell` dependency of its own. (PRD-0001)

#### Scenario: A cell with no imports
- **WHEN** a new notebook's only cell contains `CellOutput::text(format!("{}", 42))` and is run
- **THEN** the cell succeeds with no `use` line and no Cargo.toml dependencies
- **AND** its output shows a text panel reading `42`

#### Scenario: An explicit prelude import is harmless
- **WHEN** a cell begins with `use ironpad_cell::prelude::*;`
- **THEN** it compiles and runs exactly as it would without that line

### Requirement: Final expression becomes the output
The system SHALL treat a code cell's source as the body of a block whose final expression is converted with `Into<CellOutput>`, so any value with a `From<T> for CellOutput` conversion may end a cell, and a cell that ends in a statement yields an empty output. (PRD-0001)

#### Scenario: A bare literal
- **WHEN** a new cell with its default source `42` is run
- **THEN** the cell reaches success and its output shows a text panel reading `42`

#### Scenario: A trailing statement
- **WHEN** a cell's source is `let x = 5;`
- **THEN** the cell succeeds with no display panels and 0 output bytes

#### Scenario: A value with no conversion
- **WHEN** a cell ends with a value whose type has no `From` conversion into `CellOutput`
- **THEN** the cell fails to compile with rustc's unsatisfied-trait error

### Requirement: Primitive, string, and unit values
The system SHALL convert a final value of type `i8` through `i128`, `u8` through `u128`, `isize`, `usize`, `f32`, `f64`, or `bool` into a text panel of its `Display` form with a piped value tagged by the type's own name, convert `String` and `&str` into a text panel tagged `String`, and convert `()` into an output with no panels, no bytes, and no tag. (ee994b2, acb0738)

#### Scenario: Numbers and booleans
- **WHEN** cells end with `42i32`, `42.5f64`, and `true`
- **THEN** their text panels read `42`, `42.5`, and `true`
- **AND** downstream cells see them typed `i32`, `f64`, and `bool`

#### Scenario: A string slice pipes as String
- **WHEN** a cell ends with `"hello"`
- **THEN** its text panel reads `hello` and a downstream cell sees it as `String`

### Requirement: Vec values display truncated
The system SHALL convert a final `Vec<T>` (with `T: Serialize + Debug`) into a text panel of its `Debug` elements, showing at most the first 20 elements and at most 80 bytes of each element's `Debug` form, and SHALL pipe the whole vector tagged with its type such as `Vec<u64>` or `Vec<String>`, spelled as a downstream cell can write it (`Vec<&str>` as `Vec<String>`, `HashMap`, `BTreeMap`, `VecDeque`, `Rc`, `Arc`, and `Range` by their public `std` paths, a notebook-shared type through `crate::shared`), and SHALL pipe it untagged when the element type cannot be named downstream, such as a borrow, `Cow`, or a type the cell declares itself. (ee994b2, 79f5dab)

#### Scenario: A short vector
- **WHEN** a cell ends with `vec![10, 20, 30]` of type `Vec<i32>`
- **THEN** its text panel reads `[10, 20, 30]` and a downstream cell sees it as `Vec<i32>`

#### Scenario: Element types a downstream cell can and cannot name
- **WHEN** cell 0 ends with `vec!["apple", "banana"]` and cell 1 reads `cell0.len()`
- **THEN** cell 1 sees `cell0` as a `Vec<String>` and compiles
- **AND** a cell 0 ending with a `Vec` of a struct it declares carries no type tag, so a cell mentioning `last` still compiles and a cell naming `cell0` gets an error on its own line

#### Scenario: A long vector
- **WHEN** a cell ends with `(1..=100).collect::<Vec<u64>>()`
- **THEN** its text panel starts with `Vec<u64>, len = 100, [1, 2, 3,` and ends with `...]`
- **AND** element 21 onward is not shown, while all 100 elements are piped downstream

#### Scenario: A long element
- **WHEN** an element's `Debug` form exceeds 80 characters, including multibyte text
- **THEN** it is cut at a character boundary at or below 80 bytes and suffixed with `…`, without panicking

### Requirement: CellOutput builders
The system SHALL provide `CellOutput::text`, `html`, `svg`, `markdown`, and `empty` as display-only outputs with nothing piped, `CellOutput::new(&value)` returning a `Result` that pipes the bincode encoding of `value` with no type tag, and the chaining appenders `with_text`, `with_display`, `with_html`, `with_svg`, and `with_markdown`, which add panels in call order. (PRD-0001, PRD-0003)

#### Scenario: Chained panels
- **WHEN** a cell ends with `CellOutput::empty().with_text("a").with_svg("<svg/>")`
- **THEN** its output shows a text panel `a` followed by an SVG panel

#### Scenario: Bytes with a display line
- **WHEN** a cell contains `let data: Vec<i32> = vec![1, 2, 3, 4, 5];` and ends with `CellOutput::new(&data).unwrap().with_display(format!("Sent: {:?}", data))`
- **THEN** its output shows `Sent: [1, 2, 3, 4, 5]` and the bincode encoding of `data` is piped
- **AND** no `cellN` binding is generated for it downstream, because the output carries no type tag

### Requirement: Rich display types
The system SHALL provide `Html(String)`, `Svg(String)`, `Md(String)`, `Table::new(headers, rows)`, and `Json(serde_json::Value)` as final values that display as HTML, SVG, Markdown, table, and syntax-highlighted JSON panels respectively and pipe themselves tagged `Html`, `Svg`, `Md`, `Table`, and `Json`. (PRD-0003, PRD-0004)

#### Scenario: A table
- **WHEN** a cell ends with `Table::new(vec!["n", "fib(n)"], rows)` where `rows` is a `Vec<Vec<String>>`
- **THEN** its output shows a table with those two headers and one row per inner vector

#### Scenario: Highlighted JSON
- **WHEN** a cell that declares `serde_json` ends with `Json(serde_json::json!({"name": "ironpad", "ok": true}))`
- **THEN** its output shows a pretty-printed `<pre>` block colored through the page's syntax tokens, each with a fallback: keys `var(--ip-tok-keyword, #D34E68)`, strings `var(--ip-tok-string, #539D6C)`, numbers `var(--ip-tok-number, #996ACC)`, `true`, `false`, `null` `var(--ip-tok-type, #3E8FC8)`, and punctuation `var(--ip-tok-punct, #787894)`, so every token reaches 4.5:1 contrast in both themes, saved snapshots included
- **AND** string content is HTML-escaped before it is placed in the block

#### Scenario: JSON parsed from text
- **WHEN** a cell evaluates `"{\"a\": 1}".parse::<Json>()`
- **THEN** it gets `Ok(Json)` without declaring `serde_json`, and invalid JSON gets `Err`

### Requirement: Tuples combine outputs
The system SHALL convert a final tuple of 2 to 10 elements, each `Serialize + IntoPanels + TypeTag`, into an output whose panels are each element's panels in tuple order and whose piped value is the whole tuple tagged as a tuple type such as `(Vec<f64>, Svg)`, or untagged when any element's type cannot be named downstream. (ee994b2)

#### Scenario: Data with a chart
- **WHEN** a cell ends with `(data, Plot::line(&data))` where `data: Vec<(f64, f64)>`
- **THEN** its output shows the vector's text panel followed by the chart's SVG panel
- **AND** a downstream cell sees `cellN` typed `(Vec<(f64, f64)>, Plot)`

#### Scenario: A canvas inside a tuple
- **WHEN** a cell ends with `(table, canvas)`
- **THEN** the canvas renders as the same image panel a bare `canvas` output produces

### Requirement: Typed cellN bindings
The system SHALL bind, in each code cell, `cellN` for every upstream cell N (zero-based position in the whole notebook, Markdown and shared cells included) whose last run produced a type tag and that the cell's source references by the word `cellN`, deserialized to that tagged type before the cell's code runs. (PRD-0001, PRD-0060)

#### Scenario: Reading an upstream vector
- **WHEN** cell 0 ends with `vec![1, 2, 3, 4, 5]` and cell 1 contains `let s: i32 = cell0.iter().sum(); s`
- **THEN** cell 1's output reads `15`

#### Scenario: Positions count every cell
- **WHEN** a notebook is Markdown, code returning `u32`, Markdown, code returning `String`
- **THEN** a fifth cell can read `cell1` as a `u32` and `cell3` as a `String`
- **AND** naming `cell0` or `cell2` in it fails to compile with rustc's cannot-find-value error

#### Scenario: Unreferenced slots are not bound
- **WHEN** a cell's source names `cell2` but not `cell0`
- **THEN** only `cell2` is bound, so an unrun or failed `cell0` cannot break it

#### Scenario: Display-only outputs are not piped
- **WHEN** an upstream cell output carries no type tag, such as `CellOutput::text(..)`, a bare `Plot`, an `Animation`, a `Simulation`, or a `LiveView`
- **THEN** no `cellN` binding exists for it downstream

#### Scenario: A slot that fails to decode
- **WHEN** a bound slot's bytes do not decode as its tagged type
- **THEN** the cell panics with a message starting `failed to deserialize cellN as` the type, naming the input byte count and the decode error, and the run fails

### Requirement: The last alias
The system SHALL bind `last` as a reference to the binding of the nearest upstream cell that produced a type tag whenever the cell's source contains `last` as a standalone word not preceded by a single `.`, and SHALL bind every typed upstream slot in that case. (PRD-0060)

#### Scenario: Last refers to the nearest typed cell
- **WHEN** upstream cells 0 and 2 produced `i32` and `String` and cell 3 ends with `last.clone()`
- **THEN** `last` is `&cell2` and the output reads cell 2's string

#### Scenario: Method calls named last do not count
- **WHEN** a cell's only use of the word is `items.last()`
- **THEN** no `last` alias is bound and no upstream slot is bound because of it

### Requirement: Raw upstream input access
The system SHALL expose the raw upstream outputs to any cell whose source names `__ironpad_inputs__` and that has at least one upstream cell, with `.get(i)` returning slot i as a `CellInput` (empty when out of range), `.last()`, `.len()`, and `CellInput::deserialize::<T>()`, `is_empty()`, and `raw()`. (PRD-0001)

#### Scenario: Reading an untagged upstream value
- **WHEN** cell 0 ends with `CellOutput::new(&data).unwrap()` for `data = vec![1, 2, 3, 4, 5]` and cell 1 contains `let data: Vec<i32> = __ironpad_inputs__.get(0).deserialize().unwrap();`
- **THEN** cell 1 receives `[1, 2, 3, 4, 5]` and can output `Sum: 15`

### Requirement: Canvas pixel images
The system SHALL provide `Canvas` as an RGB pixel buffer (`new` filled black, `from_fn`, `from_pixels`, `set_pixel`, `get_pixel`, `fill`, `width`, `height`, `pixels`, `into_pixels`, `to_bmp`, `to_html`) that, as an output, displays as a bitmap image panel at the canvas's pixel size and pipes itself tagged `Canvas`. (e5aaa2c)

#### Scenario: A computed image
- **WHEN** a cell ends with `Canvas::from_fn(800, 600, |x, y| ((x % 256) as u8, (y % 256) as u8, 0))`
- **THEN** its output shows an 800×600 image rendered with pixelated scaling

#### Scenario: Out-of-range pixels are ignored
- **WHEN** a cell calls `set_pixel` or `get_pixel` outside the canvas
- **THEN** the write does nothing and the read returns `(0, 0, 0)`, with no panic

#### Scenario: A mismatched pixel list
- **WHEN** `Canvas::from_pixels(w, h, pixels)` receives a list whose length is not `w * h`
- **THEN** the cell panics with `Canvas::from_pixels: expected` the pixel count

### Requirement: Precomputed animations
The system SHALL provide `Animation::new(frames: Vec<Canvas>, fps)` which, as an output, displays an auto-playing, looping frame player sized to the first frame and pipes nothing; an animation with no frames produces no panel. (PRD-0017)

#### Scenario: A looping animation
- **WHEN** a cell ends with `Animation::new(frames, 30)` for 200 frames of 600×300
- **THEN** its output plays the frames in order at 30 fps and returns to the first after the last

### Requirement: Simulation cells
The system SHALL run a cell containing a line `impl Simulation for <Name>` as a live simulation: its source is item-level code, `init()` runs once and `tick()` once more for the first frame, then the page calls `tick()` at `fps()` (default 30) to draw each new `Canvas`; such a cell gets no `cellN` bindings and pipes nothing. (PRD-0017, PRD-0018)

#### Scenario: A live simulation
- **WHEN** a cell defines `struct Pendulum` and `impl Simulation for Pendulum` with `init`, `tick`, and `fn fps() -> u32 { 60 }`
- **THEN** its output shows the first frame immediately and keeps advancing at up to 60 frames per second

#### Scenario: A commented impl does not count
- **WHEN** the only `impl Simulation for` line in a cell starts with `//`
- **THEN** the cell is compiled as an ordinary expression cell

#### Scenario: Slider declarations
- **WHEN** `sliders()` returns `vec![sim_slider("rod", 0.0, 1.0).step(0.05).label("Rod depth").default_value(0.5).into_meta()]`
- **THEN** the simulation output shows a `Rod depth` range slider whose value `tick()` reads with `sim::read::<f64>("rod")`
- **AND** an unset `sim_slider` uses step 0.01, its key as label, and its minimum as default

### Requirement: LiveView cells
The system SHALL run a cell containing a line `impl LiveView for <Name>` as a live view: `init()` runs once and `tick()` once for the initial content, then the page calls `tick()` at `fps()` (default 10) and replaces the panel with each returned `LiveContent::Text`, `LiveContent::Html`, or `LiveContent::Markdown`; such a cell pipes nothing. (PRD-0019)

#### Scenario: A live dashboard
- **WHEN** a cell implements `LiveView` whose `tick` returns `LiveContent::Markdown(format!("k_eff = {k:.4}"))` from `sim::read::<f64>("k_eff")`
- **THEN** its output re-renders the Markdown about 10 times per second as the simulation emits new values

#### Scenario: Simulation takes precedence
- **WHEN** one cell contains both an `impl Simulation for` line and an `impl LiveView for` line
- **THEN** the cell runs as a simulation

### Requirement: Simulation bus
The system SHALL provide a page-wide named-value bus: `sim::emit(key, &value)` publishes a JSON-serializable value and keeps the last 1000 per key, `sim::read::<T>(key)` returns the latest as `Some` (or `None` when absent or not decodable as `T`), and `sim::read_all::<T>(key)` returns the retained history oldest first (empty when absent or not decodable). Replacing the execution worker, on a cancel or a crash, SHALL NOT empty the bus: the new worker starts with every retained value. Cells in the worker and cells on the main-thread fallback SHALL share the one bus, each side reading what the other emitted and what was written before the fallback loaded. (PRD-0018)

#### Scenario: Emit then read across cells
- **WHEN** a simulation cell calls `sim::emit("k_eff", &k)` each tick and a later ordinary cell calls `sim::read_all::<f64>("k_eff")`
- **THEN** the later cell receives up to 1000 values, oldest first

#### Scenario: Unknown key
- **WHEN** a cell calls `sim::read::<f64>("never-emitted")`
- **THEN** it receives `None`, and `sim::read_all` for that key receives an empty `Vec`

#### Scenario: Emitted from a worker
- **WHEN** a rayon cell running in the executor worker calls `sim::emit("e2e-rayon-sum", &total)` with `total == 500500`
- **THEN** the page-side bus reads back `500500` for that key

#### Scenario: Emitted on the main-thread fallback
- **WHEN** a cell running on the main-thread fallback calls `sim::emit("k", &1)` and then `sim::read::<i32>("k")`
- **THEN** it reads `Some(1)`, and a later cell in the worker reads it too

#### Scenario: A slider value outlives Stop
- **WHEN** a slider bound to a bus key is moved to 7, the author stops a running cell, and a cell then calls `sim::read` for that key
- **THEN** it reads 7

#### Scenario: An unserializable value
- **WHEN** `sim::emit` receives a value that cannot serialize to JSON, such as a map keyed by tuples
- **THEN** the value is dropped and the cell keeps running

### Requirement: Interactive widgets
The system SHALL provide `ui::slider(name, min, max)`, `ui::number(min, max)`, `ui::dropdown(&options)`, `ui::checkbox(name)`, `ui::switch(label)`, `ui::text_input(placeholder)`, `ui::button(label)`, and `ui::progress_bar()` builders that render as interactive controls and pipe their current value: `f64` for slider and number, `String` for dropdown and text input, `bool` for checkbox and switch, `()` for button. (PRD-0005, PRD-0007, PRD-0008)

#### Scenario: Builder defaults
- **WHEN** widgets are built with no options set
- **THEN** slider and number default to their minimum with step 1, dropdown to its first option (empty string if none), checkbox and switch to `false`, text input to empty, and a checkbox's label is its name

#### Scenario: Builder options
- **WHEN** a cell ends with `ui::slider("speed", 1.0, 10.0).step(0.5).label("Speed").default_value(5.0)`
- **THEN** its output shows a `Speed` range input from 1 to 10 in steps of 0.5 at 5 with its value shown beside it, and downstream `cellN` is `5.0_f64`

#### Scenario: One widget per cell
- **WHEN** cells 1 through 4 output a slider, a dropdown, a number input, and a switch, and cell 6 reads `cell1` through `cell4`
- **THEN** cell 6 sees an `f64`, a `String`, an `f64`, and a `bool` holding the widgets' current values

### Requirement: Widget changes update the piped value
The system SHALL, when a reader changes a live widget that is its cell's whole output, replace that cell's piped value with the new value so downstream cells read it on their next run; a widget inside a tuple output publishes to the simulation bus only and leaves the tuple's piped value as it was. In the editor it SHALL mark every runnable code cell below it stale (re-run automatically when Reactive Mode is on) without re-running the widget's own cell. (PRD-0005, PRD-0007, PRD-0025)

#### Scenario: Slider feeds a downstream cell
- **WHEN** a reader drags a slider in cell 1 to 7.5 and then runs cell 2, which reads `cell1`
- **THEN** cell 2 sees `7.5` while cell 1 keeps its rendered slider and is not re-run

#### Scenario: A slider inside a tuple
- **WHEN** cell 0 ends with `(ui::slider("a", 0.0, 10.0), ui::slider("b", 0.0, 10.0))`, a reader moves the first slider, and then runs cell 1, which reads `let (a, b) = cell0;`
- **THEN** cell 1 runs without a deserialization error and sees the tuple's original values

#### Scenario: Read-only pages
- **WHEN** a widget value changes on a read-only notebook page
- **THEN** the piped value updates but nothing is marked stale or re-run

### Requirement: Button widget runs the cells below it
The system SHALL render `ui::button(label)` as a button (labelled `Run` when the label is empty) whose click runs, in notebook order, every runnable code cell below the button's cell, skipping Markdown and shared cells and never re-running the button's own cell, in both the editor and read-only pages. (PRD-0007)

#### Scenario: Recompute with current widget values
- **WHEN** cell 1 outputs `ui::slider("repeat_count", 1.0, 100.0)`, cell 2 outputs `ui::button("Calculate ▶")`, cell 3 reads `cell1`, and the reader moves the slider to 64 and clicks `Calculate ▶`
- **THEN** cell 3 re-runs and reports results for 64, while cells 1 and 2 are not re-run

#### Scenario: Nothing below
- **WHEN** the button's cell is the last runnable cell
- **THEN** clicking it runs nothing

### Requirement: Widgets publish to the simulation bus
The system SHALL publish widget values to the simulation bus when the page renders the widget and on every change: a slider under its name as `f64`, a dropdown given `.name(key)` as the selected option's index as `f64`, and a checkbox under its name as `1.0` or `0.0`; number, switch, and text input do not publish. (PRD-0018)

#### Scenario: Slider-driven simulation
- **WHEN** one cell outputs `ui::slider("rod_depth", 0.0, 1.0).default_value(0.5)`, a simulation's `tick` reads `sim::read::<f64>("rod_depth").unwrap_or(0.5)`, and Reactive Mode is off
- **THEN** moving the slider changes the next simulation frames with no re-run of either cell

### Requirement: Progress bars
The system SHALL render `ui::progress_bar()` (with `.label` and `.initial(pct)` clamped to 0 to 100) as a bar showing a whole percentage, pipe its id as a `String`, and let any later-running cell call `ProgressHandle::new(id).update(pct)` to move that bar live, with values clamped to 0 to 100 and NaN shown as 0. A bar's id SHALL be unique on the page, including among bars made by different cells. (PRD-0008, PRD-0015)

#### Scenario: Updating from a downstream cell
- **WHEN** cell 1 outputs `ui::progress_bar().label("Download Progress").initial(25.0)` and cell 2 runs `ProgressHandle::new(cell1).update(75.0)`
- **THEN** cell 1's bar first shows `25%` and changes to `75%` while cell 2 runs, including when cell 2 executes in the worker

#### Scenario: Bars from two cells
- **WHEN** cells 0 and 2 each output `ui::progress_bar()`, cell 1 runs `ProgressHandle::new(cell0).update(10.0)` and cell 3 runs `ProgressHandle::new(cell2).update(50.0)`
- **THEN** the two bars carry different ids, cell 0's bar shows `10%` and cell 2's shows `50%`

#### Scenario: An unknown id
- **WHEN** `update` is called with an id no bar on the page carries
- **THEN** nothing changes and the cell keeps running

### Requirement: Plot charts
The system SHALL provide `Plot::line` and `Plot::scatter` over `&[(f64, f64)]` and `Plot::bar` over `&[(&str, f64)]`, with `.title`, `.x_label`, `.y_label`, `.size(w, h)` (default 800×400), `.tooltips(bool)`, and `.point_labels(bool)` (both off by default), rendering an SVG panel; a bare `Plot` pipes nothing, and inside a tuple it pipes tagged `Plot`. (PRD-0004, PRD-0008, PRD-0065)

#### Scenario: Labelled chart
- **WHEN** a cell ends with `Plot::scatter(&data).title("Squares").x_label("x").y_label("x squared")`
- **THEN** its output is an SVG panel containing those three strings, drawn on a transparent background

#### Scenario: Tooltips and point labels
- **WHEN** `Plot::scatter(&[(1.0, 2.0), (3.0, 4.0)])` is built with `.tooltips(true).point_labels(true)`
- **THEN** hovering the points shows `(1, 2)` and `(3, 4)`, and the values `2.0` and `4.0` are drawn beside them

#### Scenario: Bar chart style
- **WHEN** a cell ends with `Plot::bar(&[("A", 3.7), ("B", 9.1), ("C", 5.3), ("D", 1.2)])`
- **THEN** every bar is labelled with its value at one decimal and the bars' opacities by rank are `0.62`, `1`, `0.78`, `0.46` in input order
- **AND** every category name sits on the x axis under the centre of its own bar, however many bars there are

#### Scenario: Negative bars
- **WHEN** a cell ends with `Plot::bar(&[("Q1", 12.0), ("Q2", -7.5)])`
- **THEN** the y axis reaches below zero, `Q2`'s bar extends down from a zero baseline with its value label below its end, and an all-negative chart hangs every bar from a top tick of 0

#### Scenario: Non-finite data
- **WHEN** a chart's data holds `NaN`, an infinity, or values whose span overflows `f64`, such as `Plot::line(&[(0.0, f64::INFINITY), (1.0, 1.0)])`
- **THEN** the chart still renders: non-finite points and bars are skipped rather than drawn at the axis limit, and coordinates are clamped to ±1e300

#### Scenario: Axes and gridlines
- **WHEN** any chart renders
- **THEN** it draws horizontal dashed gridlines only, at most 6 ticks per axis, and a zero baseline only when 0 lies inside the y range

### Requirement: Async HTTP
The system SHALL compile a cell whose source contains `.await` anywhere as an async cell and provide `http::get(url)` returning `Result<String, String>`, `http::get_json::<T>(url)`, and `http::post_json::<B, R>(url, &body)`, each performing a browser fetch and reporting transport or decode failures as `Err` text; a non-2xx response is not itself an error. (ee994b2)

#### Scenario: Awaiting a request
- **WHEN** a cell contains `let body = http::get("https://httpbin.org/get").await;`
- **THEN** the cell runs asynchronously and `body` is `Ok` with the response text, or `Err` with the browser's error message

### Requirement: Blocking host calls
The system SHALL provide `blocking::sleep_ms(ms)`, `blocking::fetch_bytes(url)`, `fetch_text(url)`, and `fetch_json::<T>(url)` as plain synchronous calls that suspend the cell until the browser settles them, for cells with no `.await` on browsers with WebAssembly JSPI; a non-2xx fetch returns `Err("HTTP <status> <text>")`. (PRD-0043)

#### Scenario: Sleep and fetch from synchronous code
- **WHEN** a cell runs `blocking::sleep_ms(300.0)` then `blocking::fetch_text("/notebooks/welcome.ironpad")` with no `.await`
- **THEN** a `Stopwatch` measures at least 250 ms across the sleep and the fetched body is longer than 100 bytes

#### Scenario: A browser without JSPI
- **WHEN** a cell that calls any `blocking::` function runs in a browser lacking JSPI
- **THEN** the run fails before the cell executes with `This cell uses blocking host calls (ironpad_cell::blocking), which need WebAssembly JSPI: available in Chrome and Edge 137+, not yet in Firefox or Safari.`

#### Scenario: Decode failures
- **WHEN** a fetched body is not UTF-8, or not valid JSON for `fetch_json`
- **THEN** the call returns `Err` starting `response body is not valid UTF-8:` or `failed to deserialize JSON:` respectively

### Requirement: Stopwatch timing
The system SHALL provide `Stopwatch::new()` with `elapsed_ms()` and `elapsed_secs()` measuring wall-clock time inside a cell, where `std::time::Instant` is unavailable. (292c7d4)

#### Scenario: Timing a computation
- **WHEN** a cell creates a `Stopwatch`, runs a loop, and reads `elapsed_ms()`
- **THEN** it receives the elapsed milliseconds as an `f64` without panicking

### Requirement: GPU compute canvases
The system SHALL provide `GpuCanvas::new(w, h, wgsl, &uniforms)`, `.with_fallback(|x, y, uniforms| (r, g, b))`, and `.render()`, and SHALL display a GpuCanvas output as an image produced by the WGSL shader on WebGPU, else by the fallback per pixel, else all black; `gpu_available()` reports WebGPU availability. (PRD-0026)

#### Scenario: Shader contract
- **WHEN** a shader declares `uniforms: array<f32>` at binding 0, `output: array<vec4<f32>>` at binding 1, and `@compute @workgroup_size(16, 16) fn main`
- **THEN** it is dispatched over ceil(w/16) × ceil(h/16) workgroups with `uniforms[0]` = width and `uniforms[1]` = height ahead of the cell's own uniforms
- **AND** each output pixel's RGB channels are scaled from 0.0..1.0 to 0..255, clamped, and alpha is dropped

#### Scenario: CPU fallback
- **WHEN** WebGPU is unavailable, or creating buffers or dispatching the shader fails, and a fallback closure was given
- **THEN** the image is computed by calling the closure for every pixel with the full uniforms slice

#### Scenario: No GPU and no fallback
- **WHEN** WebGPU is unavailable and no fallback was given
- **THEN** the output is an all-black image of the requested size

#### Scenario: Two same-size GPU images
- **WHEN** one output contains two GpuCanvas images of the same dimensions
- **THEN** each panel shows its own shader's result, in order

### Requirement: GPU simulations tick on the CPU
The system SHALL provide the `GpuSimulation` trait (`init`, `dimensions`, `shader`, `uniforms`, `tick`, `tick_cpu`, `fps` default 30, `sliders` default empty) whose default `tick` always calls `tick_cpu` (default: a black canvas of `dimensions()`), even when WebGPU is available; it runs live only through an `impl Simulation` that delegates to it. (PRD-0026)

#### Scenario: Adapting to Simulation
- **WHEN** a cell implements `GpuSimulation for Particles` and `Simulation for Particles` with `fn tick(&mut self) -> Canvas { GpuSimulation::tick(self) }`
- **THEN** the simulation animates frames produced by `tick_cpu`

### Requirement: Rayon parallelism
The system SHALL let a cell use rayon by declaring `rayon` in the cell's or notebook's `[dependencies]`: the prelude then brings the rayon parallel-iterator traits into scope, and the page starts a thread pool sized to the browser's reported hardware concurrency (4 when unknown) before the cell runs. (PRD-0024)

#### Scenario: A parallel sum
- **WHEN** a cell with `rayon = "1"` in its Cargo.toml ends with `let total: u64 = (1..=1000u64).into_par_iter().sum(); CellOutput::text(format!("sum={total}"))`
- **THEN** its output reads `sum=500500` with no `use rayon` line and no thread-pool setup in the cell

### Requirement: std::autodiff in cells
The system SHALL let a cell or the notebook's shared source differentiate functions with `std::autodiff` (`#[autodiff_forward]`, `#[autodiff_reverse]`) with no crate-level feature gate written by the author, including reverse-mode functions whose differentiation needs heap allocation. (PRD-0041)

#### Scenario: Reverse-mode gradient through a loop
- **WHEN** the shared source declares `#[autodiff_reverse(d_range, Active, Active)] pub fn range(angle: f64) -> f64` with a data-dependent loop and a cell ends with `let (_r, grad) = shared::d_range(0.6, 1.0); CellOutput::from(grad)`
- **THEN** the cell compiles to a WebAssembly module and outputs the gradient

### Requirement: SIMD in cells
The system SHALL let a cell use `std::simd` portable SIMD and `std::arch::wasm32` SIMD intrinsics with no crate-level feature gate written by the author. (PRD-0042)

#### Scenario: Portable SIMD dot product
- **WHEN** a cell uses `use std::simd::prelude::*;` and ends with `CellOutput::text(format!("dot={}", (a * b).reduce_sum()))` for `f32x4` vectors `[1, 2, 3, 4]` and `[5, 6, 7, 8]`
- **THEN** its output reads `dot=70`

### Requirement: Host messages
The system SHALL provide `host_message(&str)` and `host_message_json(&value)` to send a JSON message with a `"type"` field from a running cell to the page, which acts on `progress_update` and `sim_emit` messages (and the GPU readback requests `GpuCanvas` sends) and ignores every other type. (PRD-0008)

#### Scenario: A malformed message
- **WHEN** a cell sends a host message that is not valid JSON
- **THEN** the page logs a console warning and the cell keeps running

#### Scenario: An unknown type
- **WHEN** a cell sends `{"type": "custom"}`
- **THEN** nothing visible happens and the cell keeps running
