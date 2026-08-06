//! Access administration for a published share (PRD-0061): the private
//! toggle plus READ grants by GitHub handle.
//!
//! Its own module rather than a resident of `metadata_panel.rs`: that file's
//! concern is presentation metadata (the PRD-0051 `NotebookMetaPatch` flow),
//! while this component speaks to the share-RBAC server fns and touches no
//! metadata seam — and it will grow (`rbac_grant` is shaped for an EDIT role
//! later). The metadata panel composes it under the published-URL row.

use leptos::prelude::*;

use crate::components::toaster::{ToastIntent, Toaster};

/// Access controls for a published share (PRD-0061): the private toggle plus
/// READ grants by GitHub handle. Rendered only in `ServerDraft` mode, under
/// the published-URL row — access is a property of the PUBLISHED thing.
#[component]
#[allow(clippy::too_many_lines)] // One form, read top to bottom.
pub(super) fn ShareAccessSection(share_id: String) -> impl IntoView {
    use ironpad_common::ShareGrant;

    let toaster = Toaster::expect_context();
    let share_id = StoredValue::new(share_id);
    // Consumed only inside the hydrate-gated closures; SSR still renders the
    // (inert) markup.
    #[cfg(not(feature = "hydrate"))]
    let _ = (&toaster, &share_id);

    let is_private = RwSignal::new(false);
    let grants: RwSignal<Vec<ShareGrant>> = RwSignal::new(Vec::new());
    let grant_input = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let loaded = RwSignal::new(false);

    // Seed from the server once on mount (client-only; the panel body never
    // renders during SSR).
    #[cfg(feature = "hydrate")]
    {
        leptos::task::spawn_local(async move {
            if let Ok(access) = crate::server_fns::get_mutable_access(share_id.get_value()).await {
                let _ = is_private.try_set(access.private);
                let _ = grants.try_set(access.grants);
                let _ = loaded.try_set(true);
            }
        });
    }

    let on_toggle_private = move |_| {
        if busy.get_untracked() || !loaded.get_untracked() {
            return;
        }
        #[cfg(feature = "hydrate")]
        {
            let next = !is_private.get_untracked();
            busy.set(true);
            leptos::task::spawn_local(async move {
                match crate::server_fns::set_mutable_private(share_id.get_value(), next).await {
                    Ok(()) => {
                        let _ = is_private.try_set(next);
                        toaster.toast(
                            ToastIntent::Success,
                            if next {
                                "Share is now private"
                            } else {
                                "Share is now public"
                            },
                            if next {
                                "Only you and people you grant below can view it.".to_string()
                            } else {
                                "Anyone with the link can view it again.".to_string()
                            },
                            4,
                        );
                    }
                    Err(e) => toaster.toast(
                        ToastIntent::Error,
                        "Could not change visibility",
                        e.to_string(),
                        5,
                    ),
                }
                let _ = busy.try_set(false);
            });
        }
    };

    let on_grant = move |_| {
        if busy.get_untracked() {
            return;
        }
        #[cfg(feature = "hydrate")]
        {
            let login = grant_input.get_untracked();
            if login.trim().is_empty() {
                return;
            }
            busy.set(true);
            leptos::task::spawn_local(async move {
                match crate::server_fns::grant_mutable_read(share_id.get_value(), login).await {
                    Ok(grant) => {
                        let _ = grants.try_update(|g| {
                            if !g.iter().any(|x| x.github_id == grant.github_id) {
                                g.push(grant);
                            }
                        });
                        let _ = grant_input.try_set(String::new());
                    }
                    Err(e) => toaster.toast(
                        ToastIntent::Error,
                        "Could not grant access",
                        e.to_string(),
                        6,
                    ),
                }
                let _ = busy.try_set(false);
            });
        }
    };

    let on_revoke = move |github_id: String| {
        #[cfg(not(feature = "hydrate"))]
        let _ = &github_id;
        #[cfg(feature = "hydrate")]
        {
            leptos::task::spawn_local(async move {
                match crate::server_fns::revoke_mutable_read(
                    share_id.get_value(),
                    github_id.clone(),
                )
                .await
                {
                    Ok(()) => {
                        let _ = grants.try_update(|g| g.retain(|x| x.github_id != github_id));
                    }
                    Err(e) => toaster.toast(
                        ToastIntent::Error,
                        "Could not revoke access",
                        e.to_string(),
                        5,
                    ),
                }
            });
        }
    };

    view! {
        <div class="ironpad-metadata-field">
            <span class="ironpad-metadata-label">"Access"</span>
            <label class="ironpad-access-toggle">
                <input
                    r#type="checkbox"
                    prop:checked=move || is_private.get()
                    prop:disabled=move || busy.get() || !loaded.get()
                    on:change=on_toggle_private
                />
                <span>"Private (only you and people you grant can view)"</span>
            </label>
            <Show when=move || is_private.get()>
                <div class="ironpad-access-grants">
                    {move || {
                        let list = grants.get();
                        if list.is_empty() {
                            view! {
                                <span class="ironpad-metadata-hint">
                                    "No one else has access yet."
                                </span>
                            }
                            .into_any()
                        } else {
                            list.into_iter()
                                .map(|g| {
                                    let gid = g.github_id.clone();
                                    view! {
                                        <span class="ironpad-access-grant">
                                            {format!("@{}", g.login)}
                                            <button
                                                class="ironpad-access-revoke"
                                                title="Revoke access"
                                                on:click=move |_| on_revoke(gid.clone())
                                            >
                                                "\u{2715}"
                                            </button>
                                        </span>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }
                    }}
                </div>
                <div class="ironpad-access-add">
                    <input
                        class="ironpad-metadata-input"
                        r#type="text"
                        placeholder="GitHub username"
                        prop:value=move || grant_input.get()
                        on:input=move |ev| grant_input.set(event_target_value(&ev))
                    />
                    <button
                        class="ironpad-btn"
                        on:click=on_grant
                        prop:disabled=move || busy.get()
                    >
                        "Grant"
                    </button>
                </div>
                <span class="ironpad-metadata-hint">
                    "They need to have signed in to ironpad with GitHub once before you can grant them access."
                </span>
            </Show>
        </div>
    }
}
