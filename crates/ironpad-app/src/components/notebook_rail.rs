//! Left cell-outline rail for the view-only notebook surface (PRD-0065
//! T-006/T-007).
//!
//! Three stacked groups — `Cells`, `Runtime`, `Deps` — over a scroll-spy that
//! keeps the selected row on whichever cell owns the top of the scrollport.
//!
//! The component is deliberately **pure**: everything it draws arrives as a
//! prop, and the one piece of live state (per-cell run status and timings)
//! travels through [`RailRunState`], a signal the caller owns and writes. That
//! keeps the rail testable without a notebook, and keeps
//! `view_only_notebook.rs` free to decide when a cell has run.
//!
//! Two behaviours are contractual rather than cosmetic:
//!
//! - **Embed renders nothing.** `/embed/*` is chrome-less by contract
//!   (PRD-0039), so `embed=true` returns an empty view before any observer is
//!   installed — not a collapsed rail, not an empty one.
//! - **The scroll-spy owns its closure.** The `IntersectionObserver` callback
//!   is held in a page-scoped [`StoredValue`] and the observer is disconnected
//!   in `on_cleanup`, because a leaked `Closure` on a navigation is the
//!   documented failure mode here (see `DEVELOPMENT.md`). Reads that can
//!   outlive disposal go through `try_*`, since only reads panic on a disposed
//!   signal.

use std::collections::HashMap;

use ironpad_common::{CellType, IronpadCell};
use leptos::prelude::*;

use crate::components::icon::Chevron;

// ── Anchors ─────────────────────────────────────────────────────────────────

/// DOM `id` of the element a rail row scrolls to.
///
/// The one place the anchor naming lives: the rail resolves rows with
/// `getElementById` (no CSS escaping to get wrong on a cell id), and the cell
/// renderer stamps the same string onto its wrapper. Both call this.
#[must_use]
pub fn cell_anchor_id(cell_id: &str) -> String {
    format!("ip-cell-{cell_id}")
}

// ── Outline model ───────────────────────────────────────────────────────────

/// What kind of row this is, which decides its marker and whether it can carry
/// a timing at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RailCellKind {
    /// Runs, and therefore has a status and a timing.
    Code,
    /// Prose. 51% of the cells across `public/notebooks/` are markdown, so an
    /// outline that skipped them would skip half the document and strand the
    /// selected row while the reader scrolled through it.
    Markdown,
    /// A shared cell (PRD-0044): compiles into every other cell's `shared.rs`
    /// and never executes on its own, so it has no status and no timing.
    Shared,
}

/// Per-cell run state. `Stale` is unreachable on `/public` (public source is
/// read-only and `capture-outputs-check` gates freshness in `ci`) but the
/// handoff specifies its treatment and the editor will reach it, so the enum
/// carries it rather than growing a variant later.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RailCellStatus {
    /// Never executed in this browser.
    #[default]
    NotRun,
    /// Compiling or executing right now.
    Running,
    /// Ran and produced output.
    Ran,
    /// Source changed since the output was produced.
    Stale,
    /// Dropped because a cell it depends on failed (PRD-0060).
    Blocked,
    /// Compile or runtime error.
    Failed,
}

/// One cell's live run state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RailCellRun {
    pub status: RailCellStatus,
    /// Wall time spent acquiring the compiled blob, in milliseconds.
    pub compile_ms: Option<f64>,
    /// Wall time spent executing, in milliseconds.
    pub run_ms: Option<f64>,
}

/// The rail's live half: a caller-owned map from cell id to [`RailCellRun`].
///
/// A newtype rather than a bare `RwSignal<HashMap<..>>` prop for two reasons.
/// It keeps the map type an implementation detail, so the storage can change
/// without touching call sites; and it puts the "a status change must not
/// erase the timings already recorded" rule in one place instead of at every
/// cell that reports progress.
#[derive(Clone, Copy)]
pub struct RailRunState(RwSignal<HashMap<String, RailCellRun>>);

impl Default for RailRunState {
    fn default() -> Self {
        Self::new()
    }
}

impl RailRunState {
    #[must_use]
    pub fn new() -> Self {
        Self(RwSignal::new(HashMap::new()))
    }

