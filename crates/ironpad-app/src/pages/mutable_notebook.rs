use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use ironpad_common::{MutableNotebookResponse, ShareManifest};

use crate::components::app_layout::LayoutContext;
use crate::components::social_meta::{mark_not_found, SocialMeta};
use crate::components::view_only_notebook::ViewOnlyNotebook;
use crate::server_fns::{get_mutable_manifest, get_mutable_notebook};

/// Route component for `/mutable/{id}` (PRD-0049, accounts-backed PRD-0053).
///
/// Fetches a mutable share's current notebook (plus owner attribution and
/// whether the caller's session owns it) and renders it read-only via
/// [`ViewOnlyNotebook`]. The owner gets an Edit path on any device they're
/// signed in on: bound devices open the local working copy, fresh devices
/// clone the published copy into one. The reader page itself never edits.
#[component]
pub fn MutableNotebookPage() -> impl IntoView {
    let params = use_params_map();
    let id = params.read_untracked().get("id").unwrap_or_default();

    let ctx = expect_context::<LayoutContext>();
    // Real title shows once in the view-only toolbar; keep the header center clear.
    ctx.notebook_title.set(None);

    let notebook_resource = Resource::new(
        {
            let id = id.clone();
            move || id.clone()
        },
        |id| async move {
            let response = get_mutable_notebook(id.clone()).await?;
            // Manifest only matters when the notebook exists; a
            // missing/degraded manifest falls back to live compilation.
            let manifest = if response.is_some() {
                get_mutable_manifest(id).await.unwrap_or(None)
            } else {
                None
            };
            Ok::<_, ServerFnError>((response, manifest))
        },
    );

    Effect::new(move || {
        if let Some(Ok((Some(r), _))) = notebook_resource.get() {
            ctx.cell_count.set(r.notebook.cells.len());
        }
    });

    view! {
        <Suspense fallback=move || {
            view! {
                <div class="ironpad-loading">
                    <p>"Loading notebook..."</p>
                </div>
            }
        }>
            {move || {
                let id = id.clone();
                Suspend::new(async move {
                    match notebook_resource.await {
                        Ok((Some(response), manifest)) => view! {
                            // `noindex`: like immutable shares, a mutable
                            // link is unlisted, so it unfurls without being
                            // indexed.
                            <SocialMeta
                                title=response.notebook.title.clone()
                                description=response.notebook.description.clone()
                                path=format!("/mutable/{id}")
                                image=response.notebook
                                    .og_image_path()
                                    .map_or_else(
                                        || format!("/og/mutable/{id}.png"),
                                        str::to_string,
                                    )
                                image_size=response.notebook.og_image_dimensions()
                                noindex=true
                            />
                            <MutableReader id response manifest />
                        }.into_any(),
                        Ok((None, _)) => view! {
                            {mark_not_found()}
                            <div class="ironpad-error-boundary">
                                <div class="ironpad-error-boundary-icon">"△"</div>
                                <p class="ironpad-error-boundary-message">
                                    "This mutable notebook was not found."
                                </p>
                                <p class="ironpad-error-boundary-hint">
                                    "The link may be wrong, or the author may have unpublished it."
                                </p>
                            </div>
                        }.into_any(),
                        Err(e) => view! {
                            <div class="ironpad-error-boundary">
                                <div class="ironpad-error-boundary-icon">"△"</div>
                                <p class="ironpad-error-boundary-message">
                                    {format!("Could not load notebook: {e}")}
                                </p>
                            </div>
                        }.into_any(),
                    }
                })
            }}
        </Suspense>
    }
}

