use ironpad_common::{IronpadNotebook, PublicNotebookSummary};
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
#[cfg(feature = "hydrate")]
use leptos_router::NavigateOptions;
use thaw::{Button, ButtonAppearance, Card, CardHeader, Skeleton, SkeletonItem};

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

    let public_resource = Resource::new(|| (), |_| list_public_notebooks());

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
                crate::storage::client::save_notebook(&nb).await;
                navigate(&format!("/notebook/{id}"), NavigateOptions::default());
            });
        }
    };

    view! {
        <div class="ironpad-home">
            <div class="ironpad-home-header">
                <h1>"Notebooks"</h1>
                <Button
                    appearance=ButtonAppearance::Primary
                    on_click=on_create
                >
                    "+ New Notebook"
                </Button>
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
                    >"🔒 Private"</button>
                    <button
                        class=move || if filter_mode.get() == FilterMode::Public { "ironpad-chip active" } else { "ironpad-chip" }
                        on:click=move |_| filter_mode.set(FilterMode::Public)
                    >"🌐 Public"</button>
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

            // Public notebooks (fixed order from index.json).
            if matches!(mode, FilterMode::All | FilterMode::Public) {
                for nb in &public_notebooks {
                    if query.is_empty() || nb.title.to_lowercase().contains(&query) {
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
            let cell_label = if cell_count == 1 { "cell" } else { "cells" };
            let cell_text = format!("{cell_count} {cell_label}");

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
                                <span class="ironpad-notebook-badge private">"🔒"</span>
                                <span class="ironpad-notebook-card-title">{title}</span>
                            </CardHeader>
                            <div class="ironpad-notebook-card-body">
                                <span class="ironpad-notebook-card-cells">{cell_text}</span>
                                <span class="ironpad-notebook-card-updated">{updated_at}</span>
                            </div>
                        </Card>
                    </a>
                    <button class="ironpad-delete-btn" on:click=on_delete title="Delete notebook">
                        "🗑"
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
            let cell_label = if cell_count == 1 { "cell" } else { "cells" };
            let cell_text = format!("{cell_count} {cell_label}");

            view! {
                <div class="ironpad-notebook-card-wrapper">
                    <a href=href class="ironpad-notebook-card-link">
                        <Card class="ironpad-notebook-card">
                            <CardHeader>
                                <span class="ironpad-notebook-badge public">"🌐"</span>
                                <span class="ironpad-notebook-card-title">{title}</span>
                            </CardHeader>
                            <div class="ironpad-notebook-card-body">
                                <p class="ironpad-notebook-card-description">{description}</p>
                                <div class="ironpad-notebook-card-meta">
                                    <span class="ironpad-notebook-card-cells">{cell_text}</span>
                                    {if !tags.is_empty() {
                                        Some(view! {
                                            <div class="ironpad-notebook-card-tags">
                                                {tags.into_iter().map(|tag| {
                                                    view! { <span class="ironpad-tag-pill">{tag}</span> }
                                                }).collect::<Vec<_>>()}
                                            </div>
                                        })
                                    } else {
                                        None
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