    /// Replace a cell's whole entry.
    pub fn set(self, cell_id: &str, run: RailCellRun) {
        self.0.update(|m| {
            m.insert(cell_id.to_string(), run);
        });
    }

    /// Move a cell to `status`, preserving whatever timings it already had.
    ///
    /// This is the common report ("started", "blocked", "failed"), and doing
    /// it with [`Self::set`] would silently blank the timings the previous run
    /// recorded.
    pub fn set_status(self, cell_id: &str, status: RailCellStatus) {
        self.0.update(|m| {
            m.entry(cell_id.to_string()).or_default().status = status;
        });
    }

    /// Record a completed run: status plus both timings.
    pub fn set_ran(self, cell_id: &str, compile_ms: Option<f64>, run_ms: Option<f64>) {
        self.set(
            cell_id,
            RailCellRun {
                status: RailCellStatus::Ran,
                compile_ms,
                run_ms,
            },
        );
    }

    /// Reactive read of one cell's state. Disposal-safe: a row rendering after
    /// its owner was disposed reads the default rather than panicking.
    #[must_use]
    fn get(self, cell_id: &str) -> RailCellRun {
        self.0
            .try_with(|m| m.get(cell_id).copied().unwrap_or_default())
            .unwrap_or_default()
    }

    /// Reactive read of the whole map, for the runtime group's totals.
    #[must_use]
    fn totals(self) -> (Option<f64>, Option<f64>) {
        self.0
            .try_with(|m| {
                let sum = |pick: fn(&RailCellRun) -> Option<f64>| {
                    let vals: Vec<f64> = m.values().filter_map(pick).collect();
                    (!vals.is_empty()).then(|| vals.iter().sum())
                };
                (sum(|r| r.compile_ms), sum(|r| r.run_ms))
            })
            .unwrap_or((None, None))
    }
}

/// One static outline row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailCell {
    /// Notebook cell id; the anchor is [`cell_anchor_id`] of this.
    pub id: String,
    /// Display name. Real notebooks label both code and markdown cells
    /// meaningfully ("Introduction", "The pitch", "HTTP GET"), so nothing is
    /// derived from the cell body.
    pub label: String,
    pub kind: RailCellKind,
}

/// Project a notebook's cells onto outline rows, in notebook order.
#[must_use]
pub fn rail_cells(cells: &[IronpadCell]) -> Vec<RailCell> {
    cells
        .iter()
        .map(|c| RailCell {
            id: c.id.clone(),
            label: c.label.clone(),
            kind: if c.shared {
                RailCellKind::Shared
            } else {
                match c.cell_type {
                    CellType::Code => RailCellKind::Code,
                    CellType::Markdown => RailCellKind::Markdown,
                }
            },
        })
        .collect()
}

// ── Dependencies ────────────────────────────────────────────────────────────

/// One line of the `Deps` group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailDep {
    pub name: String,
    /// `None` for path/git dependencies, which have no version to show.
    pub version: Option<String>,
}

