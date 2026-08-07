use crate::components::icon::Icon;
use crate::components::icons;
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::components::app_layout::LayoutContext;
use crate::components::social_meta::{mark_not_found, SocialMeta};
use crate::components::view_only_notebook::ViewOnlyNotebook;
use crate::server_fns::get_public_notebook;

/// Route component for `/public/{name}` (extension-less, PRD-0048).
///
/// Fetches a public notebook from the server's static files and renders it
/// in view-only mode via [`ViewOnlyNotebook`].
#[component]
pub fn PublicNotebookPage() -> impl IntoView {
    let params = use_params_map();
    // Tracked: leptos_router reuses this outlet when only the param changes
    // (e.g. a markdown cross-link from /public/cannon to /public/autodiff), so
    // a one-shot untracked read would keep rendering the old notebook forever.
    let filename = Memo::new(move |_| params.read().get("filename").unwrap_or_default());

    // Reset layout context for public notebook. The notebook's real title is
    // shown once in the view-only toolbar (<h1>), so clear the header center to
    // avoid a duplicate (and to avoid the header showing the raw filename).
    let ctx = expect_context::<LayoutContext>();
    ctx.notebook_title.set(None);

    let notebook_resource = Resource::new(move || filename.get(), get_public_notebook);

    // Update footer cell count when the resource resolves (Effect runs on the
    // client, avoiding the SSR Suspense-boundary signal propagation issue).
    Effect::new(move || {
        if let Some(Ok(nb)) = notebook_resource.get() {
            ctx.cell_count.set(nb.cells.len());
        }
    });

    view! {
        <Suspense fallback=move || {
            view! {
                <div class="ironpad-loading">
                    <p>"Loading public notebook..."</p>
                </div>
            }
        }>
            {move || {
                let filename = filename.get();
                // Spec handed to ViewOnlyNotebook so its Embed button can
                // build snippets.
                let embed_spec = (!filename.is_empty()).then(|| format!("public/{filename}"));
                // Canonical form for `og:url` and the card path:
                // extension-less (PRD-0048), even when the route was reached
                // via a legacy `.ironpad` link.
                let meta_name = filename
                    .strip_suffix(".ironpad")
                    .unwrap_or(&filename)
                    .to_string();
                Suspend::new(async move {
                    match notebook_resource.await {
                        Ok(notebook) => view! {
                            <SocialMeta
                                title=notebook.title.clone()
                                description=notebook.description.clone()
                                path=format!("/public/{}", meta_name)
                                image=notebook
                                    .og_image_path()
                                    .map_or_else(
                                        || format!("/og/public/{meta_name}.png"),
                                        str::to_string,
                                    )
                                image_size=notebook.og_image_dimensions()
                                oembed=true
                            />
                            <ViewOnlyNotebook
                                notebook
                                embed_spec=embed_spec.unwrap_or_default()
                                // First-party showcase content auto-runs
                                // (PRD-0040); shared notebooks never do.
                                autorun=true
                            />
                        }
                        .into_any(),

                        Err(e) => view! {
                            {mark_not_found()}
                            <div class="ironpad-error-boundary">
                                <div class="ironpad-error-boundary-icon"><Icon icon=icons::WARNING/></div>
                                <p class="ironpad-error-boundary-message">
                                    {format!("Failed to load public notebook: {e}")}
                                </p>
                            </div>
                        }
                        .into_any(),
                    }
                })
            }}
        </Suspense>
    }
}