/// The resolved reader: attribution, owner controls, and the read-only
/// notebook.
#[component]
fn MutableReader(
    id: String,
    response: MutableNotebookResponse,
    manifest: Option<ShareManifest>,
) -> impl IntoView {
    let MutableNotebookResponse {
        notebook,
        owner,
        is_owner,
    } = response;

    let busy = RwSignal::new(false);

    // `Some(local_uuid)` when this device holds the working copy (a record
    // for the notebook's uuid exists in the local mutable store). Resolved on
    // hydrate; until then owner controls render the clone variant.
    let binding: RwSignal<Option<String>> = RwSignal::new(None);
    // The local working copy differs from the published copy. Symmetric on
    // purpose: content inequality cannot tell unpushed local edits from a
    // push made on another device, so the banner names both remedies.
    let diverged = RwSignal::new(false);

    let notebook_stored = StoredValue::new(notebook.clone());
    let share_id = StoredValue::new(id);

    #[cfg(feature = "hydrate")]
    let navigate = StoredValue::new_local(leptos_router::hooks::use_navigate());

    // Resolved under the page's owner; `Toaster` is Copy and app-root-owned,
    // so moving it into the async continuation below is disposal-safe.
    #[cfg(feature = "hydrate")]
    let toaster = crate::components::toaster::Toaster::expect_context();

    #[cfg(feature = "hydrate")]
    Effect::new(move || {
        // Capture under the page's owner; the async block outlives it and
        // must not touch reactive context (only try_ writes).
        let nb = notebook_stored.get_value();
        leptos::task::spawn_local(async move {
            let local_uuid = nb.id.to_string();
            if let Some(record) = crate::storage::client::get_mutable(&local_uuid).await {
                let _ = diverged.try_set(!record.notebook.content_matches(&nb));
                let _ = binding.try_set(Some(local_uuid));
            }
        });
    });

    // Owner on a device with no working copy: clone the published copy into
    // the local mutable store and open the editor (the PRD-0053 replacement
    // for the key-based rebind flow — signing in IS the authorization).
    let clone_and_edit = move || {
        #[cfg(feature = "hydrate")]
        {
            if busy.get_untracked() {
                return;
            }
            busy.set(true);
            let navigate = navigate.get_value();
            leptos::task::spawn_local(async move {
                let sid = share_id.get_value();
                let nb = notebook_stored.get_value();
                let local_uuid = nb.id.to_string();

                // The binding check is async and a fast click can beat it:
                // never clobber an existing working copy, just open it.
                if crate::storage::client::get_mutable(&local_uuid)
                    .await
                    .is_none()
                {
                    if let Err(e) = crate::storage::client::save_mutable(&nb, &sid).await {
                        busy.set(false);
                        toaster.toast(
                            crate::components::toaster::ToastIntent::Error,
                            "Could Not Clone",
                            format!("Saving the working copy failed: {e:?}"),
                            6,
                        );
                        return;
                    }
                    toaster.toast(
                        crate::components::toaster::ToastIntent::Success,
                        "Ready to Edit",
                        "The published copy is now your working copy on this device.",
                        4,
                    );
                }
                navigate(
                    &format!("/local/{local_uuid}"),
                    leptos_router::NavigateOptions::default(),
                );
            });
        }
    };

    let attribution = owner.map(|o| {
        let avatar = (!o.avatar_url.is_empty()).then(|| {
            view! { <img class="mutable-attribution-avatar" src=o.avatar_url.clone() alt="" /> }
        });
        let profile = format!("https://github.com/{}", o.login);
        view! {
            <div class="mutable-attribution">
                {avatar}
                <span class="mutable-attribution-text">
                    "Published by "
                    <a class="mutable-attribution-link" href=profile target="_blank" rel="noopener">
                        {format!("@{}", o.login)}
                    </a>
                </span>
            </div>
        }
    });

    view! {
        {attribution}
        // Author-only divergence notice (implies a binding, so the editor
        // link inside can assume one). Readers on other devices never see it.
        {move || diverged.get().then(|| {
            let editor_href = binding.get().map(|uuid| format!("/local/{uuid}"));
            view! {
                <div class="mutable-author-banner">
                    <span class="mutable-author-banner-text">
                        "This is the published copy. Your local working copy \
                         differs from it; push or pull from the editor to \
                         reconcile."
                    </span>
                    {editor_href.map(|href| view! {
                        <a class="mutable-author-banner-link" href=href>
                            "✎ Open editor"
                        </a>
                    })}
                </div>
            }
        })}
        <ViewOnlyNotebook
            notebook
            fork_label="Fork to Private".to_string()
            share_manifest=manifest
            // Owners get a first-class Edit control: a link into the local
            // working copy when one exists, a clone-and-edit button on a
            // fresh device. Reactive inside the slots because the binding
            // resolves asynchronously; non-owners render nothing.
            controls=Some(view! {
                {move || is_owner.then(|| binding.get().map_or_else(
                    || view! {
                        <button
                            class="view-only-edit-button"
                            on:click=move |_| clone_and_edit()
                            disabled=move || busy.get()
                        >
                            {move || if busy.get() { "Preparing…" } else { "✎ Edit" }}
                        </button>
                    }.into_any(),
                    |uuid| view! {
                        <a
                            class="view-only-edit-button"
                            href=format!("/local/{uuid}")
                        >
                            "✎ Edit"
                        </a>
                    }.into_any(),
                ))}
            }.into_any())
            menu=Some(view! {
                {move || is_owner.then(|| binding.get().map_or_else(
                    || view! {
                        <button
                            class="ironpad-toolbar-dropdown-item mutable-edit-menu-item"
                            on:click=move |_| clone_and_edit()
                        >
                            "✎ Edit on this device"
                        </button>
                    }.into_any(),
                    |uuid| view! {
                        <a
                            class="ironpad-toolbar-dropdown-item mutable-edit-menu-item"
                            href=format!("/local/{uuid}")
                        >
                            "✎ Open in editor"
                        </a>
                    }.into_any(),
                ))}
            }.into_any())
        />
    }
}