/// The notebook's dependency surface: shared `Cargo.toml` plus every cell's,
/// deduplicated by crate name with cell-level entries winning.
///
/// The "what counts as a dependency" question is answered by
/// [`ironpad_common::cache_key::merge_dependencies`] — the same function the
/// scaffold uses — so a rail listing can never disagree with what actually
/// compiles. Only the presentational split into name and version lives here.
#[must_use]
pub fn rail_deps(shared_cargo_toml: Option<&str>, cells: &[IronpadCell]) -> Vec<RailDep> {
    let cell_manifests = cells
        .iter()
        .filter_map(|c| c.cargo_toml.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    parse_rendered_deps(&ironpad_common::cache_key::merge_dependencies(
        shared_cargo_toml,
        &cell_manifests,
    ))
}

/// Split `merge_dependencies`' rendered block into display rows.
///
/// The block is inline `name = spec` lines first, then any dotted
/// `[dependencies.NAME]` subtables as multi-line blocks.
fn parse_rendered_deps(rendered: &str) -> Vec<RailDep> {
    let mut deps: Vec<RailDep> = Vec::new();
    let mut in_section = false;

    for line in rendered.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('[') {
            in_section = ironpad_common::cache_key::is_dotted_dependency_section(trimmed);
            if in_section {
                if let Some(name) = trimmed
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .trim()
                    .strip_prefix("dependencies.")
                {
                    deps.push(RailDep {
                        name: name.trim().to_string(),
                        version: None,
                    });
                }
            }
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        if in_section {
            // A subtable's own `version = "1"` line fills in the entry the
            // header opened.
            if key.trim() == "version" {
                if let Some(last) = deps.last_mut() {
                    last.version = first_quoted(value).map(str::to_string);
                }
            }
        } else if !key.trim().is_empty() {
            deps.push(RailDep {
                name: key.trim().to_string(),
                version: dep_version(value).map(str::to_string),
            });
        }
    }
    deps
}

/// Version out of a dependency value: `"1.0"` directly, or the `version` key
/// of an inline table. Path and git dependencies yield `None`.
fn dep_version(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.starts_with('"') {
        return first_quoted(value);
    }
    // Scan the value side only, so a crate NAME containing "version" cannot
    // be mistaken for the key.
    let idx = value.find("version")?;
    let after = value.get(idx + "version".len()..)?;
    first_quoted(after.split_once('=')?.1)
}

/// The contents of the first double-quoted run in `s`.
fn first_quoted(s: &str) -> Option<&str> {
    let rest = s.split_once('"')?.1;
    rest.split_once('"').map(|(inner, _)| inner)
}

// ── Formatting ──────────────────────────────────────────────────────────────

/// Milliseconds as the rail shows them: a decimal below 10ms, whole numbers
/// above, so a column of timings stays the same width as it changes.
fn format_ms(ms: f64) -> String {
    if ms < 10.0 {
        format!("{ms:.1}ms")
    } else {
        format!("{ms:.0}ms")
    }
}

/// `"1 cell"` / `"12 cells"`.
fn cell_count_label(count: usize) -> String {
    if count == 1 {
        "1 cell".to_string()
    } else {
        format!("{count} cells")
    }
}

/// The timing text for a row.
///
/// Not-run cells render **nothing** rather than the handoff's `—`: the dim dot
/// already says "not run", and a bare em-dash is both a glyph doing an icon's
/// job and the punctuation this codebase strips from user-visible strings.
fn row_timing(kind: RailCellKind, run: RailCellRun) -> Option<String> {
    if kind != RailCellKind::Code {
        return None;
    }
    match run.status {
        RailCellStatus::Running => Some("running".to_string()),
        RailCellStatus::Stale => Some("stale".to_string()),
        RailCellStatus::Blocked => Some("skipped".to_string()),
        RailCellStatus::Failed => Some("failed".to_string()),
        RailCellStatus::Ran => run.run_ms.map(format_ms),
        RailCellStatus::NotRun => None,
    }
}

/// Modifier suffix for the status marker, so the colour lives in one place.
fn marker_modifier(kind: RailCellKind, status: RailCellStatus) -> &'static str {
    match kind {
        RailCellKind::Markdown => "prose",
        RailCellKind::Shared => "shared",
        RailCellKind::Code => match status {
            RailCellStatus::NotRun => "not-run",
            RailCellStatus::Running => "running",
            RailCellStatus::Ran => "ran",
            RailCellStatus::Stale => "stale",
            RailCellStatus::Blocked => "blocked",
            RailCellStatus::Failed => "failed",
        },
    }
}

/// Accessible description of the marker, which is a coloured shape carrying
/// real information and therefore needs a text equivalent.
fn row_state_label(kind: RailCellKind, status: RailCellStatus) -> &'static str {
    match kind {
        RailCellKind::Markdown => "Markdown cell",
        RailCellKind::Shared => "Shared cell, never runs",
        RailCellKind::Code => match status {
            RailCellStatus::NotRun => "Not run",
            RailCellStatus::Running => "Running",
            RailCellStatus::Ran => "Ran",
            RailCellStatus::Stale => "Stale",
            RailCellStatus::Blocked => "Skipped, a dependency failed",
            RailCellStatus::Failed => "Failed",
        },
    }
}

