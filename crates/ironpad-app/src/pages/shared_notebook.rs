use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::components::app_layout::LayoutContext;
use crate::components::social_meta::{mark_not_found, SocialMeta};
use crate::components::view_only_notebook::ViewOnlyNotebook;
use crate::server_fns::{get_shared_manifest, get_shared_notebook};

/// Route component for `/shared/{hash}`.
///
/// Fetches a previously shared notebook from the server's share cache
/// and renders it in view-only mode via [`ViewOnlyNotebook`].
#[component]
pub fn SharedNotebookPage() -> impl IntoView {
    let params = use_params_map();
    let hash = params.read_untracked().get("hash").unwrap_or_default();
    // Spec handed to ViewOnlyNotebook so its Embed button can build snippets.
    let embed_spec = (!hash.is_empty()).then(|| format!("shared/{hash}"));
    let meta_hash = hash.clone();

    // Reset layout context for shared notebook.
    let ctx = expect_context::<LayoutContext>();
    // Real title is shown once in the view-only toolbar (<h1>); keep the header
    // center clear to avoid a duplicate.
    ctx.notebook_title.set(None);

    let notebook_resource = Resource::new(
        move || hash.clone(),
        |hash| async move {
            let notebook = get_shared_notebook(hash.clone()).await?;
            // A missing/failed manifest degrades to live compilation
            // (PRD-0047); it never fails the page.
            let manifest = get_shared_manifest(hash).await.unwrap_or(None);
            Ok::<_, ServerFnError>((notebook, manifest))
        },
    );

    // Update footer cell count when the resource resolves.
    Effect::new(move || {
        if let Some(Ok((nb, _))) = notebook_resource.get() {
            ctx.cell_count.set(nb.cells.len());
        }
    });

    view! {
        <Suspense fallback=move || {
            view! {
                <div class="ironpad-loading">
                    <p>"Loading shared notebook..."</p>
                </div>
            }
        }>
            {move || {
                let embed_spec = embed_spec.clone();
                let meta_hash = meta_hash.clone();
                Suspend::new(async move {
                match notebook_resource.await {
                    Ok((notebook, share_manifest)) => {
                        view! {
                            // `noindex`: a share link is unlisted by
                            // construction, so it gets a preview card without
                            // landing in a search index.
                            <SocialMeta
                                title=notebook.title.clone()
                                description=notebook.description.clone()
                                path=format!("/shared/{meta_hash}")
                                image=notebook
                                    .og_image_path()
                                    .map_or_else(
                                        || format!("/og/shared/{meta_hash}.png"),
                                        str::to_string,
                                    )
                                noindex=true
                            />
                            <ViewOnlyNotebook
                                notebook
                                fork_label="Fork to Private".to_string()
                                embed_spec=embed_spec.unwrap_or_default()
                                share_manifest=share_manifest
                            />
                        }.into_any()
                    }

                    Err(e) => view! {
                        {mark_not_found()}
                        <div class="ironpad-error-boundary">
                            <div class="ironpad-error-boundary-icon">"△"</div>
                            <p class="ironpad-error-boundary-message">
                                {format!("Shared notebook not found or expired: {e}")}
                            </p>
                            <p class="ironpad-error-boundary-hint">
                                "The share link may have expired, or the notebook may have been removed."
                            </p>
                        </div>
                    }.into_any(),
                }
            })
            }}
        </Suspense>
    }
}
