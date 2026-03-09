use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::components::app_layout::LayoutContext;
use crate::components::view_only_notebook::ViewOnlyNotebook;
use crate::server_fns::get_public_notebook;

/// Route component for `/notebook/public/{filename}`.
///
/// Fetches a public notebook from the server's static files and renders it
/// in view-only mode via [`ViewOnlyNotebook`].
#[component]
pub fn PublicNotebookPage() -> impl IntoView {
    let params = use_params_map();
    let filename = params.read_untracked().get("filename").unwrap_or_default();

    // Reset layout context for public notebook.
    let ctx = expect_context::<LayoutContext>();
    ctx.notebook_title
        .set(Some(format!("📓 {}", filename.replace(".ironpad", ""))));
    ctx.show_save_button.set(false);

    let notebook_resource = Resource::new(move || filename.clone(), get_public_notebook);

    view! {
        <Suspense fallback=move || {
            view! {
                <div class="ironpad-loading">
                    <p>"Loading public notebook..."</p>
                </div>
            }
        }>
            {move || {
                Suspend::new(async move {
                    match notebook_resource.await {
                        Ok(notebook) => view! {
                            <ViewOnlyNotebook notebook fork_label="Fork to Private".to_string()/>
                        }
                        .into_any(),

                        Err(e) => view! {
                            <div class="ironpad-error-boundary">
                                <div class="ironpad-error-boundary-icon">"⚠"</div>
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