// ── NotebookRail ────────────────────────────────────────────────────────────

/// Cell outline with status, timings, runtime and dependency groups.
///
/// Renders nothing at all in embed mode. Under ~1000px the groups collapse
/// behind a disclosure button and float over the content as a dropdown.
#[allow(clippy::needless_pass_by_value)] // Component props are owned.
#[component]
pub fn NotebookRail(
    /// Outline rows, in notebook order.
    cells: Vec<RailCell>,
    /// Live per-cell status and timings, written by the cell renderers.
    run: RailRunState,
    /// Toolchain cells compile on (`crate::CELL_TOOLCHAIN`). A prop rather
    /// than a direct read so the rail stays renderable from a test.
    #[prop(into)]
    toolchain: String,
    /// Compilation target.
    #[prop(into, default = String::from("wasm32-unknown-unknown"))]
    target: String,
    /// Notebook dependencies, one row each. See [`rail_deps`].
    #[prop(default = Vec::new())]
    deps: Vec<RailDep>,
    /// Chrome-less embed (PRD-0039). Renders nothing — no collapsed rail, no
    /// empty rail, and no observer.
    #[prop(optional)]
    embed: bool,
    /// CSS selector for the element the cells scroll inside. The observer uses
    /// it as its root so the "top of the viewport" the spy tracks is the top
    /// of the *cell list*, independent of how tall the header and toolbar
    /// above it happen to be. Falls back to the viewport when it matches
    /// nothing.
    #[prop(default = ".view-only-cells")]
    scroll_root: &'static str,
) -> impl IntoView {
    if embed || cells.is_empty() {
        return ().into_any();
    }

    let cell_count = cells.len();
    // SSR renders a selected first row, so the rail never flashes unselected
    // before the observer's first callback.
    let selected: RwSignal<Option<String>> = RwSignal::new(cells.first().map(|c| c.id.clone()));
    // Monotonic deadline: while `now()` is below it the spy ignores its own
    // callbacks, so a click's smooth scroll does not drag the selection
    // through every cell it passes. A timestamp rather than a timer keeps the
    // cleanup surface at one observer.
    let suppress_until = StoredValue::new(0.0_f64);
    let mobile_open = RwSignal::new(false);

    #[cfg(feature = "hydrate")]
    {
        let ids: Vec<String> = cells.iter().map(|c| c.id.clone()).collect();
        install_scroll_spy(ids, selected, suppress_until, scroll_root);
        crate::components::dismiss::dismiss_on_outside_click(".ip-rail", move || {
            crate::components::dismiss::clear_if_open(mobile_open);
        });
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = scroll_root;

    let rows = cells
        .into_iter()
        .map(|cell| {
            let kind = cell.kind;
            let label = cell.label;
            // One StoredValue rather than four clones: the id is read from
            // four independent reactive closures on this row.
            let id = StoredValue::new(cell.id);
            let is_selected = Signal::derive(move || {
                selected.try_get().flatten().as_deref() == Some(id.get_value().as_str())
            });
            let state = Signal::derive(move || run.get(&id.get_value()));
            view! {
                <li class="ip-rail-row-item">
                    <button
                        class=move || {
                            if is_selected.get() {
                                "ip-rail-row ip-rail-row--selected"
                            } else {
                                "ip-rail-row"
                            }
                        }
                        aria-current=move || is_selected.get().then_some("true")
                        on:click=move |_| {
                            let cell_id = id.get_value();
                            selected.set(Some(cell_id.clone()));
                            mobile_open.set(false);
                            #[cfg(feature = "hydrate")]
                            scroll_to_cell(&cell_id, suppress_until);
                        }
                    >
                        <span
                            class=move || format!(
                                "ip-rail-dot ip-rail-dot--{}",
                                marker_modifier(kind, state.get().status),
                            )
                            aria-hidden="true"
                        ></span>
                        <span class="ip-rail-row-name">{label}</span>
                        // Same modifier as the marker, so "stale" reads amber
                        // in both places from one mapping.
                        <span class=move || format!(
                            "ip-rail-row-timing ip-rail-row-timing--{}",
                            marker_modifier(kind, state.get().status),
                        )>
                            {move || row_timing(kind, state.get())}
                        </span>
                        <span class="ironpad-visually-hidden">
                            {move || row_state_label(kind, state.get().status)}
                        </span>
                    </button>
                </li>
            }
        })
        .collect_view();

    view! {
        <nav class="ip-rail" aria-label="Notebook outline">
            // Only rendered under ~1000px, where the rail is a dropdown.
            <button
                class="ip-rail-toggle"
                aria-expanded=move || if mobile_open.get() { "true" } else { "false" }
                on:click=move |_| mobile_open.update(|o| *o = !*o)
            >
                <Chevron expanded=Signal::derive(move || mobile_open.get())/>
                <span>"Outline"</span>
                <span class="ip-rail-toggle-count">{cell_count_label(cell_count)}</span>
            </button>

            <div class=move || {
                if mobile_open.get() { "ip-rail-body ip-rail-body--open" } else { "ip-rail-body" }
            }>
                <div class="ip-rail-group">
                    // Plain divs, not headings: the page already has an h1 and
                    // the markdown cells bring their own heading tree, so three
                    // more would interleave into it. The landmark and the
                    // list's own label carry the structure instead.
                    <div class="ip-rail-group-header" id="ip-rail-cells-header">"Cells"</div>
                    <ul class="ip-rail-rows" aria-labelledby="ip-rail-cells-header">{rows}</ul>
                </div>

                <div class="ip-rail-group">
                    <div class="ip-rail-group-header">"Runtime"</div>
                    <p class="ip-rail-line">{toolchain}</p>
                    <p class="ip-rail-line">{target}</p>

                    // Hidden until something has actually run: a "0ms" total
                    // would state a measurement nobody took.
                    {move || {
                        let (compile, total) = run.totals();
                        (compile.is_some() || total.is_some()).then(|| view! {
                            <div class="ip-rail-stats">
                                {compile.map(|ms| view! {
                                    <div class="ip-rail-stat">
                                        <span class="ip-rail-stat-label">"compile"</span>
                                        <span class="ip-rail-stat-value">{format_ms(ms)}</span>
                                    </div>
                                })}
                                {total.map(|ms| view! {
                                    <div class="ip-rail-stat">
                                        <span class="ip-rail-stat-label">"total run"</span>
                                        <span class="ip-rail-stat-value">{format_ms(ms)}</span>
                                    </div>
                                })}
                            </div>
                        })
                    }}
                </div>

                {(!deps.is_empty()).then(|| view! {
                    <div class="ip-rail-group">
                        <div class="ip-rail-group-header">"Deps"</div>
                        {deps.into_iter().map(|dep| view! {
                            <p class="ip-rail-line ip-rail-line--dep">
                                <span class="ip-rail-dep-name">{dep.name}</span>
                                {dep.version.map(|v| view! {
                                    <span class="ip-rail-dep-version">{v}</span>
                                })}
                            </p>
                        }).collect_view()}
                    </div>
                })}
            </div>
        </nav>
    }
    .into_any()
}

// ── Scroll-spy (hydrate only) ───────────────────────────────────────────────

/// How long after a click the spy ignores its own callbacks, in milliseconds.
/// Long enough to cover a smooth scroll across a long notebook; the final
/// callbacks after it lands correct any drift.
#[cfg(feature = "hydrate")]
const CLICK_SUPPRESS_MS: f64 = 700.0;

/// Scroll a cell to the top of its scrollport and hold the spy off while it
/// travels.
#[cfg(feature = "hydrate")]
fn scroll_to_cell(cell_id: &str, suppress_until: StoredValue<f64>) {
    let Some(element) = leptos::web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(&cell_anchor_id(cell_id)))
    else {
        return;
    };

    let options = leptos::web_sys::ScrollIntoViewOptions::new();
    options.set_block(leptos::web_sys::ScrollLogicalPosition::Start);
    // `scrollIntoView`'s option overrides CSS `scroll-behavior`, so the
    // reduced-motion preference has to be read here rather than left to the
    // stylesheet.
    options.set_behavior(if prefers_reduced_motion() {
        leptos::web_sys::ScrollBehavior::Auto
    } else {
        suppress_until.set_value(js_sys::Date::now() + CLICK_SUPPRESS_MS);
        leptos::web_sys::ScrollBehavior::Smooth
    });
    element.scroll_into_view_with_scroll_into_view_options(&options);
}

