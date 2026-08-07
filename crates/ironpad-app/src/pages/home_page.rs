use crate::components::icon::{Icon, IconLabel};
use crate::components::icons;
#[cfg(feature = "hydrate")]
use crate::components::toaster::Toaster;
use ironpad_common::{IronpadNotebook, PublicNotebookSummary};
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
#[cfg(feature = "hydrate")]
use leptos_router::NavigateOptions;

use crate::components::app_layout::LayoutContext;
use crate::components::social_meta::SocialMeta;
use crate::server_fns::list_public_notebooks;

// ── Types ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
enum NotebookListItem {
    Private {
        id: String,
        title: String,
        cell_count: usize,
        updated_at: String,
    },
    /// A mutable share (PRD-0054): server-enumerated by session; the card
    /// links to `/mutable/{id}`, which is the editor for the owner and the
    /// reader for everyone else. No local copy exists.
    Mutable {
        share_id: String,
        title: String,
        cell_count: usize,
        updated_at: String,
    },
    Public {
        title: String,
        description: String,
        filename: String,
        cell_count: usize,
        tags: Vec<String>,
    },
}

/// A mutable-share row for the home "Published" group (server-enumerated by
/// session; target-agnostic so the signal type is valid on both builds).
#[derive(Clone)]
struct MutableEntry {
    share_id: String,
    title: String,
    cell_count: usize,
    updated_at: String,
}

#[derive(Clone, Copy, PartialEq)]
enum FilterMode {
    All,
    Private,
    Mutable,
    Public,
}

/// Whether a public notebook matches a lowercased search query. Matches
/// title, description, or any tag, so tag-style queries like "blog" or
/// "physics" surface the tagged set directly.
fn public_notebook_matches(summary: &PublicNotebookSummary, query: &str) -> bool {
    query.is_empty()
        || summary.title.to_lowercase().contains(query)
        || summary.description.to_lowercase().contains(query)
        || summary
            .tags
            .iter()
            .any(|t| t.to_lowercase().contains(query))
}

/// Whether a private (`IndexedDB`) notebook matches a lowercased search query:
/// title or any tag, same contract as public search.
fn private_notebook_matches(nb: &IronpadNotebook, query: &str) -> bool {
    query.is_empty()
        || nb.title.to_lowercase().contains(query)
        || nb
            .tags
            .as_ref()
            .is_some_and(|tags| tags.iter().any(|t| t.to_lowercase().contains(query)))
}

// ── Home page ───────────────────────────────────────────────────────────────

