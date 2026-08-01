/// View-only notebook component — renders a notebook in read-only mode.
///
/// Users can view source code, run cells, and fork the notebook, but cannot
/// edit, add, delete, or reorder cells.
use std::collections::HashMap;

use leptos::prelude::*;

use ironpad_common::{CellType, ExecutionResult, IronpadCell, IronpadNotebook};
#[cfg(feature = "hydrate")]
use ironpad_common::{CompileRequest, CompileResponse};

use crate::components::copy_button::CopyButton;
use crate::components::markdown_cell::render_markdown;
use crate::components::monaco_editor::MonacoEditor;
use crate::components::output_render::{
    render_display_panel, CellOutputData, DisplayPanel, WidgetSink, VIEW_ONLY_PANEL_CLASSES,
};
#[cfg(feature = "hydrate")]
use crate::server_fns::compile_cell;

// ── ViewOnlyNotebook ────────────────────────────────────────────────────────

/// Await `ms` milliseconds via a `setTimeout`-backed promise, for spacing
/// compile-transport retries.
#[cfg(feature = "hydrate")]
async fn transport_backoff(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(window) = leptos::web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        } else {
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// Derive the canonical (full-page) route for an embed spec:
/// `shared/{hash}` → `/shared/{hash}`, `public/{name}` → `/public/{name}`.
fn canonical_path(spec: &str) -> Option<String> {
    spec.strip_prefix("shared/")
        .map(|h| format!("/shared/{h}"))
        .or_else(|| {
            // Canonical public URLs are extension-less (PRD-0048); specs from
            // pre-0048 embeds still carry `.ironpad`.
            spec.strip_prefix("public/")
                .map(|f| format!("/public/{}", f.strip_suffix(".ironpad").unwrap_or(f)))
        })
}

/// Read-only notebook view. Displays cells (code + markdown), supports
/// execution, and provides a fork button to clone the notebook.
#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::implicit_hasher)] // RwSignal needs the concrete map type.
#[component]
pub(crate) fn ViewOnlyNotebook(
    notebook: IronpadNotebook,
    /// Label shown on the fork button (e.g., "Fork" for public, "Fork to Private" for shared).
    #[prop(default = "Fork".to_string())]
    fork_label: String,
    /// Render for iframe embedding (PRD-0039): hides Fork (IndexedDB is
    /// partitioned inside cross-origin iframes), adds the "Open in ironpad"
    /// badge, and surfaces the threaded-cell limitation.
    #[prop(optional)]
    embed: bool,
    /// Notebook source spec (`shared/{hash}` or `public/{filename}`), used to
    /// build embed snippets on view pages and the canonical link inside embeds.
    /// Empty means "unknown source": no Embed button, badge links to `/`.
    #[prop(optional)]
    embed_spec: Option<String>,
    /// Run all code cells on load. Set only for TRUSTED first-party sources
    /// (public showcase notebooks, embeds of them when the embedder opts in,
    /// and the editor's own view mode — the user's own content): shared
    /// notebooks are arbitrary user content and must never auto-execute
    /// (PRD-0040 scopes auto-run to the trust boundary).
    #[prop(optional)]
    autorun: bool,
    /// Hide the Fork button without the rest of embed mode. The editor's
    /// view mode uses this: forking your own open notebook from its preview
    /// is a duplicate-with-extra-steps.
    #[prop(optional)]
    hide_fork: bool,
    /// Share the caller's outputs map instead of an internal one, so cell
    /// piping state survives the editor's edit/view mode flips. `None`
    /// (public/shared/embed pages) keeps a private map.
    #[prop(optional)]
    cell_outputs: Option<RwSignal<HashMap<String, CellOutputData>>>,
    /// Blob-snapshot manifest for shared notebooks (PRD-0047): cells with an
    /// entry fetch their compiled WASM from the immutable `/share-blobs/`
    /// route instead of invoking `compile_cell`. `None` (public pages,
    /// pre-snapshot shares) keeps the live compile path. `default` rather
    /// than `optional` so the setter takes the `Option` as-is (optional
    /// Option-props strip to the bare inner type).
    #[prop(default = None)]
    share_manifest: Option<ironpad_common::ShareManifest>,
    /// Page-specific actions, rendered inside a "☰" dropdown at the end of
    /// the toolbar (the mutable reader's rebind entry, PRD-0049). `None`
    /// hides the menu entirely; same `default`-not-`optional` reasoning as
    /// `share_manifest`.
    #[prop(default = None)]
    menu: Option<AnyView>,
    /// Page-specific toolbar controls, rendered inline before the "☰" menu
    /// (the mutable reader's "✎ Edit" link for the authoring device). The
    /// slot itself is static; put reactivity *inside* the `AnyView` when the
    /// control appears from hydrate-time state.
    #[prop(default = None)]
    controls: Option<AnyView>,
) -> impl IntoView {
    // Leptos's optional-prop setter takes the bare String, so callers with no
    // spec pass ""; normalize that back to None here.
    let embed_spec = embed_spec.filter(|s| !s.is_empty());
    // Threaded (rayon) cells need SharedArrayBuffer, which requires a
    // crossOriginIsolated context — impossible inside a cross-origin iframe.
    // Detect the combination up front and explain it instead of letting cells
    // fail cryptically (PRD-0039 T-007). Hydrate-only: SSR can't know the
    // isolation state, and the banner only matters in a live embed.
    #[allow(unused_mut)]
    let mut threaded_cells_blocked = false;
    #[cfg(feature = "hydrate")]
    if embed {
        let mentions_rayon = notebook
            .shared_cargo_toml
            .as_deref()
            .unwrap_or("")
            .contains("rayon")
            || notebook
                .cells
                .iter()
                .any(|c| c.cargo_toml.as_deref().unwrap_or("").contains("rayon"));
        let isolated = leptos::web_sys::window()
            .and_then(|w| {
                js_sys::Reflect::get(&w, &"crossOriginIsolated".into())
                    .ok()
                    .and_then(|v| v.as_bool())
            })
            .unwrap_or(false);
        threaded_cells_blocked = mentions_rayon && !isolated;
    }

    let notebook = StoredValue::new(notebook);

    // Shared state: execution outputs keyed by cell ID (for piping between
    // cells) — the caller's map when provided (editor view mode), else local.
    let cell_outputs: RwSignal<HashMap<String, CellOutputData>> =
        cell_outputs.unwrap_or_else(|| RwSignal::new(HashMap::new()));

    // Run-all sequential execution queue (cell IDs in order).
    let run_all_queue: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let running_all: RwSignal<bool> = RwSignal::new(false);
    let force_recompile: RwSignal<bool> = RwSignal::new(false);

    // Keep running_all in sync with the queue.
    Effect::new(move || {
        let empty = run_all_queue.get().is_empty();
        running_all.set(!empty);
    });

    // Auto-execute all code cells on page load (hydrate-only, fires once) when
    // the route says the content is trusted first-party (`autorun`, PRD-0040)
    // or the notebook itself opted into reactive mode (PRD-0025).
    #[cfg(feature = "hydrate")]
    {
        let is_reactive = notebook.with_value(|nb| nb.reactive_mode.unwrap_or(false));
        if autorun || is_reactive {
            let auto_run_done = RwSignal::new(false);
            Effect::new(move || {
                if auto_run_done.get_untracked() {
                    return;
                }
                auto_run_done.set(true);

                let cell_ids: Vec<String> = notebook.with_value(|nb| {
                    nb.cells
                        .iter()
                        .filter(|c| c.is_runnable())
                        .map(|c| c.id.clone())
                        .collect()
                });
                if !cell_ids.is_empty() {
                    run_all_queue.set(cell_ids);
                }
            });
        }
    }

    // Run All handler — collect all code cell IDs and enqueue them.
    let run_all_action = move |_| {
        if running_all.get_untracked() {
            return;
        }
        let cell_ids: Vec<String> = notebook.with_value(|nb| {
            nb.cells
                .iter()
                .filter(|c| c.is_runnable())
                .map(|c| c.id.clone())
                .collect()
        });
        if !cell_ids.is_empty() {
            run_all_queue.set(cell_ids);
        }
    };

    // Fork handler — clones the notebook with a new ID and navigates to it.
    let fork_label_clone = fork_label.clone();
    let navigate = leptos_router::hooks::use_navigate();
    let forking = RwSignal::new(false);
    // Resolved under this component's owner; Copy, app-root-owned, so the
    // async fork continuation can carry it safely.
    #[cfg(feature = "hydrate")]
    let fork_toaster = crate::components::toaster::Toaster::expect_context();
    let fork_action = move |_| {
        // Guard against a double-click starting a second fork (which would
        // create a duplicate notebook and navigate twice).
        if forking.get_untracked() {
            return;
        }
        let _ = &navigate;
        #[cfg(feature = "hydrate")]
        {
            forking.set(true);
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                let mut nb = notebook.get_value();
                nb.id = uuid::Uuid::new_v4();
                nb.title = format!("{} (fork)", nb.title);
                nb.created_at = chrono::Utc::now();
                nb.updated_at = chrono::Utc::now();

                match crate::storage::client::save_notebook(&nb).await {
                    Ok(()) => {
                        // Same copy as Import's toast; the silent navigation
                        // used to read as a no-op.
                        fork_toaster.toast(
                            crate::components::toaster::ToastIntent::Success,
                            "Forked",
                            format!("\"{}\" has been added to your notebooks.", nb.title),
                            4,
                        );
                        navigate(
                            &format!("/local/{}", nb.id),
                            leptos_router::NavigateOptions::default(),
                        );
                    }
                    Err(e) => {
                        forking.set(false); // allow a retry
                        fork_toaster.toast(
                            crate::components::toaster::ToastIntent::Error,
                            "Fork Failed",
                            "Could not save the fork locally. Try again.",
                            5,
                        );
                        leptos::logging::error!(
                            "failed to persist forked notebook to IndexedDB: {e:?}"
                        );
                    }
                }
            });
        }
    };

    // ── Embed snippets popover (view pages only) ────────────────────────
    //
    // Snippets need the deployment origin, which only the browser knows, so
    // they're built lazily on first open (hydrate) rather than at SSR time.
    let embed_popover_open = RwSignal::new(false);
    let embed_snippets: RwSignal<Option<(String, String)>> = RwSignal::new(None);
    let embed_spec_stored = StoredValue::new(embed_spec.clone());
    let toggle_embed_popover = move |_| {
        #[cfg(feature = "hydrate")]
        if !embed_popover_open.get_untracked() && embed_snippets.get_untracked().is_none() {
            if let (Some(spec), Some(win)) =
                (embed_spec_stored.get_value(), leptos::web_sys::window())
            {
                if let Ok(origin) = win.location().origin() {
                    let iframe = format!(
                        r#"<iframe src="{origin}/embed/{spec}" style="width:100%;border:0;" height="600" loading="lazy" title="ironpad notebook"></iframe>"#
                    );
                    let script = format!(
                        r#"<script src="{origin}/embed.js" data-notebook="{spec}" async></script>"#
                    );
                    embed_snippets.set(Some((iframe, script)));
                }
            }
        }
        embed_popover_open.update(|open| *open = !*open);
    };

    // ── Shared source / dependencies appendix ───────────────────────────
    //
    // Cells reference notebook-level shared code as `shared::…`, so readers
    // need a way to see it. Rendered after the cells as collapsed sections:
    // the cells are the story, the shared code is the footnotes.
    let shared_source_appendix = notebook
        .with_value(|nb| nb.shared_source.clone())
        .filter(|s| !s.trim().is_empty());
    let shared_cargo_appendix = notebook
        .with_value(|nb| nb.shared_cargo_toml.clone())
        .filter(|s| !s.trim().is_empty());

    // Canonical full-page link shown as a badge inside embeds. Relative href:
    // inside the iframe it resolves against the ironpad origin, and
    // target="_blank" opens the full app in a new tab.
    let embed_badge_href = embed.then(|| {
        embed_spec
            .as_deref()
            .and_then(canonical_path)
            .unwrap_or_else(|| "/".to_string())
    });

    view! {
        <div class="view-only-notebook">
            <div class="view-only-toolbar">
                <h1 class="view-only-title">{notebook.with_value(|nb| nb.title.clone())}</h1>
                // Controls live in their own row below the title: a long title
                // plus per-item flex wrapping used to strand buttons on ragged
                // rows (and Run All's width changes while running reflowed them
                // live).
                <div class="view-only-toolbar-controls">
                <div class="ironpad-theme-toggle" title="Bypass WASM cache and force fresh compilation">
                    <button
                        class=move || if force_recompile.get() { "ironpad-theme-toggle-segment" } else { "ironpad-theme-toggle-segment ironpad-theme-toggle-segment--active" }
                        on:click=move |_| force_recompile.set(false)
                    >
                        "Cached"
                    </button>
                    <button
                        class=move || if force_recompile.get() { "ironpad-theme-toggle-segment ironpad-theme-toggle-segment--active" } else { "ironpad-theme-toggle-segment" }
                        on:click=move |_| force_recompile.set(true)
                    >
                        "↻ Fresh"
                    </button>
                </div>
                <button
                    class="run-all-button"
                    on:click=run_all_action
                    disabled=move || running_all.get()
                >
                    {move || if running_all.get() { "◐ Running…" } else { "▶▶ Run All" }}
                </button>
                {(!embed && !hide_fork).then(|| view! {
                    <button
                        class="fork-button"
                        on:click=fork_action
                        disabled=move || forking.get()
                    >
                        {move || if forking.get() { "↳ Forking…".to_string() } else { format!("↳ {fork_label_clone}") }}
                    </button>
                })}
                {(!embed && embed_spec.is_some()).then(|| view! {
                    <div class="view-only-embed">
                        <button class="embed-button" on:click=toggle_embed_popover>
                            "</> Embed"
                        </button>
                        {move || embed_popover_open.get().then(|| view! {
                            <div class="view-only-embed-popover">
                                {move || embed_snippets.get().map(|(iframe_snip, script_snip)| view! {
                                    <div class="view-only-embed-snippet">
                                        <span class="view-only-embed-snippet-label">"iframe"</span>
                                        <pre>{iframe_snip.clone()}</pre>
                                        <CopyButton text=iframe_snip />
                                    </div>
                                    <div class="view-only-embed-snippet">
                                        <span class="view-only-embed-snippet-label">"script (auto-resizing)"</span>
                                        <pre>{script_snip.clone()}</pre>
                                        <CopyButton text=script_snip />
                                    </div>
                                }.into_any())}
                            </div>
                        })}
                    </div>
                })}
                {controls}
                {menu.map(|items| {
                    let menu_open = RwSignal::new(false);
                    view! {
                        <div class="ironpad-toolbar-dropdown view-only-menu">
                            <button
                                class="ironpad-toolbar-dropdown-toggle"
                                title="Notebook menu"
                                aria-label="Notebook menu"
                                on:click=move |_| menu_open.update(|v| *v = !*v)
                            >
                                "☰"
                            </button>
                            // Display-toggled rather than conditionally
                            // rendered: the slot's AnyView is consumed once,
                            // so it can't be rebuilt per open the way the
                            // editor's inline menu is. Any click inside
                            // (i.e. choosing an item) closes it.
                            <div
                                class="ironpad-toolbar-dropdown-menu"
                                style:display=move || {
                                    if menu_open.get() { "block" } else { "none" }
                                }
                                on:click=move |_| menu_open.set(false)
                            >
                                {items}
                            </div>
                        </div>
                    }
                })}
                </div>
            </div>
            {threaded_cells_blocked.then(|| view! {
                <div class="view-only-embed-notice">
                    "This notebook uses threaded (rayon) cells, which need a cross-origin-isolated page and can't run inside an embed. Plain and async cells run fine; use the badge below to open the full notebook."
                </div>
            })}
            <div class="view-only-cells">
                {notebook.with_value(|nb| {
                    let cells = nb.cells.clone();
                    let shared_cargo_toml = nb.shared_cargo_toml.clone();
                    // Notebook-level shared source plus every shared cell, in
                    // notebook order (PRD-0044) — what the compiler must see.
                    let shared_source = nb.effective_shared_source();
                    let notebook_id = nb.id.to_string();

                    cells.iter().map(|cell| {
                        let cell = cell.clone();
                        let all_cells = cells.clone();
                        let shared = shared_cargo_toml.clone();
                        let shared_src = shared_source.clone();
                        let nid = notebook_id.clone();
                        // Snapshotted blob for this cell, if the share
                        // manifest has one (PRD-0047).
                        let share_blob = share_manifest
                            .as_ref()
                            .and_then(|m| m.cells.get(&cell.id).cloned());

                        view! {
                            <ViewOnlyCell
                                cell=cell
                                all_cells=all_cells
                                shared_cargo_toml=shared
                                shared_source=shared_src
                                notebook_id=nid
                                cell_outputs=cell_outputs
                                run_all_queue=run_all_queue
                                force_recompile=force_recompile
                                share_blob=share_blob
                            />
                        }
                    }).collect_view()
                })}
                // `.into_any()` erases this block's view type: the extra
                // nesting otherwise pushes the whole-page tachys type past
                // rustc's query depth limit on the SSR build.
                {(shared_source_appendix.is_some() || shared_cargo_appendix.is_some()).then(|| view! {
                    <div class="view-only-shared-appendix">
                        {shared_source_appendix.map(|src| view! {
                            <SharedAppendixSection
                                label="\u{270e} Shared Source (shared.rs)"
                                language="rust"
                                content=src
                            />
                        })}
                        {shared_cargo_appendix.map(|toml| view! {
                            <SharedAppendixSection
                                label="\u{2b21} Shared Dependencies (Cargo.toml)"
                                language="toml"
                                content=toml
                            />
                        })}
                    </div>
                }.into_any())}
            </div>
            {embed_badge_href.map(|href| view! {
                <a class="ironpad-embed-badge" href=href target="_blank" rel="noopener">
                    "⚡ Powered by ironpad · Open ↗"
                </a>
            })}
        </div>
    }
}