#[cfg(feature = "hydrate")]
fn prefers_reduced_motion() -> bool {
    leptos::web_sys::window()
        .and_then(|w| {
            w.match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .is_some_and(|m| m.matches())
}

/// Observe every cell anchor and keep `selected` on the cell owning the top of
/// the scrollport.
///
/// The rule is exact rather than tuned: membership is "visible anywhere in the
/// root", and the winner is the last cell whose top edge has passed the root's
/// top (falling back to the topmost visible cell when the list is scrolled to
/// its start). Every transition of "which cell owns the top" is a cell leaving
/// or entering the root, so the observer fires exactly when the answer can
/// change — no `rootMargin` band to calibrate, and no scroll listener.
///
/// Tops are re-measured live inside the callback instead of being cached from
/// the entries: an entry only reports a rect at the moment it crossed a
/// threshold, and a cell that stays visible across several callbacks would
/// keep comparing with a rect from wherever the page used to be.
#[cfg(feature = "hydrate")]
fn install_scroll_spy(
    cell_ids: Vec<String>,
    selected: RwSignal<Option<String>>,
    suppress_until: StoredValue<f64>,
    scroll_root: &'static str,
) {
    use std::cell::RefCell;
    use std::collections::HashSet;

    use leptos::web_sys;
    // `JsCast` (for `dyn_into` / `unchecked_ref`) comes in with the prelude.
    use wasm_bindgen::prelude::*;

    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };

    // Resolve by element id: a cell id needs no CSS escaping this way, and a
    // cell whose anchor is missing is simply not observed.
    let expected = cell_ids.len();
    let targets: Vec<(String, web_sys::Element)> = cell_ids
        .into_iter()
        .filter_map(|id| {
            document
                .get_element_by_id(&cell_anchor_id(&id))
                .map(|el| (id, el))
        })
        .collect();
    // Both halves of the integration contract fail SILENTLY if they are wrong:
    // no anchors means no spy AND no click-to-scroll, and it looks exactly like
    // a rail nobody has scrolled yet. Say so, loudly, at the moment it happens.
    if targets.len() < expected {
        leptos::logging::warn!(
            "notebook rail: {} of {expected} cells have no `{}` anchor element; \
             the cell renderer must stamp `cell_anchor_id(&cell.id)` onto each \
             cell wrapper or scroll-spy and click-to-scroll cannot work",
            expected - targets.len(),
            cell_anchor_id("{cell_id}"),
        );
    }
    if targets.is_empty() {
        return;
    }

    let root = document.query_selector(scroll_root).ok().flatten();
    if root.is_none() {
        // Not fatal: the observer falls back to the viewport and the spy still
        // tracks, just measured against the window top rather than the top of
        // the cell list, so the selection runs a header's height early.
        leptos::logging::warn!(
            "notebook rail: scroll root `{scroll_root}` matched nothing; \
             falling back to the viewport, which offsets the scroll-spy by the \
             height of everything above the cell list"
        );
    }
    let anchor_to_cell: HashMap<String, String> = targets
        .iter()
        .map(|(cell_id, el)| (el.id(), cell_id.clone()))
        .collect();

    // Owned by the callback, so it dies with it. Membership only — the tops
    // are measured fresh each time.
    let visible: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    let elements = targets.clone();
    let root_for_cb = root.clone();

    let closure = Closure::<dyn Fn(js_sys::Array)>::new(move |entries: js_sys::Array| {
        for entry in entries.iter() {
            let Ok(entry) = entry.dyn_into::<web_sys::IntersectionObserverEntry>() else {
                continue;
            };
            let Some(cell_id) = anchor_to_cell.get(&entry.target().id()) else {
                continue;
            };
            if entry.is_intersecting() {
                visible.borrow_mut().insert(cell_id.clone());
            } else {
                visible.borrow_mut().remove(cell_id);
            }
        }

        if suppress_until.try_get_value().unwrap_or(0.0) > js_sys::Date::now() {
            return;
        }

        let root_top = root_for_cb
            .as_ref()
            .map_or(0.0, |r| r.get_bounding_client_rect().top());

        let members = visible.borrow();
        let mut passed: Option<(f64, &str)> = None;
        let mut topmost: Option<(f64, &str)> = None;
        for (cell_id, el) in &elements {
            if !members.contains(cell_id) {
                continue;
            }
            // 1px of slack: a cell scrolled exactly to the top can land a
            // hair below it after sub-pixel layout, which would otherwise
            // read as "not reached yet".
            let top = el.get_bounding_client_rect().top() - root_top - 1.0;
            if top <= 0.0 && passed.is_none_or(|(best, _)| top > best) {
                passed = Some((top, cell_id));
            }
            if topmost.is_none_or(|(best, _)| top < best) {
                topmost = Some((top, cell_id));
            }
        }

        if let Some((_, cell_id)) = passed.or(topmost) {
            // Writes on a disposed signal no-op, but the read would panic.
            if selected.try_get_untracked().flatten().as_deref() != Some(cell_id) {
                selected.set(Some(cell_id.to_string()));
            }
        }
    });

    let init = web_sys::IntersectionObserverInit::new();
    if let Some(root) = root.as_ref() {
        init.set_root(Some(root));
    }
    let Ok(observer) =
        web_sys::IntersectionObserver::new_with_options(closure.as_ref().unchecked_ref(), &init)
    else {
        return;
    };
    for (_, el) in &targets {
        observer.observe(el);
    }

    // Hold the closure for as long as the observer can call it, and tear both
    // down on unmount: a leaked `Closure` here would keep the whole rail's
    // captured state (and its signals) alive after the page is gone.
    let stored_closure = StoredValue::new_local(closure);
    let stored_observer = StoredValue::new_local(observer);
    on_cleanup(move || {
        let _ = stored_observer.try_with_value(web_sys::IntersectionObserver::disconnect);
        // The closure itself is freed when the arena drops with this owner;
        // naming it here is what keeps it owned by the cleanup rather than
        // looking like a value nobody uses.
        let _ = stored_closure;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(id: &str, label: &str, cell_type: CellType, shared: bool) -> IronpadCell {
        IronpadCell {
            id: id.to_string(),
            order: 0,
            label: label.to_string(),
            cell_type,
            source: String::new(),
            cargo_toml: None,
            shared,
            collapsed: false,
            output_collapsed: false,
            version: 0,
            saved_output: None,
        }
    }

    #[test]
    fn anchor_ids_are_namespaced_by_cell_id() {
        assert_eq!(cell_anchor_id("abc-123"), "ip-cell-abc-123");
    }

    #[test]
    fn outline_keeps_notebook_order_and_classifies_every_cell() {
        let cells = vec![
            cell("a", "Introduction", CellType::Markdown, false),
            cell("b", "Line Chart", CellType::Code, false),
            cell("c", "Helpers", CellType::Code, true),
        ];
        let rows = rail_cells(&cells);
        assert_eq!(
            rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
            vec![
                RailCellKind::Markdown,
                RailCellKind::Code,
                RailCellKind::Shared
            ]
        );
        assert_eq!(rows[0].label, "Introduction");
        assert_eq!(rows[2].id, "c");
    }

    #[test]
    fn deps_come_from_the_shared_manifest_with_versions() {
        // `facet`'s real manifest: a `[profile.release]` section follows the
        // dependencies and must not leak into the list.
        let shared = "[dependencies]\nfacet = \"0.46.5\"\nfacet-pretty = \"0.46.5\"\n\n\
                      [profile.release]\nopt-level = 1\nlto = false\n";
        let deps = rail_deps(Some(shared), &[]);
        assert_eq!(
            deps,
            vec![
                RailDep {
                    name: "facet".into(),
                    version: Some("0.46.5".into())
                },
                RailDep {
                    name: "facet-pretty".into(),
                    version: Some("0.46.5".into())
                },
            ]
        );
    }

    #[test]
    fn deps_merge_cell_manifests_and_drop_the_injected_runtime() {
        let mut c = cell("a", "One", CellType::Code, false);
        c.cargo_toml = Some("[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nironpad-cell = { path = \"x\" }\n".into());
        let deps = rail_deps(Some("[dependencies]\nrand = \"0.8\"\n"), &[c]);
        assert_eq!(
            deps,
            vec![
                RailDep {
                    name: "rand".into(),
                    version: Some("0.8".into())
                },
                RailDep {
                    name: "serde".into(),
                    version: Some("1".into())
                },
            ],
            "ironpad-cell is always injected by the scaffold and is not the notebook's dependency"
        );
    }

    #[test]
    fn dotted_dependency_subtables_list_with_their_version() {
        let shared = "[dependencies.tokio]\nversion = \"1.40\"\nfeatures = [\"rt\"]\n";
        assert_eq!(
            rail_deps(Some(shared), &[]),
            vec![RailDep {
                name: "tokio".into(),
                version: Some("1.40".into())
            }]
        );
    }

    #[test]
    fn path_and_git_dependencies_list_without_a_version() {
        let shared = "[dependencies]\nlocal = { path = \"../local\" }\n";
        assert_eq!(
            rail_deps(Some(shared), &[]),
            vec![RailDep {
                name: "local".into(),
                version: None
            }]
        );
    }

    #[test]
    fn a_notebook_with_no_dependencies_lists_nothing() {
        assert!(rail_deps(None, &[]).is_empty());
        assert!(rail_deps(Some("[dependencies]\n"), &[]).is_empty());
    }

    #[test]
    fn timings_keep_one_column_width_across_magnitudes() {
        assert_eq!(format_ms(3.09), "3.1ms");
        assert_eq!(format_ms(0.04), "0.0ms");
        assert_eq!(format_ms(13.44), "13ms");
        assert_eq!(format_ms(214.0), "214ms");
    }

    #[test]
    fn only_code_cells_carry_a_timing() {
        let ran = RailCellRun {
            status: RailCellStatus::Ran,
            compile_ms: Some(200.0),
            run_ms: Some(3.1),
        };
        assert_eq!(row_timing(RailCellKind::Code, ran), Some("3.1ms".into()));
        // Prose and shared cells never execute, so a timing column entry would
        // be a measurement of nothing.
        assert_eq!(row_timing(RailCellKind::Markdown, ran), None);
        assert_eq!(row_timing(RailCellKind::Shared, ran), None);
        // A code cell that has not run says so with its dot, not with a glyph.
        assert_eq!(row_timing(RailCellKind::Code, RailCellRun::default()), None);
    }

    #[test]
    fn non_terminal_states_name_themselves_in_the_timing_column() {
        let with = |status| {
            row_timing(
                RailCellKind::Code,
                RailCellRun {
                    status,
                    ..RailCellRun::default()
                },
            )
        };
        assert_eq!(with(RailCellStatus::Running), Some("running".into()));
        assert_eq!(with(RailCellStatus::Stale), Some("stale".into()));
        assert_eq!(with(RailCellStatus::Blocked), Some("skipped".into()));
        assert_eq!(with(RailCellStatus::Failed), Some("failed".into()));
    }

    #[test]
    fn markers_distinguish_prose_from_a_code_cell_that_has_not_run() {
        assert_eq!(
            marker_modifier(RailCellKind::Markdown, RailCellStatus::NotRun),
            "prose"
        );
        assert_eq!(
            marker_modifier(RailCellKind::Code, RailCellStatus::NotRun),
            "not-run"
        );
        assert_eq!(
            marker_modifier(RailCellKind::Shared, RailCellStatus::Ran),
            "shared",
            "a shared cell's marker never reflects a run state it cannot have"
        );
    }

    #[test]
    fn every_marker_has_a_text_equivalent() {
        for kind in [
            RailCellKind::Code,
            RailCellKind::Markdown,
            RailCellKind::Shared,
        ] {
            for status in [
                RailCellStatus::NotRun,
                RailCellStatus::Running,
                RailCellStatus::Ran,
                RailCellStatus::Stale,
                RailCellStatus::Blocked,
                RailCellStatus::Failed,
            ] {
                assert!(!row_state_label(kind, status).is_empty());
            }
        }
    }
}