/// Home page showing private (IndexedDB) and public notebooks with search and
/// filter controls.
#[component]
pub fn HomePage() -> impl IntoView {
    // Reset layout context for home page.

    let ctx = expect_context::<LayoutContext>();
    ctx.notebook_title.set(None);
    ctx.cell_count.set(0);
    ctx.last_save_time.set(None);

    // Load public notebooks (participates in SSR).

    let public_resource = Resource::new(|| (), |()| list_public_notebooks());

    // Private notebooks (IndexedDB, client-only).

    let private_notebooks: RwSignal<Vec<IronpadNotebook>> = RwSignal::new(vec![]);

    #[cfg(feature = "hydrate")]
    {
        leptos::task::spawn_local(async move {
            let nbs = crate::storage::client::list_notebooks().await;
            private_notebooks.set(nbs);
        });
    }

    // Mutable shares (PRD-0054): server-enumerated by session, one source of
    // truth. Anonymous gets an empty list; there is nothing local to merge.

    let mutable_entries: RwSignal<Vec<MutableEntry>> = RwSignal::new(vec![]);

    #[cfg(feature = "hydrate")]
    {
        leptos::task::spawn_local(async move {
            if let Ok(remote) = crate::server_fns::list_mutable_shares().await {
                let entries: Vec<MutableEntry> = remote
                    .into_iter()
                    .map(|s| MutableEntry {
                        share_id: s.id,
                        title: s.title,
                        cell_count: s.cell_count,
                        updated_at: s.pushed_at.get(..10).unwrap_or(&s.pushed_at).to_string(),
                    })
                    .collect();
                let _ = mutable_entries.try_set(entries);
            }
        });
    }

    // Search and filter state.

    let search_query = RwSignal::new(String::new());
    let filter_mode = RwSignal::new(FilterMode::All);

    // Create notebook (IndexedDB, client-only).

    let navigate = use_navigate();
    let on_create = move |_| {
        let _ = &navigate;
        #[cfg(feature = "hydrate")]
        {
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                let nb = IronpadNotebook::new("Untitled Notebook");
                let id = nb.id.to_string();
                match crate::storage::client::save_notebook(&nb).await {
                    Ok(()) => navigate(&format!("/local/{id}"), NavigateOptions::default()),
                    Err(e) => {
                        leptos::logging::error!(
                            "failed to persist new notebook to IndexedDB: {e:?}"
                        );
                    }
                }
            });
        }
    };

    // Import notebook from file (IndexedDB, client-only). The toaster must be
    // resolved here, inside the component's reactive owner: the import flow's
    // FileReader callbacks run outside any owner, where `expect_context` panics
    // (which used to swallow both the success and the rejection toast).
    #[cfg(feature = "hydrate")]
    let toaster = Toaster::expect_context();

    let on_import = move |_| {
        let _ = &private_notebooks;
        #[cfg(feature = "hydrate")]
        {
            import_notebook_from_file(private_notebooks, toaster);
        }
    };

    view! {
        // Static, so it needs no resource and lands in the first flush of the
        // head without the async SSR mode the notebook routes require.
        <SocialMeta
            title="ironpad"
            description=Some(
                "Write Rust in a notebook. Cells compile to WebAssembly and run in your browser."
                    .to_string(),
            )
            path="/"
            image="/og/ironpad.png"
            kind="website"
        />
        <div class="ironpad-home">
            <div class="ironpad-home-header">
                <div class="ironpad-home-header-text">
                    <h1>"Notebooks"</h1>
                    <p class="ironpad-home-tagline">"Interactive Rust notebooks — compile to WebAssembly, run in the browser."</p>
                </div>
                <div class="ironpad-home-actions">
                    <button
                        class="ironpad-btn ironpad-btn--primary"
                        on:click=on_create
                    >
                        "+ New Notebook"
                    </button>
                    <button
                        class="ironpad-btn ironpad-btn--subtle"
                        on:click=on_import
                    >
                        <IconLabel icon=icons::IMPORT label="Import Notebook"/>
                    </button>
                </div>
            </div>

            <div class="ironpad-home-toolbar">
                <input
                    type="text"
                    class="ironpad-search-input"
                    placeholder="Search notebooks..."
                    on:input=move |ev| search_query.set(event_target_value(&ev))
                />
                <div class="ironpad-filter-chips">
                    <button
                        class=move || if filter_mode.get() == FilterMode::All { "ironpad-chip active" } else { "ironpad-chip" }
                        on:click=move |_| filter_mode.set(FilterMode::All)
                    >"All"</button>
                    <button
                        class=move || if filter_mode.get() == FilterMode::Private { "ironpad-chip active" } else { "ironpad-chip" }
                        on:click=move |_| filter_mode.set(FilterMode::Private)
                    ><IconLabel icon=icons::PRIVATE label="Private"/></button>
                    <button
                        class=move || if filter_mode.get() == FilterMode::Mutable { "ironpad-chip active" } else { "ironpad-chip" }
                        on:click=move |_| filter_mode.set(FilterMode::Mutable)
                    ><IconLabel icon=icons::PUBLISHED label="Published"/></button>
                    <button
                        class=move || if filter_mode.get() == FilterMode::Public { "ironpad-chip active" } else { "ironpad-chip" }
                        on:click=move |_| filter_mode.set(FilterMode::Public)
                    ><IconLabel icon=icons::PUBLIC label="Public"/></button>
                </div>
            </div>

            <Suspense fallback=move || view! {
                <div class="ironpad-notebook-grid">
                    <NotebookCardSkeleton />
                    <NotebookCardSkeleton />
                    <NotebookCardSkeleton />
                </div>
            }>
                {move || Suspend::new(async move {
                    let public_list = public_resource.await.unwrap_or_default();

                    view! {
                        <NotebookGrid
                            public_notebooks=public_list
                            private_notebooks=private_notebooks
                            mutable_entries=mutable_entries
                            search_query=search_query
                            filter_mode=filter_mode
                        />
                    }.into_any()
                })}
            </Suspense>
        </div>
    }
}