// ── Shared appendix ─────────────────────────────────────────────────────────

/// One collapsed-by-default appendix section showing notebook-level shared
/// text (shared source or Cargo.toml) in a read-only Monaco editor. The
/// editor mounts lazily on first expand.
#[component]
fn SharedAppendixSection(
    label: &'static str,
    language: &'static str,
    content: String,
) -> impl IntoView {
    let collapsed = RwSignal::new(true);
    let toggle_icon = move || if collapsed.get() { "▸" } else { "▾" };

    view! {
        <div class="view-only-shared-section">
            <button
                class="view-only-shared-header"
                on:click=move |_| collapsed.update(|c| *c = !*c)
            >
                <span class="ironpad-output-toggle">{toggle_icon}</span>
                <span>{label}</span>
            </button>
            {move || (!collapsed.get()).then(|| view! {
                <div class="view-only-shared-body">
                    <MonacoEditor
                        initial_value=content.clone()
                        language=language
                        read_only=true
                    />
                </div>
            })}
        </div>
    }
}

// ── ViewOnlyCell ────────────────────────────────────────────────────────────

/// Dispatches rendering to the correct sub-component based on cell type.
#[allow(clippy::implicit_hasher)]
#[component]
fn ViewOnlyCell(
    cell: IronpadCell,
    all_cells: Vec<IronpadCell>,
    shared_cargo_toml: Option<String>,
    shared_source: Option<String>,
    notebook_id: String,
    cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
    run_all_queue: RwSignal<Vec<String>>,
    force_recompile: RwSignal<bool>,
    share_blob: Option<ironpad_common::ShareBlobEntry>,
) -> impl IntoView {
    if cell.shared {
        // Shared cells never execute: their source rides in every other
        // cell's shared.rs. Render read-only with the shared chrome.
        return view! {
            <ViewOnlySharedCell cell=cell />
        }
        .into_any();
    }
    match cell.cell_type {
        CellType::Code => view! {
            <ViewOnlyCodeCell
                cell=cell
                all_cells=all_cells
                shared_cargo_toml=shared_cargo_toml
                shared_source=shared_source
                notebook_id=notebook_id
                cell_outputs=cell_outputs
                run_all_queue=run_all_queue
                force_recompile=force_recompile
                share_blob=share_blob
            />
        }
        .into_any(),
        CellType::Markdown => view! {
            <ViewOnlyMarkdownCell source=cell.source.clone() />
        }
        .into_any(),
    }
}

