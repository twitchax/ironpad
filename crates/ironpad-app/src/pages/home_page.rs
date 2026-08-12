use crate::components::icon::{Icon, IconData, IconLabel};
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
    /// A local (`IndexedDB`) notebook, this browser only. Named for the
    /// storage class like its sibling chips and its route (`/local/{id}`):
    /// "private" described the audience, and an unpublished account
    /// notebook is private too (PRD-0064).
    Local {
        id: String,
        title: String,
        cell_count: usize,
        updated_at: String,
    },
    /// An account notebook (PRD-0054, PRD-0064): server-enumerated by
    /// session; the card links to `/mutable/{id}`, which is the editor for
    /// the owner and the reader for everyone else. No local copy exists.
    /// Listed whether or not it has ever been published: unpublished is a
    /// flag on the row, not a separate storage class.
    Mutable {
        share_id: String,
        title: String,
        cell_count: usize,
        updated_at: String,
        published: bool,
    },
    Public {
        title: String,
        description: String,
        filename: String,
        cell_count: usize,
        tags: Vec<String>,
    },
}

/// A server-stored notebook for the home "Account" group (enumerated by
/// session; target-agnostic so the signal type is valid on both builds).
#[derive(Clone)]
struct MutableEntry {
    share_id: String,
    title: String,
    cell_count: usize,
    updated_at: String,
    /// Whether a reader-visible published copy exists (PRD-0064).
    published: bool,
    tags: Vec<String>,
}

/// The storage class a chip filters to. Named after where the notebook
/// LIVES, not who can read it: publishing is a flag on an account notebook
/// (PRD-0064), so "Published" stopped naming a group the moment an account
/// notebook could be unpublished.
#[derive(Clone, Copy, PartialEq)]
enum FilterMode {
    All,
    /// `IndexedDB`, this browser only.
    Local,
    /// Server-stored and owned by the signed-in user, published or not.
    Account,
    /// The bundled static showcase notebooks.
    Public,
}

// ── Icon roles ──────────────────────────────────────────────────────────────

// The chips sort notebooks by WHERE THEY LIVE, so each one wears the icon of
// its storage class. Named here rather than written inline in the markup so
// the rule below is assertable: the Account group holds published and
// unpublished notebooks at once, which means its chip cannot wear the badge
// of either half without contradicting the other.
//
// (`LOCAL_CHIP_ICON` and `PUBLIC_CHIP_ICON` are the same diamond on purpose:
// filled vs outline is a `.private` CSS rule, per the PRD-0062 sweep.)

/// Local (`IndexedDB`) notebooks.
const LOCAL_CHIP_ICON: IconData = icons::PRIVATE;
/// Server-stored notebooks owned by the signed-in user, published or not.
const ACCOUNT_CHIP_ICON: IconData = icons::ACCOUNT;
/// Bundled static showcase notebooks.
const PUBLIC_CHIP_ICON: IconData = icons::PUBLIC;

/// The badge class and icon an account card wears.
///
/// A globe only once there is something for the world to read: an account
/// notebook with no published copy is 404 to everyone but its owner
/// (PRD-0064), so it wears the lock and the muted colour instead.
fn account_badge(published: bool) -> (&'static str, IconData) {
    if published {
        ("ironpad-notebook-badge mutable", icons::PUBLISHED)
    } else {
        ("ironpad-notebook-badge mutable unpublished", icons::LOCKED)
    }
}

// ── Search ──────────────────────────────────────────────────────────────────

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

/// Whether a title/tag pair matches a lowercased search query. The shared
/// contract for the two storage classes that carry no description: local
/// (`IndexedDB`) and account notebooks. One helper so an account notebook is
/// searchable on exactly the fields a local one is.
fn title_or_tag_matches(title: &str, tags: &[String], query: &str) -> bool {
    query.is_empty()
        || title.to_lowercase().contains(query)
        || tags.iter().any(|t| t.to_lowercase().contains(query))
}

/// Whether a local (`IndexedDB`) notebook matches a lowercased search query:
/// title or any tag, same contract as public search.
fn local_notebook_matches(nb: &IronpadNotebook, query: &str) -> bool {
    title_or_tag_matches(&nb.title, nb.tags.as_deref().unwrap_or(&[]), query)
}