// ── Notebook grid ───────────────────────────────────────────────────────────

/// Reactive grid that merges private and public notebooks with search/filter.
#[component]
fn NotebookGrid(
    public_notebooks: Vec<PublicNotebookSummary>,
    private_notebooks: RwSignal<Vec<IronpadNotebook>>,
    mutable_entries: RwSignal<Vec<MutableEntry>>,
    search_query: RwSignal<String>,
    filter_mode: RwSignal<FilterMode>,
) -> impl IntoView {
    let filtered_items = {
        let public_notebooks = public_notebooks.clone();
        move || {
            let query = search_query.get().to_lowercase();
            let mode = filter_mode.get();
            let private = private_notebooks.get();

            let mut items: Vec<NotebookListItem> = vec![];

            // Private notebooks first (already sorted by updated_at desc from IndexedDB).
            if matches!(mode, FilterMode::All | FilterMode::Private) {
                for nb in &private {
                    if private_notebook_matches(nb, &query) {
                        items.push(NotebookListItem::Private {
                            id: nb.id.to_string(),
                            title: nb.title.clone(),
                            cell_count: nb.cells.len(),
                            updated_at: nb.updated_at.format("%b %d, %Y").to_string(),
                        });
                    }
                }
            }

            // Mutable shares (PRD-0054), between private and public.
            if matches!(mode, FilterMode::All | FilterMode::Mutable) {
                for e in &mutable_entries.get() {
                    if query.is_empty() || e.title.to_lowercase().contains(&query) {
                        items.push(NotebookListItem::Mutable {
                            share_id: e.share_id.clone(),
                            title: e.title.clone(),
                            cell_count: e.cell_count,
                            updated_at: e.updated_at.clone(),
                        });
                    }
                }
            }

            // Public notebooks (sorted alphabetically by title).
            if matches!(mode, FilterMode::All | FilterMode::Public) {
                for nb in &public_notebooks {
                    if public_notebook_matches(nb, &query) {
                        items.push(NotebookListItem::Public {
                            title: nb.title.clone(),
                            description: nb.description.clone(),
                            filename: nb.filename.clone(),
                            cell_count: nb.cell_count,
                            tags: nb.tags.clone(),
                        });
                    }
                }
            }

            items
        }
    };

    view! {
        {move || {
            let items = filtered_items();
            if items.is_empty() {
                view! {
                    <div class="ironpad-home-empty">
                        <p>"No notebooks found."</p>
                        <p>"Create one to get started!"</p>
                    </div>
                }.into_any()
            } else {
                view! {
                    <div class="ironpad-notebook-grid">
                        {items.into_iter().map(|item| {
                            view! { <NotebookCard item=item private_notebooks=private_notebooks /> }
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_any()
            }
        }}
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn format_cell_count(count: usize) -> String {
    let label = if count == 1 { "cell" } else { "cells" };
    format!("{count} {label}")
}

// ── Notebook card ───────────────────────────────────────────────────────────

/// A single notebook card, rendering either a private or public variant.
#[component]
fn NotebookCard(
    item: NotebookListItem,
    private_notebooks: RwSignal<Vec<IronpadNotebook>>,
) -> impl IntoView {
    let _ = &private_notebooks;
    match item {
        NotebookListItem::Private {
            id,
            title,
            cell_count,
            updated_at,
        } => {
            let href = format!("/local/{id}");
            let cell_text = format_cell_count(cell_count);

            #[cfg(feature = "hydrate")]
            let delete_id = id.clone();
            let on_delete = move |_| {
                #[cfg(feature = "hydrate")]
                {
                    let id = delete_id.clone();
                    let confirmed = web_sys::window()
                        .unwrap()
                        .confirm_with_message("Delete this notebook? This cannot be undone.")
                        .unwrap_or(false);
                    if confirmed {
                        leptos::task::spawn_local(async move {
                            crate::storage::client::delete_notebook(&id).await;
                            let nbs = crate::storage::client::list_notebooks().await;
                            private_notebooks.set(nbs);
                        });
                    }
                }
            };

            view! {
                <div class="ironpad-notebook-card-wrapper">
                    <a href=href class="ironpad-notebook-card-link">
                        <div class="ironpad-notebook-card">
                            <div class="ironpad-notebook-card-header">
                                <span class="ironpad-notebook-badge private"><Icon icon=icons::PRIVATE/></span>
                                <span class="ironpad-notebook-card-title">{title}</span>
                            </div>
                            <div class="ironpad-notebook-card-body">
                                <span class="ironpad-notebook-card-cells">{cell_text}</span>
                                <span class="ironpad-notebook-card-updated">{updated_at}</span>
                            </div>
                        </div>
                    </a>
                    <button class="ironpad-delete-btn" on:click=on_delete title="Delete notebook">
                        <Icon icon=icons::DELETE/>
                    </button>
                </div>
            }
            .into_any()
        }

        NotebookListItem::Mutable {
            share_id,
            title,
            cell_count,
            updated_at,
        } => {
            // One address (PRD-0054): the same link is the editor for the
            // owner and the reader for everyone else.
            let href = format!("/mutable/{share_id}");
            let cell_text = format_cell_count(cell_count);
            let hint = "published";

            view! {
                <div class="ironpad-notebook-card-wrapper">
                    <a href=href class="ironpad-notebook-card-link">
                        <div class="ironpad-notebook-card">
                            <div class="ironpad-notebook-card-header">
                                <span class="ironpad-notebook-badge mutable"><Icon icon=icons::PUBLISHED/></span>
                                <span class="ironpad-notebook-card-title">{title}</span>
                            </div>
                            <div class="ironpad-notebook-card-body">
                                <span class="ironpad-notebook-card-cells">{cell_text}</span>
                                <span class="ironpad-notebook-card-updated">{updated_at}</span>
                                <span class="ironpad-notebook-card-mutable-hint">{hint}</span>
                            </div>
                        </div>
                    </a>
                </div>
            }
            .into_any()
        }

        NotebookListItem::Public {
            title,
            description,
            filename,
            cell_count,
            tags,
        } => {
            // Canonical public URLs are extension-less (PRD-0048).
            let href = format!(
                "/public/{}",
                filename.strip_suffix(".ironpad").unwrap_or(&filename)
            );
            let cell_text = format_cell_count(cell_count);

            view! {
                <div class="ironpad-notebook-card-wrapper">
                    <a href=href class="ironpad-notebook-card-link">
                        <div class="ironpad-notebook-card">
                            <div class="ironpad-notebook-card-header">
                                <span class="ironpad-notebook-badge public"><Icon icon=icons::PUBLIC/></span>
                                <span class="ironpad-notebook-card-title">{title}</span>
                            </div>
                            <div class="ironpad-notebook-card-body">
                                <p class="ironpad-notebook-card-description">{description}</p>
                                <div class="ironpad-notebook-card-meta">
                                    <span class="ironpad-notebook-card-cells">{cell_text}</span>
                                    {if tags.is_empty() {
                                        None
                                    } else {
                                        Some(view! {
                                            <div class="ironpad-notebook-card-tags">
                                                {tags.into_iter().map(|tag| {
                                                    view! { <span class="ironpad-tag-pill">{tag}</span> }
                                                }).collect::<Vec<_>>()}
                                            </div>
                                        })
                                    }}
                                </div>
                            </div>
                        </div>
                    </a>
                </div>
            }
            .into_any()
        }
    }
}

// ── Notebook card skeleton ──────────────────────────────────────────────────

/// A skeleton placeholder shown while notebook cards are loading.
#[component]
fn NotebookCardSkeleton() -> impl IntoView {
    view! {
        <div class="ironpad-notebook-card-skeleton">
            <div class="ironpad-skeleton-item ironpad-skeleton-title" />
            <div class="ironpad-skeleton-item ironpad-skeleton-meta" />
        </div>
    }
}

// ── Notebook import ─────────────────────────────────────────────────────────

/// Opens a file picker, reads the selected `.ironpad`/`.json` file, validates
/// it, imports it via `storage.js`, and refreshes the notebook list.
///
/// Takes the toaster by value: the `FileReader` callbacks below run outside any
/// reactive owner, where `Toaster::expect_context()` panics, so the caller
/// resolves it inside the component and moves it in.
#[cfg(feature = "hydrate")]
fn import_notebook_from_file(private_notebooks: RwSignal<Vec<IronpadNotebook>>, toaster: Toaster) {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use crate::components::toaster::ToastIntent;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    // Closures kept alive across the async file-picker flow (see below).
    type KeepAlive = Rc<RefCell<Vec<Closure<dyn Fn()>>>>;

    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };

    // Create a hidden <input type="file"> element.
    let Ok(input_el) = document.create_element("input") else {
        return;
    };
    let Ok(input) = input_el.dyn_into::<web_sys::HtmlInputElement>() else {
        return;
    };
    input.set_type("file");
    input.set_accept(".ironpad,.json");
    input.set_attribute("style", "display:none").ok();

    // Attach to the DOM so it can fire events.
    if let Some(body) = document.body() {
        let _ = body.append_child(&input);
    }

    // Keep every JS closure alive here — nothing else owns them once this fn
    // returns, so `input`'s onchange/cancel callbacks would otherwise dangle.
    // This is a deliberate Rc cycle (closures ↔ keepalive) that `cleanup` breaks
    // — on file selection OR dialog dismissal (`cancel`) — so the hidden <input>
    // and every closure are freed instead of leaked. (Previously each import
    // `forget()`-leaked its closures and orphaned the <input> on a cancel.)
    let keepalive: KeepAlive = Rc::new(RefCell::new(Vec::new()));
    let cleaned = Rc::new(Cell::new(false));

    let cleanup = {
        let keepalive = keepalive.clone();
        let cleaned = cleaned.clone();
        let input = input.clone();
        move || {
            if cleaned.replace(true) {
                return;
            }
            if let Some(parent) = input.parent_node() {
                let _ = parent.remove_child(&input);
            }
            // A closure can't drop itself while running, so defer clearing the
            // keepalive to a 0 ms timer; `once_into_js` frees that callback when
            // it fires.
            let keepalive = keepalive.clone();
            let drop_cb = Closure::once_into_js(move || {
                keepalive.borrow_mut().clear();
            });
            if let Some(win) = web_sys::window() {
                let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
                    drop_cb.unchecked_ref(),
                    0,
                );
            }
        }
    };

    // File selected: read, validate, and import it.
    let on_change = {
        let input = input.clone();
        let cleanup = cleanup.clone();
        let keepalive = keepalive.clone();
        Closure::<dyn Fn()>::new(move || {
            let Some(file) = input.files().and_then(|files| files.get(0)) else {
                cleanup();
                return;
            };
            let Ok(reader) = web_sys::FileReader::new() else {
                cleanup();
                return;
            };

            let reader_clone = reader.clone();
            let cleanup = cleanup.clone();
            let on_load = Closure::<dyn Fn()>::new(move || {
                let text = reader_clone.result().ok().and_then(|r| r.as_string());
                // Done with the <input> and closures once we hold the bytes.
                cleanup();
                let Some(text) = text else {
                    return;
                };

                // Validate the JSON before importing.
                if let Err(msg) = crate::storage::validate::validate_notebook_json(&text) {
                    toaster.toast(ToastIntent::Error, "Import Failed", msg, 5);
                    return;
                }

                // Import and refresh the notebook list.
                leptos::task::spawn_local(async move {
                    match crate::storage::client::import_notebook(&text).await {
                        Some(nb) => {
                            let title = nb.title.clone();
                            let nbs = crate::storage::client::list_notebooks().await;
                            private_notebooks.set(nbs);
                            toaster.toast(
                                ToastIntent::Success,
                                "Notebook Imported",
                                format!("\"{title}\" has been added to your notebooks."),
                                3,
                            );
                        }
                        None => {
                            toaster.toast(
                                ToastIntent::Error,
                                "Import Failed",
                                "Failed to import the notebook. The file may be corrupted.",
                                5,
                            );
                        }
                    }
                });
            });

            reader.set_onload(Some(on_load.as_ref().unchecked_ref()));
            keepalive.borrow_mut().push(on_load);
            let _ = reader.read_as_text(&file);
        })
    };

    // Dialog dismissed without a selection: no `change` fires, so release the
    // input and closures here instead of orphaning them.
    let on_cancel = Closure::<dyn Fn()>::new(cleanup);

    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    let _ = input.add_event_listener_with_callback("cancel", on_cancel.as_ref().unchecked_ref());
    keepalive.borrow_mut().push(on_change);
    keepalive.borrow_mut().push(on_cancel);

    // Trigger the file picker.
    input.click();
}

#[cfg(test)]
mod search_tests {
    use super::public_notebook_matches;
    use ironpad_common::PublicNotebookSummary;

    fn summary() -> PublicNotebookSummary {
        PublicNotebookSummary {
            id: "autodiff".into(),
            title: "std::autodiff: Differentiation in the Compiler".into(),
            description: "Exact derivatives without writing them.".into(),
            filename: "autodiff.ironpad".into(),
            cell_count: 12,
            tags: vec!["blog".into(), "autodiff".into(), "machine-learning".into()],
        }
    }

    #[test]
    fn private_search_matches_title_and_tags() {
        use ironpad_common::IronpadNotebook;
        let mut nb = IronpadNotebook::new("My Physics Scratchpad");
        nb.tags = Some(vec!["blog".into(), "wip".into()]);

        assert!(super::private_notebook_matches(&nb, ""));
        assert!(super::private_notebook_matches(&nb, "physics"));
        assert!(super::private_notebook_matches(&nb, "blog"));
        assert!(!super::private_notebook_matches(&nb, "quaternions"));

        nb.tags = None;
        assert!(super::private_notebook_matches(&nb, "scratchpad"));
        assert!(!super::private_notebook_matches(&nb, "blog"));
    }

    #[test]
    fn search_matches_title_description_and_tags() {
        let nb = summary();
        // Queries arrive pre-lowercased (the caller lowercases once).
        assert!(public_notebook_matches(&nb, ""));
        assert!(public_notebook_matches(&nb, "autodiff"));
        assert!(public_notebook_matches(&nb, "derivatives"));
        assert!(public_notebook_matches(&nb, "blog"));
        assert!(public_notebook_matches(&nb, "machine-learning"));
        assert!(!public_notebook_matches(&nb, "quaternions"));
    }
}
