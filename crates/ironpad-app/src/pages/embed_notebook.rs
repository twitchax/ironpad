//! Chrome-less embed routes for shared and public notebooks (PRD-0039).
//!
//! These render [`ViewOnlyNotebook`] in embed mode inside the normal app shell
//! (Monaco/executor load globally, so cells stay runnable); [`AppLayout`]
//! suppresses the header and status bar for `/embed/` paths. The host page's
//! `embed.js` sizes the iframe via the height messages `embed-frame.js` posts.
//!
//! [`AppLayout`]: crate::components::app_layout::AppLayout

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use ironpad_common::IronpadNotebook;

use crate::components::view_only_notebook::ViewOnlyNotebook;
use crate::server_fns::{
    get_mutable_manifest, get_mutable_notebook, get_public_notebook, get_shared_manifest,
    get_shared_notebook,
};

/// Route component for `/embed/shared/{hash}` — the iframe-embeddable variant
/// of [`SharedNotebookPage`](super::SharedNotebookPage).
#[component]
pub fn EmbedSharedPage() -> impl IntoView {
    let params = use_params_map();
    // Tracked: the router reuses this outlet on a param-only change.
    let hash = Memo::new(move |_| params.read().get("hash").unwrap_or_default());

    let notebook_resource = Resource::new(
        move || hash.get(),
        |hash| async move {
            let notebook = get_shared_notebook(hash.clone()).await?;
            // A missing/failed manifest degrades to live compilation
            // (PRD-0047); it never fails the embed.
            let manifest = get_shared_manifest(hash).await.unwrap_or(None);
            Ok::<_, ServerFnError>((notebook, manifest))
        },
    );

    view! {
        <Suspense fallback=embed_loading>
            {move || {
                let spec = format!("shared/{}", hash.get());
                Suspend::new(async move {
                    let (result, manifest) = match notebook_resource.await {
                        Ok((nb, manifest)) => (Ok(nb), manifest),
                        Err(e) => (Err(e), None),
                    };
                    // Shared content is arbitrary user code: never auto-run.
                    embed_body(
                        result,
                        manifest,
                        spec,
                        false,
                        "Shared notebook not found or expired",
                    )
                })
            }}
        </Suspense>
    }
}

/// Route component for `/embed/public/{filename}` — the iframe-embeddable
/// variant of [`PublicNotebookPage`](super::PublicNotebookPage).
///
/// Unlike the full public page (which always auto-runs), an embed auto-runs
/// only when the EMBEDDER opts in via `?autorun=1` (forwarded by embed.js from
/// `data-autorun`): readers of a third-party page didn't choose ironpad, so
/// surprise CPU is the embedder's call. Shared embeds never auto-run at all —
/// [`EmbedSharedPage`] doesn't read the param (PRD-0040).
#[component]
pub fn EmbedPublicPage() -> impl IntoView {
    let params = use_params_map();
    // Tracked: the router reuses this outlet on a param-only change.
    let filename = Memo::new(move |_| params.read().get("filename").unwrap_or_default());

    let query = leptos_router::hooks::use_query_map();
    let autorun = matches!(
        query.read_untracked().get("autorun").as_deref(),
        Some("1" | "true")
    );

    let notebook_resource = Resource::new(move || filename.get(), get_public_notebook);

    view! {
        <Suspense fallback=embed_loading>
            {move || {
                let spec = format!("public/{}", filename.get());
                Suspend::new(async move {
                    embed_body(
                        notebook_resource.await,
                        None,
                        spec,
                        autorun,
                        "Public notebook not found",
                    )
                })
            }}
        </Suspense>
    }
}

/// Route component for `/embed/mutable/{id}` — the iframe-embeddable variant
/// of the `/mutable/{id}` reader (PRD-0057). Resolves the PUBLISHED copy
/// (drafts never reach an embed) with live semantics: a Push updates every
/// consumer's iframe on its next load.
#[component]
pub fn EmbedMutablePage() -> impl IntoView {
    let params = use_params_map();
    // Tracked: the router reuses this outlet on a param-only change.
    let id = Memo::new(move |_| params.read().get("id").unwrap_or_default());

    let notebook_resource = Resource::new(
        move || id.get(),
        |id| async move {
            let response = get_mutable_notebook(id.clone()).await?;
            let manifest = if response.is_some() {
                get_mutable_manifest(id).await.unwrap_or(None)
            } else {
                None
            };
            Ok::<_, ServerFnError>((response, manifest))
        },
    );

    view! {
        <Suspense fallback=embed_loading>
            {move || {
                let spec = format!("mutable/{}", id.get());
                Suspend::new(async move {
                    let (result, manifest) = match notebook_resource.await {
                        Ok((Some(response), manifest)) => (Ok(response.notebook), manifest),
                        Ok((None, _)) => (
                            Err(ServerFnError::new("not found")),
                            None,
                        ),
                        Err(e) => (Err(e), None),
                    };
                    // A published notebook is one author's content, not
                    // first-party showcase: never auto-run (PRD-0040 trust
                    // boundary, same as shared embeds).
                    embed_body(
                        result,
                        manifest,
                        spec,
                        false,
                        "Published notebook not found",
                    )
                })
            }}
        </Suspense>
    }
}

// ── Shared rendering ────────────────────────────────────────────────────────

fn embed_loading() -> impl IntoView {
    view! {
        <div class="ironpad-loading">
            <p>"Loading notebook..."</p>
        </div>
    }
}

/// Render a fetched notebook in embed mode, or a compact error panel. The
/// error text stays terse: it renders inside someone else's page.
fn embed_body(
    result: Result<IronpadNotebook, ServerFnError>,
    share_manifest: Option<ironpad_common::ShareManifest>,
    spec: String,
    autorun: bool,
    not_found: &'static str,
) -> AnyView {
    match result {
        Ok(notebook) => view! {
            <ViewOnlyNotebook
                notebook
                embed=true
                embed_spec=spec
                autorun=autorun
                share_manifest=share_manifest
            />
        }
        .into_any(),

        Err(e) => view! {
            <div class="ironpad-error-boundary">
                <div class="ironpad-error-boundary-icon">"△"</div>
                <p class="ironpad-error-boundary-message">{format!("{not_found}: {e}")}</p>
            </div>
        }
        .into_any(),
    }
}