// ── ViewOnlyCodeCell ────────────────────────────────────────────────────────

/// Renders a code cell with a read-only Monaco editor, run button, and output area.
#[allow(clippy::implicit_hasher)]
#[allow(unused_variables)] // Parameters used only in hydrate cfg.
#[component]
fn ViewOnlyCodeCell(
    cell: IronpadCell,
    all_cells: Vec<IronpadCell>,
    shared_cargo_toml: Option<String>,
    shared_source: Option<String>,
    notebook_id: String,
    cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
    run_all_queue: RwSignal<Vec<String>>,
    force_recompile: RwSignal<bool>,
    share_blob: Option<ironpad_common::ShareBlobEntry>,
) -> impl IntoView {
    let cell = StoredValue::new(cell);
    let all_cells = StoredValue::new(all_cells);
    let stored_cargo_toml = StoredValue::new(shared_cargo_toml);
    let stored_source = StoredValue::new(shared_source);
    let stored_notebook_id = StoredValue::new(notebook_id);
    let stored_share_blob = StoredValue::new(share_blob);

    let compiling = RwSignal::new(false);
    let execution_result: RwSignal<Option<ExecutionResult>> = RwSignal::new(None);
    let error_message: RwSignal<Option<String>> = RwSignal::new(None);
    let compile_time_ms: RwSignal<Option<f64>> = RwSignal::new(None);

    // Trigger signal: incrementing this dispatches a compile.
    let run_trigger = RwSignal::new(0u64);

    // Run button click — increment run_trigger to start compile.
    let run_cell = move |_| {
        run_trigger.update(|g| *g += 1);
    };

    // ── Run-all queue watcher ───────────────────────────────────────────
    //
    // When this cell appears at the front of the run-all queue, trigger
    // its compile flow. Non-front cells show a "Queued" status badge.

    let cell_id_for_queue = StoredValue::new(cell.with_value(|c| c.id.clone()));

    let queued = Signal::derive(move || {
        let queue = run_all_queue.get();
        let cid = cell_id_for_queue.get_value();
        queue
            .iter()
            .position(|id| id == &cid)
            .is_some_and(|pos| pos > 0)
    });

    Effect::new(move || {
        let queue = run_all_queue.get();
        let cid = cell_id_for_queue.get_value();

        if queue.first().is_some_and(|id| id == &cid) && !compiling.get_untracked() {
            run_trigger.update(|g| *g += 1);
        }
    });

    // ── Compile + execute flow, driven by `run_trigger` ─────────────────

    let cell_id_for_exec = StoredValue::new(cell.with_value(|c| c.id.clone()));

    Effect::new(move || {
        let gen = run_trigger.get();
        if gen == 0 {
            return;
        }

        #[allow(clippy::needless_return)] // Guard is needed in hydrate cfg.
        if compiling.get_untracked() {
            return;
        }

        #[cfg(feature = "hydrate")]
        {
            compiling.set(true);
            error_message.set(None);

            leptos::task::spawn_local(async move {
                let cell_data = cell.get_value();
                let cell_id = cell_id_for_exec.get_value();
                let cells = all_cells.get_value();
                let my_idx = cells.iter().position(|c| c.id == cell_data.id).unwrap_or(0);

                // Compute previous cell types and serialized input bytes.
                let outputs = cell_outputs.get_untracked();
                let mut all_output_bytes: Vec<&[u8]> = Vec::new();
                let mut types: Vec<String> = Vec::new();

                for c in &cells[..my_idx] {
                    if c.cell_type == CellType::Code {
                        if let Some(data) = outputs.get(&c.id) {
                            all_output_bytes.push(&data.bytes);
                            types.push(data.type_tag.clone().unwrap_or_default());
                        } else {
                            all_output_bytes.push(&[]);
                            types.push(String::new());
                        }
                    } else {
                        all_output_bytes.push(&[]);
                        types.push(String::new());
                    }
                }

                let input_buf = crate::components::executor::encode_cell_inputs(&all_output_bytes);

                let request = CompileRequest {
                    notebook_id: stored_notebook_id.get_value(),
                    cell_id: cell_data.id.clone(),
                    source: cell_data.source.clone(),
                    cargo_toml: cell_data.cargo_toml.clone().unwrap_or_default(),
                    previous_cell_types: types,
                    shared_cargo_toml: stored_cargo_toml.get_value(),
                    shared_source: stored_source.get_value(),
                    force: force_recompile.get_untracked(),
                    shared_check: None,
                };

                let compile_start = js_sys::Date::now();

                // Snapshotted share blob (PRD-0047): in cache mode, a manifest
                // entry replaces the compile round trip with an immutable
                // static GET. Fresh mode skips the snapshot so the server
                // really recompiles; any fetch failure falls back to the live
                // pipeline below.
                let force = force_recompile.get_untracked();
                let mut snapshot_response: Option<CompileResponse> = None;
                if !force {
                    if let Some(entry) = stored_share_blob.get_value() {
                        match crate::components::executor::fetch_share_blob(&entry).await {
                            Ok((wasm_blob, js_glue)) => {
                                snapshot_response = Some(CompileResponse {
                                    wasm_blob,
                                    diagnostics: vec![],
                                    cached: true,
                                    preamble_lines: 0,
                                    js_glue,
                                });
                            }
                            Err(e) => leptos::logging::warn!(
                                "share blob fetch failed ({e}); falling back to compile"
                            ),
                        }
                    }
                }

                // Local blob store (PRD-0047): cache mode probes IndexedDB
                // before paying the wire. The hash is computed in Fresh mode
                // too (so the fresh server result can overwrite the entry),
                // but not when the snapshot already served the blob — that
                // path stores nothing, and skipping keeps a snapshot replay
                // free of even the fingerprint round trip.
                let request_hash = if snapshot_response.is_some() {
                    None
                } else {
                    crate::blob_cache::request_hash(&request).await
                };
                if !force && snapshot_response.is_none() {
                    if let Some(hash) = &request_hash {
                        snapshot_response = crate::blob_cache::try_local_hit(hash).await;
                    }
                }
                let served_without_server = snapshot_response.is_some();

                // Compile requests are idempotent (cache-keyed server-side), so
                // TRANSPORT failures — a dropped connection mid-build, a network
                // change on a mobile client — are safe to retry; the server's
                // incremental target dir makes retried builds much cheaper than
                // the first attempt. Compile diagnostics are Ok responses and
                // never retried.
                let compile_result = if let Some(response) = snapshot_response {
                    Ok(response)
                } else {
                    let mut attempt: i32 = 0;
                    loop {
                        match compile_cell(request.clone()).await {
                            Err(e) if attempt < 2 => {
                                attempt += 1;
                                leptos::logging::warn!(
                                    "compile transport error (attempt {attempt}): {e}; retrying"
                                );
                                transport_backoff(1_500 * attempt).await;
                            }
                            other => break other,
                        }
                    }
                };
                match compile_result {
                    Ok(response) => {
                        compile_time_ms.set(Some(js_sys::Date::now() - compile_start));
                        if response.wasm_blob.is_empty() {
                            let errors: Vec<String> = response
                                .diagnostics
                                .iter()
                                .map(|d| d.message.clone())
                                .collect();
                            error_message.set(Some(errors.join("\n")));
                        } else {
                            // Persist server results locally (PRD-0047). Runs
                            // for Fresh compiles too — that overwrite is what
                            // makes flipping back to cache serve the fresh
                            // blob.
                            if !served_without_server {
                                if let Some(request_hash) = &request_hash {
                                    crate::blob_cache::store_local(request_hash, &response).await;
                                }
                            }
                            let hash =
                                crate::components::executor::hash_wasm_blob(&response.wasm_blob);
                            match crate::components::executor::load_blob(
                                &cell_data.id,
                                &hash,
                                &response.wasm_blob,
                                response.js_glue,
                            )
                            .await
                            {
                                Ok(()) => {
                                    let exec_start = js_sys::Date::now();
                                    match crate::components::executor::execute_cell(
                                        &cell_data.id,
                                        &input_buf,
                                    )
                                    .await
                                    {
                                        Ok((
                                            output_bytes,
                                            display_text,
                                            type_tag,
                                            ran_on_main_thread,
                                        )) => {
                                            cell_outputs.update(|map| {
                                                map.insert(
                                                    cell_data.id.clone(),
                                                    CellOutputData {
                                                        bytes: output_bytes.clone(),
                                                        type_tag: type_tag.clone(),
                                                    },
                                                );
                                            });

                                            execution_result.set(Some(ExecutionResult {
                                                display_text,
                                                output_bytes,
                                                execution_time_ms: js_sys::Date::now() - exec_start,
                                                type_tag,
                                                ran_on_main_thread,
                                            }));
                                        }
                                        Err(e) => {
                                            error_message
                                                .set(Some(format!("Execution error: {e}")));
                                        }
                                    }
                                }
                                Err(e) => {
                                    error_message.set(Some(format!("WASM load error: {e}")));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error_message.set(Some(format!("Compile error: {e}")));
                    }
                }

                // Advance the run-all queue past this cell whether it
                // succeeded or failed. A failure renders inline on the cell
                // (sometimes deliberately — teaching cells fail on purpose),
                // and the cells behind it must still run; aborting the queue
                // used to strand every later cell with no indication why.
                run_all_queue.update(|q| {
                    if q.first().is_some_and(|id| id == &cell_id) {
                        q.remove(0);
                    }
                });

                compiling.set(false);
            });
        }
    });

    // Suppress unused warning during SSR.
    #[cfg(not(feature = "hydrate"))]
    let _ = &run_cell;

    // Code and output load in the cell's saved default state (the editor's
    // per-cell header toggles); the chevron and the output reveal are
    // transient reader affordances on top.
    let collapsed = RwSignal::new(cell.with_value(|c| c.collapsed));
    let output_collapsed = RwSignal::new(cell.with_value(|c| c.output_collapsed));
    let collapse_icon = Signal::derive(move || if collapsed.get() { "▸" } else { "▾" });
    let body_class = Signal::derive(move || {
        if collapsed.get() {
            "ironpad-cell-body ironpad-cell-body--collapsed"
        } else {
            "ironpad-cell-body"
        }
    });

    // Button text depends on compiling/queued state.
    let button_text = Signal::derive(move || {
        if compiling.get() {
            "◐ Compiling…"
        } else if queued.get() {
            "◐ Queued"
        } else {
            "▶ Run"
        }
    });

    view! {
        <div class="view-only-cell">
            <div class="view-only-cell-header">
                <button
                    class="ironpad-cell-collapse-btn"
                    on:click=move |_| collapsed.update(|c| *c = !*c)
                >
                    {collapse_icon}
                </button>
                <span class="view-only-cell-label">{cell.with_value(|c| c.label.clone())}</span>
                <button
                    class="view-only-run-button"
                    on:click=run_cell
                    disabled=move || compiling.get() || queued.get()
                >
                    {button_text}
                </button>
                {move || {
                    // compile_time_ms is set even when the compile returns an empty
                    // blob (a failed compile with diagnostics), so gate the success
                    // (✓) badge on there being no error — the error panel shows instead.
                    if error_message.get().is_some() {
                        return view! { <span /> }.into_any();
                    }
                    let compile = compile_time_ms.get();
                    let runtime = execution_result.get().map(|r| r.execution_time_ms);
                    match (compile, runtime) {
                        (Some(c), Some(r)) => view! {
                            <span class="view-only-timing-badge">{format!("✓ {c:.0}+{r:.0}ms")}</span>
                        }.into_any(),
                        (Some(c), None) => view! {
                            <span class="view-only-timing-badge">{format!("✓ {c:.0}ms")}</span>
                        }.into_any(),
                        _ => view! { <span /> }.into_any(),
                    }
                }}
            </div>
            <div class=body_class>
                <MonacoEditor
                    initial_value=cell.with_value(|c| c.source.clone())
                    language="rust"
                    read_only=true
                />
            </div>
            {move || error_message.get().map(|err| view! {
                <div class="view-only-error">
                    <pre>{err}</pre>
                </div>
            })}
            {move || execution_result.get().map(|result| {
                if output_collapsed.get() {
                    return view! {
                        <button
                            class="view-only-output-reveal"
                            on:click=move |_| output_collapsed.set(false)
                        >
                            <span class="ironpad-output-toggle">"\u{25b8}"</span>
                            <span>"Output"</span>
                        </button>
                    }.into_any();
                }
                let cell_id = cell.with_value(|c| c.id.clone());
                let all_cells_vec = all_cells.get_value();
                view! { <ViewOnlyOutput result=result cell_id=cell_id all_cells=all_cells_vec run_all_queue=run_all_queue cell_outputs=cell_outputs /> }.into_any()
            })}
        </div>
    }
}

// ── ViewOnlySharedCell ──────────────────────────────────────────────────────

/// Renders a shared cell (PRD-0044): read-only source with the amber shared
/// chrome, no run button and no output — the code compiles as part of every
/// other cell's `shared.rs`, never on its own.
#[component]
fn ViewOnlySharedCell(cell: IronpadCell) -> impl IntoView {
    let collapsed = RwSignal::new(cell.collapsed);
    let collapse_icon = Signal::derive(move || if collapsed.get() { "▸" } else { "▾" });
    let body_class = Signal::derive(move || {
        if collapsed.get() {
            "ironpad-cell-body ironpad-cell-body--collapsed"
        } else {
            "ironpad-cell-body"
        }
    });

    view! {
        <div class="view-only-cell view-only-cell--shared">
            <div class="view-only-cell-header">
                <button
                    class="ironpad-cell-collapse-btn"
                    on:click=move |_| collapsed.update(|c| *c = !*c)
                >
                    {collapse_icon}
                </button>
                <span class="view-only-cell-label">{cell.label.clone()}</span>
                <span class="ironpad-cell-type-badge ironpad-cell-type-badge--shared">
                    "\u{2b21} shared"
                </span>
            </div>
            <div class=body_class>
                <MonacoEditor
                    initial_value=cell.source.clone()
                    language="rust"
                    read_only=true
                />
            </div>
        </div>
    }
}

// ── ViewOnlyMarkdownCell ────────────────────────────────────────────────────

/// Renders markdown as HTML in preview-only mode (no edit toggle).
#[component]
fn ViewOnlyMarkdownCell(#[prop(into)] source: String) -> impl IntoView {
    let html = render_markdown(&source);

    if html.trim().is_empty() {
        view! {
            <div class="view-only-cell view-only-markdown">
                <p class="ironpad-placeholder">"(empty markdown cell)"</p>
            </div>
        }
        .into_any()
    } else {
        view! {
            <div class="view-only-cell view-only-markdown ironpad-markdown-cell-preview" inner_html=html></div>
        }
        .into_any()
    }
}

// ── ViewOnlyOutput ──────────────────────────────────────────────────────────

/// Renders execution output panels (text, HTML, SVG) and timing metadata.
#[allow(clippy::implicit_hasher)]
#[component]
fn ViewOnlyOutput(
    result: ExecutionResult,
    #[prop(into)] cell_id: String,
    all_cells: Vec<IronpadCell>,
    run_all_queue: RwSignal<Vec<String>>,
    cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
) -> impl IntoView {
    let panels: Vec<DisplayPanel> = match &result.display_text {
        Some(json) => {
            serde_json::from_str(json).unwrap_or_else(|_| vec![DisplayPanel::Text(json.clone())])
        }
        None => vec![],
    };

    // Build the widget side-effect sink. The read-only viewer has no reactive
    // runner, so `cell_stale` is `None`: a widget change updates outputs directly
    // without marking downstream cells stale.
    let sink = Some(WidgetSink {
        cell_outputs,
        run_all_queue,
        cells: Signal::derive(move || {
            all_cells
                .iter()
                .map(|c| (c.id.clone(), c.cell_type.clone()))
                .collect()
        }),
        cell_stale: None,
    });

    let output_len = result.output_bytes.len();
    let exec_time = result.execution_time_ms;
    let ran_on_main_thread = result.ran_on_main_thread;

    let collapsed = RwSignal::new(false);
    let toggle_icon = Signal::derive(move || if collapsed.get() { "▸" } else { "▾" });
    let outer_class = Signal::derive(move || {
        if collapsed.get() {
            "view-only-output view-only-output--collapsed"
        } else {
            "view-only-output"
        }
    });

    view! {
        <div class=outer_class>
            <div
                class="view-only-output-meta"
                style="cursor: pointer;"
                on:click=move |_| collapsed.update(|c| *c = !*c)
            >
                <span class="ironpad-output-toggle">{toggle_icon}</span>
                <span>{format!("{output_len} bytes")}</span>
                <span>{format!("{exec_time:.1} ms")}</span>
                {if ran_on_main_thread {
                    view! {
                        <span
                            class="ironpad-output-fallback-badge"
                            title="This cell was re-executed on the main thread because it requires DOM access (e.g. plotters font measurement)"
                        >
                            "⚠ main thread"
                        </span>
                    }.into_any()
                } else {
                    view! { <span /> }.into_any()
                }}
            </div>
            <div class="view-only-output-body" style:display=move || {
                if collapsed.get() { "none" } else { "block" }
            }>
                {panels.into_iter().map(|panel| {
                    render_display_panel(panel, VIEW_ONLY_PANEL_CLASSES, Some(cell_id.clone()), sink)
                }).collect_view()}
            </div>
        </div>
    }
}
