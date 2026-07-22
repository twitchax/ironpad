use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::components::app_layout::LayoutContext;
use crate::components::view_only_notebook::ViewOnlyNotebook;
use crate::server_fns::get_public_notebook;

/// Route component for `/public/{name}` (extension-less, PRD-0048).
///
/// Fetches a public notebook from the server's static files and renders it
/// in view-only mode via [`ViewOnlyNotebook`].
#[component]
pub fn PublicNotebookPage() -> impl IntoView {
    let params = use_params_map();
    let filename = params.read_untracked().get("filename").unwrap_or_default();

    // Reset layout context for public notebook. The notebook's real title is
    // shown once in the view-only toolbar (<h1>), so clear the header center to
    // avoid a duplicate (and to avoid the header showing the raw filename).
    let ctx = expect_context::<LayoutContext>();
    ctx.notebook_title.set(None);

    // Spec handed to ViewOnlyNotebook so its Embed button can build snippets.
    let embed_spec = params
        .read_untracked()
        .get("filename")
        .map(|f| format!("public/{f}"));

    let notebook_resource = Resource::new(move || filename.clone(), get_public_notebook);

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
                let embed_spec = embed_spec.clone();
                Suspend::new(async move {
                    match notebook_resource.await {
                        Ok(notebook) => view! {
                            <ViewOnlyNotebook
                                notebook
                                fork_label="Fork to Private".to_string()
                                embed_spec=embed_spec.unwrap_or_default()
                                // First-party showcase content auto-runs
                                // (PRD-0040); shared notebooks never do.
                                autorun=true
                            />
                        }
                        .into_any(),

                        Err(e) => view! {
                            <div class="ironpad-error-boundary">
                                <div class="ironpad-error-boundary-icon">"△"</div>
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
