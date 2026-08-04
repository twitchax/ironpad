use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use ironpad_common::{MutableNotebookResponse, ShareManifest};

use crate::components::app_layout::LayoutContext;
use crate::components::social_meta::{mark_not_found, SocialMeta};
use crate::components::view_only_notebook::ViewOnlyNotebook;
use crate::pages::notebook_editor::NotebookEditor;
use crate::server_fns::{get_mutable_manifest, get_mutable_notebook};

/// Route component for `/mutable/{id}` (PRD-0054): ONE address per published
/// notebook. Everyone gets the view-only reader of the published copy; the
/// owner is swapped into the real editor over the server-side draft on
/// hydrate. `?view=reader` forces the reader (the owner's "View as Reader").
///
/// SSR always renders the reader from published — crawlers, unfurlers, and
/// readers share that path, and draft content must never leak into it.
#[component]
pub fn MutableNotebookPage() -> impl IntoView {
    let params = use_params_map();
    let id = params.read_untracked().get("id").unwrap_or_default();

    let ctx = expect_context::<LayoutContext>();
    // Real title shows once in the view-only toolbar; keep the header center clear.
    ctx.notebook_title.set(None);

    // `?view=reader` pins the reader even for the owner.
    let query = leptos_router::hooks::use_query_map();
    let force_reader = query
        .read_untracked()
        .get("view")
        .is_some_and(|v| v == "reader");

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

    // Owner detection (PRD-0054): resolved on hydrate so SSR stays the
    // reader. A non-owner or anonymous caller errs, which simply leaves the
    // reader in place.
    let owner_edit: RwSignal<Option<ironpad_common::MutableEditResponse>> = RwSignal::new(None);

    #[cfg(feature = "hydrate")]
    if !force_reader {
        let edit_id = id.clone();
        Effect::new(move || {
            let edit_id = edit_id.clone();
            leptos::task::spawn_local(async move {
                if let Ok(Some(edit)) = crate::server_fns::get_mutable_for_edit(edit_id).await {
                    let _ = owner_edit.try_set(Some(edit));
                }
            });
        });
    }

    let share_id_for_editor = id.clone();
    view! {
        {move || {
            if let Some(edit) = owner_edit.get() {
                // The owner's editor over the server draft. Mounting the
                // editor replaces the reader wholesale; the session-cleanup
                // and persistence seams live inside NotebookEditor.
                return view! {
                    <NotebookEditor
                        notebook_id=edit.notebook.id.to_string()
                        server_draft=Some((
                            share_id_for_editor.clone(),
                            edit.notebook.clone(),
                            edit.dirty,
                        ))
                    />
                }
                .into_any();
            }
            let id = share_id_for_editor.clone();
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
                                    <MutableReader id response manifest force_reader />
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
            .into_any()
        }}
    }
}

#[component]
fn MutableReader(
    id: String,
    response: MutableNotebookResponse,
    manifest: Option<ShareManifest>,
    /// True when the owner explicitly asked for the reader (`?view=reader`);
    /// gives them a way back into the editor.
    force_reader: bool,
) -> impl IntoView {
    let MutableNotebookResponse {
        notebook,
        owner,
        is_owner,
    } = response;

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

    // The owner in forced-reader mode gets a way back to the editor; on the
    // normal path the auto-swap already handled it and this never renders.
    let edit_back = (is_owner && force_reader).then(|| {
        view! {
            <a class="view-only-edit-button" href=format!("/mutable/{id}") rel="external">
                "✎ Edit"
            </a>
        }
        .into_any()
    });

    view! {
        {attribution}
        <ViewOnlyNotebook
            notebook
            fork_label="Fork to Private".to_string()
            share_manifest=manifest
            controls=edit_back
        />
    }
}
