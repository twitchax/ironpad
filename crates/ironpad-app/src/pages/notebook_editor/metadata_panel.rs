//! Notebook presentation metadata: description, tags, and the social-preview
//! image override (PRD-0051).
//!
//! These fields have existed on [`IronpadNotebook`] since PRD-0050 wired up
//! link unfurls, but nothing could set them: they were reachable only by
//! hand-editing a `.ironpad` file, so every notebook authored *in* ironpad
//! unfurled with a generic description and no tags. This is the panel that
//! closes that loop.
//!
//! Structured like [`super::shared_editor_panel`], and mounted beside it below
//! the cell list, on the same reasoning: the cells are the story, and this is
//! the colophon.

use ironpad_common::protocol::{ClientId, Mutation, NotebookMetaPatch};
use ironpad_common::{OG_IMAGE_MAX_PX, OG_IMAGE_MIN_PX};
use leptos::ev;
use leptos::prelude::*;

use crate::components::copy_button::CopyButton;
use crate::components::toaster::{ToastIntent, Toaster};
use crate::model::NotebookModel;

#[cfg(not(feature = "hydrate"))]
use super::state::persist_notebook;
use super::state::NotebookState;

/// Floors the visible Saving… state, matching the shared-source panel: the
/// `IndexedDB` write is usually faster than a blink.
#[cfg(feature = "hydrate")]
const MIN_SAVING_MS: i32 = 500;

/// Collapsed-by-default metadata section. Expanding lazily mounts the form.
#[component]
pub(super) fn NotebookMetadataSection(
    /// `Some(share_id)` while the notebook is mutable-backed on
    /// this device (PRD-0049); drives the published-URL row, so the share
    /// link is findable after its one appearance in the share toast.
    mutable_binding: RwSignal<Option<String>>,
) -> impl IntoView {
    let collapsed = RwSignal::new(true);
    let toggle_icon = move || if collapsed.get() { "▸" } else { "▾" };

    view! {
        <div class="view-only-shared-section">
            <button
                class="view-only-shared-header"
                on:click=move |_| collapsed.update(|c| *c = !*c)
            >
                <span class="ironpad-output-toggle">{toggle_icon}</span>
                <span>"\u{1f5c2} Notebook Metadata (link previews)"</span>
            </button>
            {move || {
                (!collapsed.get())
                    .then(|| {
                        view! {
                            <div class="view-only-shared-body">
                                <NotebookMetadataPanel mutable_binding=mutable_binding />
                            </div>
                        }
                    })
            }}
        </div>
    }
}

/// The deployment origin for building the published URL. Empty during SSR:
/// the panel body only mounts client-side, but the code must compile there,
/// and a root-relative URL is a sane fallback.
fn published_origin() -> String {
    #[cfg(feature = "hydrate")]
    {
        leptos::web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default()
    }
    #[cfg(not(feature = "hydrate"))]
    {
        String::new()
    }
}

