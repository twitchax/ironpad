use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::components::app_layout::LayoutContext;
use crate::components::view_only_notebook::ViewOnlyNotebook;
use crate::server_fns::get_shared_notebook;

/// Route component for `/shared/{hash}`.
///
/// Fetches a previously shared notebook from the server's share cache
/// and renders it in view-only mode via [`ViewOnlyNotebook`].
#[component]
pub fn SharedNotebookPage() -> impl IntoView {
    let params = use_params_map();
    let hash = params.read_untracked().get("hash").unwrap_or_default();

    // Reset layout context for shared notebook.
    let ctx = expect_context::<LayoutContext>();
    ctx.notebook_title.set(Some("Shared Notebook".to_string()));
    ctx.show_save_button.set(false);

    let notebook_resource = Resource::new(move || hash.clone(), get_shared_notebook);

    view! {
        <Suspense fallback=move || {
            view! {
                <div class="ironpad-loading">
                    <p>"Loading shared notebook..."</p>
                </div>
            }
        }>
            {move || Suspend::new(async move {
                match notebook_resource.await {
                    Ok(notebook) => {
                        // Update title with the actual notebook title.
                        ctx.notebook_title.set(Some(format!("🔗 {}", notebook.title)));

                        view! {
                            <ViewOnlyNotebook notebook fork_label="Fork to Private".to_string() />
                        }.into_any()
                    }

                    Err(e) => view! {
                        <div class="ironpad-error-boundary">
                            <div class="ironpad-error-boundary-icon">"⚠"</div>
                            <p class="ironpad-error-boundary-message">
                                {format!("Shared notebook not found or expired: {e}")}
                            </p>
                            <p class="ironpad-error-boundary-hint">
                                "The share link may have expired, or the notebook may have been removed."
                            </p>
                        </div>
                    }.into_any(),
                }
            })}
        </Suspense>
    }
}
