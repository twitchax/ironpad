use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use ironpad_common::{IronpadNotebook, ShareManifest};

use crate::components::app_layout::LayoutContext;
use crate::components::social_meta::{mark_not_found, SocialMeta};
use crate::components::view_only_notebook::ViewOnlyNotebook;
use crate::server_fns::{get_mutable_manifest, get_mutable_notebook};

/// Route component for `/mutable/{id}` (PRD-0049).
///
/// Fetches a mutable share's current notebook from the server and renders it
/// read-only via [`ViewOnlyNotebook`], plus an "enter your key" rebind control
/// so the author (on any device) can pull the share into local storage and
/// edit it. The reader page itself never edits: capability-based rendering is
/// a deliberate non-goal.
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
            let notebook = get_mutable_notebook(id.clone()).await?;
            // Manifest only matters when the notebook exists; a
            // missing/degraded manifest falls back to live compilation.
            let manifest = if notebook.is_some() {
                get_mutable_manifest(id).await.unwrap_or(None)
            } else {
                None
            };
            Ok::<_, ServerFnError>((notebook, manifest))
        },
    );

    Effect::new(move || {
        if let Some(Ok((Some(nb), _))) = notebook_resource.get() {
            ctx.cell_count.set(nb.cells.len());
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
                        Ok((Some(notebook), manifest)) => view! {
                            // `noindex`: like immutable shares, a mutable
                            // link is unlisted, so it unfurls without being
                            // indexed.
                            <SocialMeta
                                title=notebook.title.clone()
                                description=notebook.description.clone()
                                path=format!("/mutable/{id}")
                                image=notebook
                                    .og_image_path()
                                    .map_or_else(
                                        || format!("/og/mutable/{id}.png"),
                                        str::to_string,
                                    )
                                noindex=true
                            />
                            <MutableReader id notebook manifest />
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

/// The resolved reader: rebind bar plus the read-only notebook.
#[component]
fn MutableReader(
    id: String,
    notebook: IronpadNotebook,
    manifest: Option<ShareManifest>,
) -> impl IntoView {
    let rebind_open = RwSignal::new(false);
    let key_input = RwSignal::new(String::new());
    let rebind_status = RwSignal::new(Option::<String>::None);
    let busy = RwSignal::new(false);

    let notebook_stored = StoredValue::new(notebook.clone());
    let share_id = StoredValue::new(id);

    #[cfg(feature = "hydrate")]
    let navigate = StoredValue::new_local(leptos_router::hooks::use_navigate());

    let submit_key = move |_| {
        #[cfg(feature = "hydrate")]
        {
            if busy.get_untracked() {
                return;
            }
            let key = key_input.get_untracked().trim().to_string();
            if key.is_empty() {
                rebind_status.set(Some("Enter your edit key.".to_string()));
                return;
            }
            busy.set(true);
            rebind_status.set(None);
            let navigate = navigate.get_value();
            leptos::task::spawn_local(async move {
                let sid = share_id.get_value();
                let nb = notebook_stored.get_value();
                let local_uuid = nb.id.to_string();

                // Already bound on this device? Just open the editor.
                if crate::storage::client::get_mutable(&local_uuid)
                    .await
                    .is_some()
                {
                    navigate(
                        &format!("/local/{local_uuid}"),
                        leptos_router::NavigateOptions::default(),
                    );
                    return;
                }

                match crate::server_fns::verify_mutable_key(sid.clone(), key.clone()).await {
                    Ok(true) => {
                        // Store the entered key as this share's edit key on
                        // this device; either key authorizes, so whatever the
                        // user pasted is a valid pushing credential.
                        if let Err(e) = crate::storage::client::save_mutable(&nb, &sid, &key).await
                        {
                            busy.set(false);
                            rebind_status.set(Some(format!("Could not save locally: {e:?}")));
                            return;
                        }
                        navigate(
                            &format!("/local/{local_uuid}"),
                            leptos_router::NavigateOptions::default(),
                        );
                    }
                    Ok(false) => {
                        busy.set(false);
                        rebind_status
                            .set(Some("That key does not match this notebook.".to_string()));
                    }
                    Err(e) => {
                        busy.set(false);
                        rebind_status.set(Some(format!("Could not verify: {e}")));
                    }
                }
            });
        }
    };

    view! {
        <div class="mutable-rebind">
            {move || if rebind_open.get() {
                view! {
                    <div class="mutable-rebind-form">
                        <input
                            class="mutable-rebind-input"
                            r#type="text"
                            placeholder="Paste your edit key"
                            prop:value=move || key_input.get()
                            on:input=move |ev| key_input.set(event_target_value(&ev))
                        />
                        <button
                            class="mutable-rebind-submit"
                            on:click=submit_key
                            disabled=move || busy.get()
                        >
                            {move || if busy.get() { "Checking…" } else { "Edit" }}
                        </button>
                        {move || rebind_status.get().map(|msg| view! {
                            <span class="mutable-rebind-status">{msg}</span>
                        })}
                    </div>
                }.into_any()
            } else {
                view! {
                    <button
                        class="mutable-rebind-toggle"
                        on:click=move |_| rebind_open.set(true)
                    >
                        "✎ Have an edit key?"
                    </button>
                }.into_any()
            }}
        </div>
        <ViewOnlyNotebook
            notebook
            fork_label="Fork to Private".to_string()
            share_manifest=manifest
        />
    }
}