/// Access controls for a published share (PRD-0061): the private toggle plus
/// READ grants by GitHub handle. Rendered only in `ServerDraft` mode, under
/// the published-URL row — access is a property of the PUBLISHED thing.
#[component]
#[allow(clippy::too_many_lines)] // One form, read top to bottom.
fn ShareAccessSection(share_id: String) -> impl IntoView {
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

/// The form body.
#[component]
#[allow(clippy::too_many_lines)] // One form, read top to bottom.
fn NotebookMetadataPanel(mutable_binding: RwSignal<Option<String>>) -> impl IntoView {
    let state = expect_context::<NotebookState>();
    let model = expect_context::<NotebookModel>();
    let toaster = Toaster::expect_context();

    // Seeded untracked from the notebook, then owned by the form until Save.
    // The panel is only mounted while expanded, so it re-seeds from current
    // state every time it opens.
    let notebook = state.notebook.get_untracked();
    let seed = |f: fn(&ironpad_common::IronpadNotebook) -> Option<String>| {
        notebook.as_ref().and_then(f).unwrap_or_default()
    };

    let description = RwSignal::new(seed(|nb| nb.description.clone()));
    let tags = RwSignal::new(
        notebook
            .as_ref()
            .and_then(|nb| nb.tags.clone())
            .unwrap_or_default()
            .join(", "),
    );
    let og_image = RwSignal::new(seed(|nb| nb.og_image.clone()));
    let og_width = RwSignal::new(seed(|nb| nb.og_image_width.map(|v| v.to_string())));
    let og_height = RwSignal::new(seed(|nb| nb.og_image_height.map(|v| v.to_string())));
    let saving = RwSignal::new(false);

    // The dimension fields describe the override, so they are meaningless
    // without one: the generated card is always 1200x630 and says so from a
    // constant.
    let has_override = move || !og_image.get().trim().is_empty();

    // Mirrors `IronpadNotebook::og_image_path`, which is the real gate. Shown
    // inline because a rejected path fails silently at unfurl time, which is
    // the worst possible moment to discover it.
    let og_image_problem = move || {
        let raw = og_image.get();
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        if !raw.starts_with('/') || raw.starts_with("//") {
            return Some(
                "Must be a root-relative path starting with a single \"/\", such as \
                 /og-custom/mandelbrot.png. Another origin would let this notebook \
                 put someone else's picture under ironpad's name."
                    .to_string(),
            );
        }
        None
    };

    let dimension_problem = move || {
        if !has_override() {
            return None;
        }
        let (w, h) = (og_width.get(), og_height.get());
        let (w, h) = (w.trim().to_string(), h.trim().to_string());
        if w.is_empty() && h.is_empty() {
            return None;
        }
        let parse = |s: &str| {
            s.parse::<u32>()
                .ok()
                .filter(|px| (OG_IMAGE_MIN_PX..=OG_IMAGE_MAX_PX).contains(px))
        };
        if w.is_empty() || h.is_empty() {
            return Some("Set both width and height, or neither.".to_string());
        }
        if parse(&w).is_none() || parse(&h).is_none() {
            return Some(format!(
                "Each side must be a whole number between {OG_IMAGE_MIN_PX} and {OG_IMAGE_MAX_PX} pixels."
            ));
        }
        None
    };

    let on_save = move |_| {
        if saving.get_untracked() {
            return;
        }

        // Empty means cleared, so every field is `Some(...)`: this form owns
        // all of them, and a blank box has to be able to remove a value.
        let text = |sig: RwSignal<String>| {
            let v = sig.get_untracked().trim().to_string();
            Some((!v.is_empty()).then_some(v))
        };
        let pixels = |sig: RwSignal<String>| Some(sig.get_untracked().trim().parse::<u32>().ok());

        let tag_list: Vec<String> = tags
            .get_untracked()
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();

        let has_image = !og_image.get_untracked().trim().is_empty();
        let meta = NotebookMetaPatch {
            description: text(description),
            tags: Some((!tag_list.is_empty()).then_some(tag_list)),
            og_image: text(og_image),
            // Dimensions describe the override; dropping them with it keeps a
            // stale size from being reattached to a later image.
            og_image_width: if has_image {
                pixels(og_width)
            } else {
                Some(None)
            },
            og_image_height: if has_image {
                pixels(og_height)
            } else {
                Some(None)
            },
            ..Default::default()
        };

        if model
            .apply(Mutation::NotebookUpdateMeta { meta }, ClientId::browser())
            .is_err()
        {
            return;
        }

        let dispatch_saved_toast = move || {
            toaster.toast(
                ToastIntent::Success,
                "Notebook metadata saved",
                "Shared and published copies update on the next share or push.",
                3,
            );
        };

        #[cfg(feature = "hydrate")]
        {
            saving.set(true);
            leptos::task::spawn_local(async move {
                let started = js_sys::Date::now();
                super::state::persist_notebook_durable(&state).await;
                #[allow(clippy::cast_possible_truncation)]
                let remaining = MIN_SAVING_MS - (js_sys::Date::now() - started) as i32;
                if remaining > 0 {
                    super::yield_for_cell_flush(remaining).await;
                }
                // The panel may have been disposed mid-save (navigation, or a
                // collapse); try_set keeps this continuation panic-free.
                let _ = saving.try_set(false);
                dispatch_saved_toast();
            });
        }
        #[cfg(not(feature = "hydrate"))]
        {
            persist_notebook(&state);
            dispatch_saved_toast();
        }
    };

    view! {
        <div class="ironpad-shared-editor-body">
            <p class="ironpad-metadata-intro">
                "Used for the page title, search, and the preview card that Reddit, \
                 X, Slack, and Discord build when someone pastes a link to this notebook."
            </p>

            {move || mutable_binding.get().map(|share_id| {
                let url = format!("{}/mutable/{share_id}", published_origin());
                view! {
                    <div class="ironpad-metadata-field">
                        <span class="ironpad-metadata-label">"Published at"</span>
                        <div class="ironpad-metadata-published-url">
                            <code>{url.clone()}</code>
                            <CopyButton text=url />
                        </div>
                    </div>
                    <ShareAccessSection share_id=share_id />
                }
            })}

            <label class="ironpad-metadata-field">
                <span class="ironpad-metadata-label">"Description"</span>
                <textarea
                    class="ironpad-metadata-textarea"
                    rows="3"
                    placeholder="One or two sentences. Shown on the home page and in link previews."
                    prop:value=move || description.get()
                    on:input=move |ev| description.set(event_target_value(&ev))
                ></textarea>
            </label>

            <label class="ironpad-metadata-field">
                <span class="ironpad-metadata-label">"Tags"</span>
                <input
                    class="ironpad-search-input"
                    r#type="text"
                    placeholder="Comma separated, e.g. simulation, autodiff"
                    prop:value=move || tags.get()
                    on:input=move |ev| tags.set(event_target_value(&ev))
                />
            </label>

            <label class="ironpad-metadata-field">
                <span class="ironpad-metadata-label">"Preview image (optional)"</span>
                <input
                    class="ironpad-metadata-input"
                    r#type="text"
                    placeholder="/og-custom/my-notebook.png"
                    prop:value=move || og_image.get()
                    on:input=move |ev| og_image.set(event_target_value(&ev))
                />
                <span class="ironpad-metadata-hint">
                    "Leave blank to use the card ironpad generates from the title, \
                     description, and first code cell."
                </span>
                {move || {
                    og_image_problem()
                        .map(|msg| {
                            view! { <span class="ironpad-metadata-problem">{msg}</span> }
                        })
                }}
            </label>

            // Only meaningful alongside an override, and actively harmful
            // without one: unfurlers lay the preview out from the declared
            // size before the image loads.
            <Show when=has_override>
                <div class="ironpad-metadata-field">
                    <span class="ironpad-metadata-label">"Preview image size"</span>
                    <div class="ironpad-metadata-dimensions">
                        <input
                            class="ironpad-metadata-number"
                            r#type="number"
                            min=OG_IMAGE_MIN_PX.to_string()
                            max=OG_IMAGE_MAX_PX.to_string()
                            placeholder="width"
                            prop:value=move || og_width.get()
                            on:input=move |ev: ev::Event| og_width.set(event_target_value(&ev))
                        />
                        <span class="ironpad-metadata-times">"\u{00d7}"</span>
                        <input
                            class="ironpad-metadata-number"
                            r#type="number"
                            min=OG_IMAGE_MIN_PX.to_string()
                            max=OG_IMAGE_MAX_PX.to_string()
                            placeholder="height"
                            prop:value=move || og_height.get()
                            on:input=move |ev: ev::Event| og_height.set(event_target_value(&ev))
                        />
                        <span class="ironpad-metadata-hint">"pixels"</span>
                    </div>
                    {move || {
                        dimension_problem()
                            .map(|msg| {
                                view! { <span class="ironpad-metadata-problem">{msg}</span> }
                            })
                    }}
                </div>
            </Show>

            <div class="ironpad-shared-editor-actions">
                <button
                    class="ironpad-btn ironpad-btn--primary"
                    on:click=on_save
                    prop:disabled=move || saving.get()
                >
                    {move || if saving.get() { "Saving\u{2026}" } else { "Save" }}
                </button>
            </div>
        </div>
    }
}
