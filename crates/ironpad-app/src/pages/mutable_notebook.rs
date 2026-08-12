use crate::components::icon::{Icon, IconLabel};
use crate::components::icons;
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use ironpad_common::{MutableNotebookAccess, MutableNotebookResponse, ShareManifest};

use crate::components::app_layout::LayoutContext;
use crate::components::social_meta::{mark_not_found, SocialMeta};
use crate::components::view_only_notebook::ViewOnlyNotebook;
use crate::pages::notebook_editor::{NotebookEditor, ServerDraftMount};
use crate::server_fns::{get_mutable_manifest, get_mutable_notebook};

/// Copy for the neutral first-paint state on a `/mutable` 404.
///
/// Deliberately identical to the `Suspense` fallback: an owner waiting on the
/// hydrate-time ownership probe is genuinely still loading, and reusing the
/// fallback's own words means the page shows ONE continuous loading state
/// instead of flashing a second, differently-worded one. It must never
/// accuse the notebook of being missing (see the test below) — that is the
/// whole defect this state exists to fix.
const LOADING_MESSAGE: &str = "Loading notebook...";

/// Whether this request carries a session cookie at all.
///
/// Deliberately NOT an ownership check, and it cannot be one here:
/// `MutableNotebookAccess::NotFound` is the same answer for a notebook that
/// never existed and for an owner's own unpublished account notebook, which
/// PRD-0064 keeps indistinguishable on purpose, and the wire type carries
/// nothing to separate them. This is the cheapest honest question the SSR
/// render can ask — one header read, no database, no draft content pulled
/// into a path whose invariant is that draft content is not there — and it
/// only picks which copy to paint under a status that stays 404 either way.
#[cfg(feature = "ssr")]
async fn request_carries_session() -> bool {
    let Ok(headers) = leptos_axum::extract::<http::HeaderMap>().await else {
        return false;
    };
    headers
        .get(http::header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(crate::auth::session_token_from_cookie_header)
        .is_some()
}

/// The client has no request to read, and the session cookie is `HttpOnly`
/// by design, so `LayoutContext::auth` answers this after hydrate instead.
#[cfg(not(feature = "ssr"))]
#[allow(clippy::unused_async)] // Mirrors the ssr signature.
async fn request_carries_session() -> bool {
    false
}

/// Whether a `/mutable/{id}` that resolved to `NotFound` paints the neutral
/// loading state instead of the not-found panel.
///
/// The HTTP status is 404 either way — `unpublished_notebooks.rs` and the
/// account-notebook e2e spec both pin it, and it is what keeps unfurlers
/// and crawlers from treating the URL as real content. This chooses copy,
/// nothing else.
///
/// It exists because an owner's own unpublished account notebook IS a 404 on
/// the reader path (PRD-0064), and SSR always renders the reader, so without
/// this the primary flow of that PRD first-paints the owner's notebook as
/// missing, with a warning icon, on every single load until the hydrate-time
/// ownership probe swaps the editor in.
///
/// The three inputs are three different vantage points on one question:
///
/// - `signed_in_hint` is read from the SSR request, so it is baked into the
///   first paint. It says "somebody is signed in", never "this person owns
///   this notebook".
/// - `auth_signed_in` is the hydrated answer (`LayoutContext::auth`). It
///   covers a client-side navigation, where there is no SSR response and
///   therefore no cookie to read; it is `None` at first render on both
///   sides, so it cannot desync SSR from hydration.
/// - `probe_settled` is the authoritative end. Once `get_mutable_for_edit`
///   has come back without an editor, the viewer demonstrably does not own
///   this notebook and the real not-found copy lands.
///
/// Consequence, stated rather than hidden: a signed-in stranger gets the
/// neutral state for one round-trip before the not-found copy, because a
/// session cookie cannot distinguish them from the owner. An exact answer
/// needs the access core to carry an owner hint on the `NotFound` arm.
fn show_owner_placeholder(signed_in_hint: bool, auth_signed_in: bool, probe_settled: bool) -> bool {
    !probe_settled && (signed_in_hint || auth_signed_in)
}

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
    // Tracked: the router reuses this outlet on a param-only change (e.g. a
    // markdown cross-link between two /mutable pages), so a frozen untracked
    // read would keep rendering — or worse, keep EDITING — the old notebook.
    let id = Memo::new(move |_| params.read().get("id").unwrap_or_default());

    let ctx = expect_context::<LayoutContext>();
    // Real title shows once in the view-only toolbar; keep the header center clear.
    ctx.notebook_title.set(None);

    // `?view=reader` pins the reader even for the owner.
    let query = leptos_router::hooks::use_query_map();
    let force_reader = Memo::new(move |_| query.read().get("view").is_some_and(|v| v == "reader"));

    let notebook_resource = Resource::new(
        move || id.get(),
        |id| async move {
            let access = get_mutable_notebook(id.clone()).await?;
            // Manifest only matters when the notebook is viewable; a
            // missing/degraded manifest falls back to live compilation.
            let manifest = if matches!(access, MutableNotebookAccess::Found(_)) {
                get_mutable_manifest(id).await.unwrap_or(None)
            } else {
                None
            };
            // Only the not-found arm consults this, so only that arm pays for
            // it. Riding the existing resource is what makes it survive into
            // the first paint: its value is serialized with the rest of the
            // SSR payload, so the hydrating client reads the SAME bool the
            // server rendered from and cannot pick a different branch.
            let signed_in_hint = matches!(access, MutableNotebookAccess::NotFound)
                && request_carries_session().await;
            Ok::<_, ServerFnError>((access, manifest, signed_in_hint))
        },
    );

    Effect::new(move || {
        if let Some(Ok((MutableNotebookAccess::Found(r), _, _))) = notebook_resource.get() {
            ctx.cell_count.set(r.notebook.cells.len());
        }
    });

    // Owner detection (PRD-0054): resolved on hydrate so SSR stays the
    // reader. A non-owner or anonymous caller errs, which simply leaves the
    // reader in place. The resolved edit is tagged with the share id it was
    // fetched FOR, so a param-only navigation can never mount the old draft
    // under the new URL (the fetch is async; render order is not guaranteed).
    let owner_edit: RwSignal<Option<(String, ironpad_common::MutableEditResponse)>> =
        RwSignal::new(None);

    // Has the ownership probe come back without an editor? Starts `false` on
    // BOTH sides — SSR never probes, and on the client the effect below runs
    // after the first render — so it cannot make hydration disagree with the
    // markup it is hydrating.
    let owner_probe_settled = RwSignal::new(false);

    #[cfg(feature = "hydrate")]
    Effect::new(move || {
        let edit_id = id.get();
        // A new target (or a pinned reader) invalidates any resolved editor.
        owner_edit.set(None);
        if force_reader.get() {
            // No probe will run, so nothing is pending: a pinned reader that
            // 404s says so immediately.
            owner_probe_settled.set(true);
            return;
        }
        owner_probe_settled.set(false);
        leptos::task::spawn_local(async move {
            let resolved = crate::server_fns::get_mutable_for_edit(edit_id.clone()).await;
            // Drop responses that raced a later navigation; the newer run of
            // this effect has already reset the probe and will settle it.
            if id.try_get_untracked().as_ref() != Some(&edit_id) {
                return;
            }
            if let Ok(Some(edit)) = resolved {
                let _ = owner_edit.try_set(Some((edit_id, edit)));
            } else {
                // Not the owner (or genuinely no such notebook): the reader's
                // real answer is final now, not a placeholder.
                let _ = owner_probe_settled.try_set(true);
            }
        });
    });

    view! {
        {move || {
            let share_id = id.get();
            if let Some((for_id, edit)) = owner_edit.get() {
                if for_id == share_id {
                    // The owner's editor over the server draft. Mounting the
                    // editor replaces the reader wholesale; the session-cleanup
                    // and persistence seams live inside NotebookEditor.
                    return view! {
                        <NotebookEditor
                            notebook_id=edit.notebook.id.to_string()
                            server_draft=Some(ServerDraftMount {
                                share_id: share_id.clone(),
                                notebook: edit.notebook.clone(),
                                dirty: edit.dirty,
                                published: edit.published,
                            })
                        />
                    }
                    .into_any();
                }
            }
            let id = share_id;
            let force_reader = force_reader.get();
            view! {
                <Suspense fallback=move || {
                    view! {
                        <div class="ironpad-loading">
                            <p>{LOADING_MESSAGE}</p>
                        </div>
                    }
                }>
                    {move || {
                        let id = id.clone();
                        Suspend::new(async move {
                            match notebook_resource.await {
                                Ok((MutableNotebookAccess::Found(response), manifest, _)) => view! {
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
                                        oembed=true
                                        noindex=true
                                    />
                                    <MutableReader id response=*response manifest force_reader />
                                }.into_any(),
                                Ok((MutableNotebookAccess::Private { signed_in }, _, _)) => view! {
                                    // No SocialMeta: a private share must not
                                    // hand its title to unfurlers. The prompt
                                    // is the whole point — a private link
                                    // given to the right person works after
                                    // one sign-in (PRD-0061).
                                    {mark_not_found()}
                                    <div class="ironpad-error-boundary">
                                        <div class="ironpad-error-boundary-icon"><Icon icon=icons::LOCKED/></div>
                                        <p class="ironpad-error-boundary-message">
                                            "This notebook is private."
                                        </p>
                                        {if signed_in {
                                            view! {
                                                <p class="ironpad-error-boundary-hint">
                                                    "Your account does not have access. Ask the author to grant your GitHub username."
                                                </p>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <p class="ironpad-error-boundary-hint">
                                                    "If the author gave you access, "
                                                    <a href="/auth/github" rel="external">"sign in with GitHub"</a>
                                                    " to view it."
                                                </p>
                                            }.into_any()
                                        }}
                                    </div>
                                }.into_any(),
                                Ok((MutableNotebookAccess::NotFound, _, signed_in_hint)) => view! {
                                    // The 404 is unconditional and lives
                                    // OUTSIDE the branch below: an owner's
                                    // unpublished notebook must stay a 404 to
                                    // crawlers and unfurlers no matter which
                                    // copy the person in front of it sees.
                                    {mark_not_found()}
                                    {move || {
                                        let auth_signed_in = ctx
                                            .auth
                                            .get()
                                            .is_some_and(|a| a.user.is_some());
                                        if show_owner_placeholder(
                                            signed_in_hint,
                                            auth_signed_in,
                                            owner_probe_settled.get(),
                                        ) {
                                            // No warning icon, no "not found":
                                            // this viewer may well own the
                                            // notebook, and the ownership probe
                                            // has not answered yet.
                                            view! {
                                                <div class="ironpad-loading">
                                                    <p>{LOADING_MESSAGE}</p>
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <div class="ironpad-error-boundary">
                                                    <div class="ironpad-error-boundary-icon"><Icon icon=icons::WARNING/></div>
                                                    <p class="ironpad-error-boundary-message">
                                                        "This mutable notebook was not found."
                                                    </p>
                                                    <p class="ironpad-error-boundary-hint">
                                                        "The link may be wrong, or the author may have unpublished it."
                                                    </p>
                                                </div>
                                            }.into_any()
                                        }
                                    }}
                                }.into_any(),
                                Err(e) => view! {
                                    <div class="ironpad-error-boundary">
                                        <div class="ironpad-error-boundary-icon"><Icon icon=icons::WARNING/></div>
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
                <IconLabel icon=icons::EDIT label="Edit"/>
            </a>
        }
        .into_any()
    });

    let embed_spec = format!("mutable/{id}");
    view! {
        {attribution}
        <ViewOnlyNotebook
            notebook
            embed_spec=embed_spec
            share_manifest=manifest
            controls=edit_back
        />
    }
}

#[cfg(test)]
mod tests {
    use super::{show_owner_placeholder, LOADING_MESSAGE};

    #[test]
    fn an_anonymous_visitor_sees_the_not_found_copy_immediately() {
        // No cookie on the request, no hydrated session: nothing here even
        // plausibly owns the notebook, so the real answer paints first and
        // never flickers through a placeholder.
        assert!(!show_owner_placeholder(false, false, false));
        assert!(!show_owner_placeholder(false, false, true));
    }

    #[test]
    fn a_signed_in_request_paints_neutral_until_the_probe_settles() {
        // This is the defect: the owner's own unpublished notebook 404s on
        // the reader path, and SSR renders the reader, so the first paint
        // must not accuse it of being missing while the ownership probe runs.
        assert!(show_owner_placeholder(true, false, false));
    }

    #[test]
    fn a_settled_probe_beats_every_hint() {
        // A signed-in stranger: the cookie hint said "maybe", the probe came
        // back without an editor, and the not-found copy is now the truth.
        assert!(!show_owner_placeholder(true, false, true));
        assert!(!show_owner_placeholder(true, true, true));
        assert!(!show_owner_placeholder(false, true, true));
    }

    #[test]
    fn a_hydrated_session_covers_a_client_side_navigation() {
        // Soft navigation (home -> /mutable/{id}) renders no SSR response, so
        // there is no cookie to read and the hint is false. Without the
        // hydrated auth term the owner would still get "not found" first.
        assert!(show_owner_placeholder(false, true, false));
    }

    #[test]
    fn the_neutral_copy_never_reports_the_notebook_missing() {
        // The placeholder is shown under a 404 for a notebook the viewer may
        // own. Wording that reports it missing, unpublished, or wrong would
        // reintroduce the exact defect while the branch logic stayed correct.
        let copy = LOADING_MESSAGE.to_ascii_lowercase();
        for accusation in ["not found", "missing", "unpublish", "wrong", "no longer"] {
            assert!(
                !copy.contains(accusation),
                "neutral placeholder copy must not say {accusation:?}: {LOADING_MESSAGE:?}"
            );
        }
    }
}