/// Whether an account notebook matches a lowercased search query. Published
/// or not: an unpublished notebook is only findable here, so dropping it
/// from search would hide it from its owner entirely.
fn account_notebook_matches(entry: &MutableEntry, query: &str) -> bool {
    title_or_tag_matches(&entry.title, &entry.tags, query)
}

/// Collect the cards to render, applying the search query and the storage
/// class filter. Pure so the chip and search contracts are unit-testable:
/// the grid below is a thin reactive wrapper over this.
fn collect_items(
    local: &[IronpadNotebook],
    account: &[MutableEntry],
    public: &[PublicNotebookSummary],
    query: &str,
    mode: FilterMode,
) -> Vec<NotebookListItem> {
    let mut items: Vec<NotebookListItem> = vec![];

    // Local notebooks first (already sorted by updated_at desc from IndexedDB).
    if matches!(mode, FilterMode::All | FilterMode::Local) {
        for nb in local {
            if local_notebook_matches(nb, query) {
                items.push(NotebookListItem::Local {
                    id: nb.id.to_string(),
                    title: nb.title.clone(),
                    cell_count: nb.cells.len(),
                    updated_at: nb.updated_at.format("%b %d, %Y").to_string(),
                });
            }
        }
    }

    // Account notebooks (PRD-0054, PRD-0064), between local and public.
    if matches!(mode, FilterMode::All | FilterMode::Account) {
        for e in account {
            if account_notebook_matches(e, query) {
                items.push(NotebookListItem::Mutable {
                    share_id: e.share_id.clone(),
                    title: e.title.clone(),
                    cell_count: e.cell_count,
                    updated_at: e.updated_at.clone(),
                    published: e.published,
                });
            }
        }
    }

    // Public notebooks (sorted alphabetically by title).
    if matches!(mode, FilterMode::All | FilterMode::Public) {
        for nb in public {
            if public_notebook_matches(nb, query) {
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

// ── Home page ───────────────────────────────────────────────────────────────

/// Home page showing all three storage classes with search and filter
/// controls: local (`IndexedDB`), account (server-stored and owned by the
/// signed-in user, published or not), and public.
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

    // Account notebooks (PRD-0054, PRD-0064): server-enumerated by session,
    // one source of truth, published or not. Anonymous gets an empty list;
    // there is nothing local to merge.

    let mutable_entries: RwSignal<Vec<MutableEntry>> = RwSignal::new(vec![]);

    #[cfg(feature = "hydrate")]
    {
        leptos::task::spawn_local(async move {
            match crate::server_fns::list_mutable_shares().await {
                Ok(remote) => {
                    let entries: Vec<MutableEntry> = remote
                        .into_iter()
                        .map(|s| {
                            // Last publish, else creation: an unpublished
                            // account notebook has never been pushed
                            // (PRD-0064).
                            let activity = s.last_activity();
                            let updated_at = activity.get(..10).unwrap_or(activity).to_string();
                            MutableEntry {
                                share_id: s.id,
                                title: s.title,
                                cell_count: s.cell_count,
                                updated_at,
                                published: s.published,
                                tags: s.tags,
                            }
                        })
                        .collect();
                    let _ = mutable_entries.try_set(entries);
                }
                // Never swallowed. An anonymous caller gets `Ok(vec![])`, so
                // reaching this arm always means something is actually
                // wrong, and the only symptom on screen is a group that
                // renders empty — indistinguishable from owning nothing.
                //
                // The likeliest cause is a decode failure rather than a
                // transport one: `pushed_at` widened to an option in
                // PRD-0064 and an unpublished notebook puts an explicit
                // null on the wire, which a bundle compiled against the old
                // `String` rejects for the WHOLE `Vec`. That is precisely
                // the stale-cache failure mode in CLAUDE.md's
                // browser-cache-hygiene note, which has shipped a live bug
                // here before, so the console must name it.
                Err(e) => {
                    leptos::logging::error!("failed to list account notebooks: {e:?}");
                }
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
                    <p class="ironpad-home-tagline">"Interactive Rust notebooks: compile to WebAssembly, run in the browser."</p>
                </div>
                <div class="ironpad-home-actions">
                    <button
                        class="ironpad-btn ironpad-btn--primary"
                        on:click=on_create
                    >
                        <IconLabel icon=icons::ADD label="New Notebook"/>
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
                        class=move || if filter_mode.get() == FilterMode::Local { "ironpad-chip active" } else { "ironpad-chip" }
                        on:click=move |_| filter_mode.set(FilterMode::Local)
                    ><IconLabel icon=LOCAL_CHIP_ICON label="Local"/></button>
                    <button
                        class=move || if filter_mode.get() == FilterMode::Account { "ironpad-chip active" } else { "ironpad-chip" }
                        on:click=move |_| filter_mode.set(FilterMode::Account)
                    ><IconLabel icon=ACCOUNT_CHIP_ICON label="Account"/></button>
                    <button
                        class=move || if filter_mode.get() == FilterMode::Public { "ironpad-chip active" } else { "ironpad-chip" }
                        on:click=move |_| filter_mode.set(FilterMode::Public)
                    ><IconLabel icon=PUBLIC_CHIP_ICON label="Public"/></button>
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

/// Reactive grid that merges all three storage classes with search/filter.
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
            collect_items(
                &private_notebooks.get(),
                &mutable_entries.get(),
                &public_notebooks,
                &query,
                filter_mode.get(),
            )
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
        NotebookListItem::Local {
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
            published,
        } => {
            // One address (PRD-0054): the same link is the editor for the
            // owner and the reader for everyone else.
            let href = format!("/mutable/{share_id}");
            let cell_text = format_cell_count(cell_count);
            // Only the published ones carry the hint under the card body.
            let (badge_class, badge_icon) = account_badge(published);

            view! {
                <div class="ironpad-notebook-card-wrapper">
                    <a href=href class="ironpad-notebook-card-link">
                        <div class="ironpad-notebook-card">
                            <div class="ironpad-notebook-card-header">
                                <span class=badge_class><Icon icon=badge_icon/></span>
                                <span class="ironpad-notebook-card-title">{title}</span>
                            </div>
                            <div class="ironpad-notebook-card-body">
                                <span class="ironpad-notebook-card-cells">{cell_text}</span>
                                <span class="ironpad-notebook-card-updated">{updated_at}</span>
                                {published.then(|| view! {
                                    <span class="ironpad-notebook-card-mutable-hint">"published"</span>
                                })}
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
    use super::{
        account_badge, account_notebook_matches, collect_items, local_notebook_matches,
        public_notebook_matches, FilterMode, MutableEntry, NotebookListItem, ACCOUNT_CHIP_ICON,
        LOCAL_CHIP_ICON, PUBLIC_CHIP_ICON,
    };
    use ironpad_common::{IronpadNotebook, PublicNotebookSummary};

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

    fn account(title: &str, published: bool, tags: &[&str]) -> MutableEntry {
        MutableEntry {
            share_id: format!("{title}-id"),
            title: title.into(),
            cell_count: 3,
            updated_at: "2026-08-12".into(),
            published,
            tags: tags.iter().map(|t| (*t).to_string()).collect(),
        }
    }

    /// Titles of the collected cards, tagged by storage class, so a chip
    /// test asserts on what actually renders rather than on a count.
    fn titles(items: &[NotebookListItem]) -> Vec<(&'static str, String)> {
        items
            .iter()
            .map(|i| match i {
                NotebookListItem::Local { title, .. } => ("local", title.clone()),
                NotebookListItem::Mutable { title, .. } => ("account", title.clone()),
                NotebookListItem::Public { title, .. } => ("public", title.clone()),
            })
            .collect()
    }

    /// The chips sort by storage class; the badges inside a card report
    /// audience. The Account group holds both halves at once, so its chip
    /// cannot wear either badge without contradicting the other half of its
    /// own contents (it wore the published globe over a stack of locks).
    #[test]
    fn account_chip_wears_a_storage_class_icon_not_a_card_badge() {
        assert_ne!(
            ACCOUNT_CHIP_ICON,
            account_badge(true).1,
            "the Account chip must not be the published badge"
        );
        assert_ne!(
            ACCOUNT_CHIP_ICON,
            account_badge(false).1,
            "the Account chip must not be the unpublished badge"
        );
        // And it names its own class rather than borrowing a neighbour's.
        assert_ne!(ACCOUNT_CHIP_ICON, LOCAL_CHIP_ICON);
        assert_ne!(ACCOUNT_CHIP_ICON, PUBLIC_CHIP_ICON);

        // Published and unpublished cards stay distinguishable, in shape and
        // in class: the colour half of that distinction is CSS-only, and a
        // badge whose only difference is a class name reads identical if the
        // rule is ever dropped (PRD-0062 shipped exactly that bug).
        assert_ne!(account_badge(true).1, account_badge(false).1);
        assert_ne!(account_badge(true).0, account_badge(false).0);
        assert!(account_badge(false).0.contains("unpublished"));
    }

    #[test]
    fn local_search_matches_title_and_tags() {
        let mut nb = IronpadNotebook::new("My Physics Scratchpad");
        nb.tags = Some(vec!["blog".into(), "wip".into()]);

        assert!(local_notebook_matches(&nb, ""));
        assert!(local_notebook_matches(&nb, "physics"));
        assert!(local_notebook_matches(&nb, "blog"));
        assert!(!local_notebook_matches(&nb, "quaternions"));

        nb.tags = None;
        assert!(local_notebook_matches(&nb, "scratchpad"));
        assert!(!local_notebook_matches(&nb, "blog"));
    }

    /// An unpublished account notebook is only findable from this listing,
    /// so search must reach it on exactly the fields a local one exposes.
    #[test]
    fn account_search_matches_title_and_tags_published_or_not() {
        let draft = account("Quantum Draft", false, &["physics", "wip"]);
        let live = account("Quantum Published", true, &["physics"]);

        for nb in [&draft, &live] {
            assert!(account_notebook_matches(nb, ""));
            assert!(account_notebook_matches(nb, "quantum"));
            assert!(account_notebook_matches(nb, "physics"));
            assert!(!account_notebook_matches(nb, "quaternions"));
        }
        assert!(account_notebook_matches(&draft, "wip"));
        assert!(!account_notebook_matches(&live, "wip"));
    }

    #[test]
    fn account_chip_lists_unpublished_alongside_published() {
        let account_notebooks = [
            account("Draft Only", false, &[]),
            account("Live", true, &[]),
        ];

        let items = collect_items(&[], &account_notebooks, &[], "", FilterMode::Account);
        assert_eq!(
            titles(&items),
            vec![
                ("account", "Draft Only".to_string()),
                ("account", "Live".to_string()),
            ]
        );
        // The published flag reaches the card, which is what decides the
        // badge; listing is not conditional on it.
        assert!(matches!(
            items[0],
            NotebookListItem::Mutable {
                published: false,
                ..
            }
        ));
        assert!(matches!(
            items[1],
            NotebookListItem::Mutable {
                published: true,
                ..
            }
        ));
    }

    #[test]
    fn chips_filter_to_one_storage_class_each() {
        let local = [IronpadNotebook::new("Local Notebook")];
        let account_notebooks = [account("Account Notebook", false, &[])];
        let public = [summary()];

        let all = collect_items(&local, &account_notebooks, &public, "", FilterMode::All);
        assert_eq!(
            titles(&all)
                .into_iter()
                .map(|(class, _)| class)
                .collect::<Vec<_>>(),
            vec!["local", "account", "public"]
        );

        for (mode, class) in [
            (FilterMode::Local, "local"),
            (FilterMode::Account, "account"),
            (FilterMode::Public, "public"),
        ] {
            let items = collect_items(&local, &account_notebooks, &public, "", mode);
            assert_eq!(items.len(), 1, "one card per storage class in this fixture");
            assert_eq!(titles(&items)[0].0, class);
        }
    }

    /// Search spans every storage class at once, so a tag query that hits an
    /// account notebook must not be filtered out by the class it lives in.
    #[test]
    fn search_spans_storage_classes() {
        let mut nb = IronpadNotebook::new("Local Notes");
        nb.tags = Some(vec!["blog".into()]);
        let local = [nb];
        let account_notebooks = [account("Account Notes", false, &["blog"])];
        let public = [summary()];

        let items = collect_items(&local, &account_notebooks, &public, "blog", FilterMode::All);
        assert_eq!(
            titles(&items)
                .into_iter()
                .map(|(class, _)| class)
                .collect::<Vec<_>>(),
            vec!["local", "account", "public"]
        );

        let none = collect_items(
            &local,
            &account_notebooks,
            &public,
            "quaternions",
            FilterMode::All,
        );
        assert!(none.is_empty());
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
