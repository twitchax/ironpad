use ironpad_common::{IronpadNotebook, PublicNotebookSummary};
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
#[cfg(feature = "hydrate")]
use leptos_router::NavigateOptions;
use thaw::{Button, ButtonAppearance, Card, CardHeader, Skeleton, SkeletonItem};
#[cfg(feature = "hydrate")]
use thaw::{Toast, ToastBody, ToastTitle, ToasterInjection};

use crate::components::app_layout::LayoutContext;
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
    Public {
        title: String,
        description: String,
        filename: String,
        cell_count: usize,
        tags: Vec<String>,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum FilterMode {
    All,
    Private,
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

// ── Home page ───────────────────────────────────────────────────────────────

/// Home page showing private (IndexedDB) and public notebooks with search and
/// filter controls.
#[component]
pub fn HomePage() -> impl IntoView {
    // Reset layout context for home page.

    let ctx = expect_context::<LayoutContext>();
    ctx.notebook_title.set(None);
    ctx.show_save_button.set(false);
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
                    Ok(()) => navigate(&format!("/notebook/{id}"), NavigateOptions::default()),
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
    let toaster = ToasterInjection::expect_context();

    let on_import = move |_| {
        let _ = &private_notebooks;
        #[cfg(feature = "hydrate")]
        {
            import_notebook_from_file(private_notebooks, toaster);
        }
    };

    view! {
        <div class="ironpad-home">
            <div class="ironpad-home-header">
                <div class="ironpad-home-header-text">
                    <h1>"Notebooks"</h1>
                    <p class="ironpad-home-tagline">"Interactive Rust notebooks — compile to WebAssembly, run in the browser."</p>
                </div>
                <div class="ironpad-home-actions">
                    <Button
                        appearance=ButtonAppearance::Primary
                        on_click=on_create
                    >
                        "+ New Notebook"
                    </Button>
                    <Button
                        appearance=ButtonAppearance::Subtle
                        on_click=on_import
                    >
                        "↑ Import Notebook"
                    </Button>
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
                    >"◆ Private"</button>
                    <button
                        class=move || if filter_mode.get() == FilterMode::Public { "ironpad-chip active" } else { "ironpad-chip" }
                        on:click=move |_| filter_mode.set(FilterMode::Public)
                    >"◇ Public"</button>
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
                    if query.is_empty() || nb.title.to_lowercase().contains(&query) {
                        items.push(NotebookListItem::Private {
                            id: nb.id.to_string(),
                            title: nb.title.clone(),
                            cell_count: nb.cells.len(),
                            updated_at: nb.updated_at.format("%b %d, %Y").to_string(),
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
            let href = format!("/notebook/{id}");
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
                        <Card class="ironpad-notebook-card">
                            <CardHeader>
                                <span class="ironpad-notebook-badge private">"◆"</span>
                                <span class="ironpad-notebook-card-title">{title}</span>
                            </CardHeader>
                            <div class="ironpad-notebook-card-body">
                                <span class="ironpad-notebook-card-cells">{cell_text}</span>
                                <span class="ironpad-notebook-card-updated">{updated_at}</span>
                            </div>
                        </Card>
                    </a>
                    <button class="ironpad-delete-btn" on:click=on_delete title="Delete notebook">
                        "╳"
                    </button>
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
            let href = format!("/notebook/public/{filename}");
            let cell_text = format_cell_count(cell_count);

            view! {
                <div class="ironpad-notebook-card-wrapper">
                    <a href=href class="ironpad-notebook-card-link">
                        <Card class="ironpad-notebook-card">
                            <CardHeader>
                                <span class="ironpad-notebook-badge public">"◇"</span>
                                <span class="ironpad-notebook-card-title">{title}</span>
                            </CardHeader>
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
                        </Card>
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
        <Skeleton class="ironpad-notebook-card-skeleton">
            <SkeletonItem class="ironpad-skeleton-title" />
            <SkeletonItem class="ironpad-skeleton-meta" />
        </Skeleton>
    }
}

// ── Notebook import ─────────────────────────────────────────────────────────

/// Opens a file picker, reads the selected `.ironpad`/`.json` file, validates
/// it, imports it via `storage.js`, and refreshes the notebook list.
///
/// Takes the toaster by value: the `FileReader` callbacks below run outside any
/// reactive owner, where `ToasterInjection::expect_context()` panics, so the
/// caller resolves it inside the component and moves it in.
#[cfg(feature = "hydrate")]
fn import_notebook_from_file(
    private_notebooks: RwSignal<Vec<IronpadNotebook>>,
    toaster: ToasterInjection,
) {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::Duration;

    use thaw::{ToastIntent, ToastOptions};
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
                    toaster.dispatch_toast(
                        move || {
                            view! {
                                <Toast>
                                    <ToastTitle>"Import Failed"</ToastTitle>
                                    <ToastBody>{msg.clone()}</ToastBody>
                                </Toast>
                            }
                        },
                        ToastOptions::default()
                            .with_intent(ToastIntent::Error)
                            .with_timeout(Duration::from_secs(5)),
                    );
                    return;
                }

                // Import and refresh the notebook list.
                leptos::task::spawn_local(async move {
                    match crate::storage::client::import_notebook(&text).await {
                        Some(nb) => {
                            let title = nb.title.clone();
                            let nbs = crate::storage::client::list_notebooks().await;
                            private_notebooks.set(nbs);
                            toaster.dispatch_toast(
                                move || {
                                    view! {
                                        <Toast>
                                            <ToastTitle>"Notebook Imported"</ToastTitle>
                                            <ToastBody>
                                                {format!("\"{title}\" has been added to your notebooks.")}
                                            </ToastBody>
                                        </Toast>
                                    }
                                },
                                ToastOptions::default()
                                    .with_intent(ToastIntent::Success)
                                    .with_timeout(Duration::from_secs(3)),
                            );
                        }
                        None => {
                            toaster.dispatch_toast(
                                move || {
                                    view! {
                                        <Toast>
                                            <ToastTitle>"Import Failed"</ToastTitle>
                                            <ToastBody>
                                                "Failed to import the notebook. The file may be corrupted."
                                            </ToastBody>
                                        </Toast>
                                    }
                                },
                                ToastOptions::default()
                                    .with_intent(ToastIntent::Error)
                                    .with_timeout(Duration::from_secs(5)),
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
