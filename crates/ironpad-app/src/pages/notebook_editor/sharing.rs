//! Notebook lifecycle workflows: Share Immutable, Save to Account, Share
//! Mutable, Push, Discard Draft, Unpublish, Delete, and Download .ironpad.
//!
//! Extracted from the editor component (`mod.rs`) so every serialize-flow
//! shares the same flush discipline (PRD-0032 T-007) — the one inline flow
//! that predated this module (Unpublish) was also the one that missed the
//! flush and could drop the last debounce-window of typing.

use ironpad_common::IronpadNotebook;
use leptos::prelude::*;

use crate::components::toaster::{ToastIntent, Toaster};
use crate::server_fns::share_notebook;

use super::state::NotebookState;

// ── Flush + read primitives ─────────────────────────────────────────────────

/// Bump the cell-flush generation, yield so the per-cell flush effects run
/// (PRD-0032 T-007), then read the notebook back out of the model. `None`
/// means the page was disposed mid-flush (navigation) — callers just stop.
async fn flush_and_read_notebook(state: &NotebookState) -> Option<IronpadNotebook> {
    state.save_generation.try_update(|g| *g += 1)?;
    #[cfg(feature = "hydrate")]
    super::yield_for_cell_flush(super::CELL_FLUSH_YIELD_MS).await;
    state.notebook.try_get_untracked().flatten()
}

/// Flush in-progress cell edits, then serialize the current notebook to JSON
/// along with its positional type tags (for blob snapshotting, PRD-0047).
/// Returns `None` if the page was disposed mid-flush or serialization fails
/// (a failure toast is emitted with `fail_title`). Shared by the immutable
/// Share, mutable Share, and Push flows.
async fn flush_serialize_tags(
    state: &NotebookState,
    toaster: Toaster,
    fail_title: &'static str,
) -> Option<(String, Vec<String>)> {
    let mut nb = flush_and_read_notebook(state).await?;
    // Embed the author's last-run outputs into the OUTGOING copy only
    // (PRD-0056): the model and the debounced autosaves stay lean.
    if let Some(texts) = state.cell_display_texts.try_get_untracked() {
        nb.embed_saved_outputs(&texts, ironpad_common::types::SAVED_OUTPUT_BUDGET_BYTES);
    }
    let json = match serde_json::to_string(&nb) {
        Ok(j) => j,
        Err(e) => {
            toaster.toast(
                ToastIntent::Error,
                fail_title,
                format!("Failed to serialize: {e}"),
                5,
            );
            return None;
        }
    };
    // Positional type tags (one per cell, empty when unrun/non-code) let the
    // server snapshot compiled blobs. Tags come from the editor's live
    // outputs — the same source the compile path hashes with, so the
    // server-side keys line up with warm cache entries.
    let tags: Vec<String> = state.cell_outputs.try_get_untracked().map_or_else(
        || vec![String::new(); nb.cells.len()],
        |outputs| {
            nb.cells
                .iter()
                .map(|c| {
                    outputs
                        .get(&c.id)
                        .and_then(|d| d.type_tag.clone())
                        .unwrap_or_default()
                })
                .collect()
        },
    );
    Some((json, tags))
}

/// The window origin (empty during SSR, where none of these flows run).
fn window_origin() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    String::new()
}

/// Copy a URL to the clipboard, best-effort.
#[cfg(target_arch = "wasm32")]
async fn copy_to_clipboard(url: &str) {
    if let Some(window) = web_sys::window() {
        let clipboard = window.navigator().clipboard();
        let _ = wasm_bindgen_futures::JsFuture::from(clipboard.write_text(url)).await;
    }
}

/// No-op off-wasm (SSR never runs these flows); async for signature parity.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::unused_async)]
async fn copy_to_clipboard(url: &str) {
    let _ = url;
}

// ── Share Immutable ─────────────────────────────────────────────────────────

/// Serialize the current notebook, upload it via `share_notebook`, copy the
/// resulting `/shared/{hash}` URL to the clipboard, and surface the outcome
/// as a toast.
pub(super) fn share_current_notebook(state: &NotebookState, toaster: Toaster) {
    let state = *state;
    // Immediate ack: the menu closes on click, and the upload plus blob
    // snapshotting can take a few seconds of visible nothing otherwise.
    toaster.toast(
        ToastIntent::Info,
        "Sharing…",
        "Uploading the notebook and snapshotting compiled cells.",
        3,
    );
    leptos::task::spawn_local(async move {
        let Some((json, tags)) = flush_serialize_tags(&state, toaster, "Share Failed").await else {
            return;
        };
        match share_notebook(json, Some(tags)).await {
            Ok(hash) => {
                let url = format!("{}/shared/{hash}", window_origin());
                copy_to_clipboard(&url).await;
                toaster.toast(ToastIntent::Success, "Link Copied!", url, 5);
            }
            Err(e) => {
                toaster.toast(ToastIntent::Error, "Share Failed", format!("{e}"), 5);
            }
        }
    });
}

// ── Account notebooks (PRD-0064) ────────────────────────────────────────────

/// Confirm copy for Save to Account.
///
/// Gated at all because the move is lossy in a way nothing else on the page
/// says: `deleteNotebook` takes the notebook's version-history ring
/// (PRD-0058, up to 30 snapshots) with it, server-side history is an explicit
/// PRD-0064 non-goal, and Unpublish no longer hands a notebook back to
/// `/local`. So the snapshots are gone at the moment of the click, and this
/// is the only place a user can still decide otherwise.
#[cfg(any(feature = "hydrate", test))]
pub(super) const SAVE_TO_ACCOUNT_CONFIRM: &str = "Move this notebook into your account? \
     The copy in this browser is deleted, and its local version history goes with it: \
     snapshots are a local-notebook feature and do not follow it to the server. \
     Your notebook stays private until you publish it.";

/// Save to Account (PRD-0064): upload the current notebook into the
/// signed-in user's account, DELETE the local copy, and hard-navigate to
/// `/mutable/{id}`, which mounts the `ServerDraft` editor.
///
/// Share Mutable minus the publish: the notebook is server-stored and
/// editable from any browser the owner is signed in on, and invisible to
/// everyone else until Publish. The delete-then-navigate is the same
/// move-never-copy discipline Share Mutable follows — two unreconciled
/// copies of one notebook is the failure PRD-0054 removed, and a feature
/// whose point is durable storage must not reintroduce it.
#[cfg(feature = "hydrate")]
pub(super) fn save_to_account_current_notebook(state: &NotebookState, toaster: Toaster) {
    let state = *state;
    let Some(uuid) = state.notebook_id.try_get_untracked() else {
        return;
    };
    let confirmed = web_sys::window().is_some_and(|w| {
        w.confirm_with_message(SAVE_TO_ACCOUNT_CONFIRM)
            .unwrap_or(false)
    });
    if !confirmed {
        return;
    }
    // Immediate ack: the menu closes on click, and the upload plus the
    // navigation are a few seconds of visible nothing otherwise.
    toaster.toast(
        ToastIntent::Info,
        "Saving…",
        "Moving this notebook into your account.",
        3,
    );
    leptos::task::spawn_local(async move {
        // The one flush-before-serialize path (PRD-0032 T-007). The tags are
        // for blob snapshotting, which only a publish does — an unpublished
        // notebook has no reader-facing blobs to snapshot.
        let Some((json, _tags)) = flush_serialize_tags(&state, toaster, "Save Failed").await else {
            return;
        };
        match crate::server_fns::save_notebook_to_account(json).await {
            Ok(share_id) => {
                // Move, never copy — but only claim the move happened. A
                // delete that fails leaves a second, silently diverging copy
                // of this notebook on the home page, which is exactly the
                // reconciliation problem PRD-0054 removed, so it is named
                // rather than swallowed. (`delete_notebook` logs and returns
                // nothing, so the surviving record is the only evidence.)
                crate::storage::client::delete_notebook(&uuid).await;
                let local_copy_survived =
                    crate::storage::client::get_notebook(&uuid).await.is_some();
                if local_copy_survived {
                    leptos::logging::warn!(
                        "local notebook {uuid} survived the delete after Save to Account"
                    );
                }
                // Hard navigation (the URL and storage class both change);
                // the toast rides sessionStorage across it.
                let (intent, title, body) = if local_copy_survived {
                    (
                        ToastIntent::Info,
                        "Saved, With a Leftover Copy",
                        "The notebook is in your account, but the copy in this browser \
                         could not be removed. Delete it from the home page so you do \
                         not end up editing two of them.",
                    )
                } else {
                    (
                        ToastIntent::Success,
                        "Saved to Your Account",
                        "Only you can see it until you publish it.",
                    )
                };
                Toaster::toast_after_reload(intent, title, body, 6);
                if let Some(window) = web_sys::window() {
                    let _ = window.location().set_href(&format!("/mutable/{share_id}"));
                }
            }
            Err(e) => toaster.toast(ToastIntent::Error, "Save Failed", format!("{e}"), 6),
        }
    });
}

// ── Mutable shares (PRD-0049 / PRD-0054) ────────────────────────────────────

/// Convert the current notebook into a mutable share (PRD-0054): create the
/// share server-side under the signed-in account, DELETE the local copy (the
/// notebook now lives at its public URL), and hard-navigate to
/// `/mutable/{id}`, which mounts the `ServerDraft` editor.
pub(super) fn share_mutable_current_notebook(state: &NotebookState, toaster: Toaster) {
    let state = *state;
    // Same immediate ack as Share Immutable.
    toaster.toast(
        ToastIntent::Info,
        "Publishing…",
        "Creating the mutable share and snapshotting compiled cells.",
        3,
    );
    leptos::task::spawn_local(async move {
        let Some((json, tags)) = flush_serialize_tags(&state, toaster, "Share Failed").await else {
            return;
        };
        #[cfg(feature = "hydrate")]
        {
            // `try_`, because this read sits AFTER an await: navigating away
            // mid-flush disposes the signal, and a plain read of a disposed
            // signal panics, which takes the whole wasm app down rather than
            // just this flow. Mirrors `save_to_account_current_notebook`.
            let Some(uuid) = state.notebook_id.try_get_untracked() else {
                return;
            };
            match crate::server_fns::create_mutable_share(json, Some(tags)).await {
                Ok(share_id) => {
                    // The notebook's home is the share now; the local copy
                    // would only drift (PRD-0054).
                    crate::storage::client::delete_notebook(&uuid).await;
                    let url = format!("{}/mutable/{share_id}", window_origin());
                    copy_to_clipboard(&url).await;
                    // Hard navigation (the URL and storage class both change);
                    // the toast rides sessionStorage across it.
                    Toaster::toast_after_reload(
                        ToastIntent::Success,
                        "Published! Link Copied",
                        url,
                        6,
                    );
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().set_href(&format!("/mutable/{share_id}"));
                    }
                }
                Err(e) => toaster.toast(ToastIntent::Error, "Share Failed", format!("{e}"), 5),
            }
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (json, tags, state);
        }
    });
}

/// Push (PRD-0054): flush edits into the server draft, then promote it to
/// published. Uploads no notebook — the server already holds the draft.
pub(super) fn push_mutable_current_notebook(
    state: &NotebookState,
    toaster: Toaster,
    share_id: String,
) {
    let state = *state;
    // Immediate ack: the push re-snapshots blobs server-side, which can take
    // a few seconds of visible nothing otherwise.
    toaster.toast(ToastIntent::Info, "Pushing…", "Publishing your draft.", 3);
    leptos::task::spawn_local(async move {
        // Flush cell edits into the model, then write the draft NOW (the
        // debounced autosave may still be pending, and the promote must see
        // current content).
        let Some((_json, tags)) = flush_serialize_tags(&state, toaster, "Push Failed").await else {
            return;
        };
        // The durable save is the payload of a Push: if the server never got
        // the draft, promoting would publish stale content while the UI
        // claims success. Abort loudly instead, and say WHICH failure this
        // was: a transient one is retrying in the background, a refusal is
        // not (PRD-0064).
        #[cfg(feature = "hydrate")]
        if !super::state::persist_notebook_durable(&state).await {
            // A REFUSED draft (PRD-0064) is not a retry away from working, so
            // the advice differs: the same cap that blocks the autosave blocks
            // the push, and telling the owner to wait for an indicator that
            // will never clear is how this failure became undiagnosable.
            let refused = matches!(
                state.draft_save_state.try_get_untracked(),
                Some(super::state::DraftSaveState::Refused)
            );
            let body = if refused {
                "Your account is out of storage, so the draft never reached the \
                 server and nothing was published. Delete a notebook from your \
                 account to free space, then push again."
            } else {
                "Your draft could not be saved to the server, so nothing was \
                 published. It will keep retrying; push again once the draft \
                 indicator clears."
            };
            toaster.toast(ToastIntent::Error, "Push Failed", body, 6);
            return;
        }
        // Whether this is the FIRST publish (PRD-0064) decides the wording:
        // "Pushed" on a notebook nobody could read yet says nothing useful.
        let was_published = state.share_published.try_get_untracked().unwrap_or(true);
        match crate::server_fns::push_mutable(share_id, Some(tags)).await {
            Ok(promoted) => {
                let _ = state.draft_dirty.try_set(false);
                let _ = state.share_published.try_set(true);
                if !was_published {
                    toaster.toast(
                        ToastIntent::Success,
                        "Published",
                        "Your notebook is readable at its link now.",
                        4,
                    );
                } else if promoted {
                    toaster.toast(
                        ToastIntent::Success,
                        "Pushed",
                        "The published copy is updated; readers see this version now.",
                        4,
                    );
                } else {
                    toaster.toast(
                        ToastIntent::Success,
                        "Up to Date",
                        "The published copy already matches your draft.",
                        4,
                    );
                }
            }
            Err(e) => toaster.toast(ToastIntent::Error, "Push Failed", format!("{e}"), 6),
        }
    });
}

/// Discard the server draft (PRD-0054): confirm, clear it server-side, and
/// reload — the page remounts from published, which is the one reliable way
/// to rebuild editor state (cells, Monaco instances, outputs).
#[cfg(feature = "hydrate")]
pub(super) fn discard_draft_current_notebook(
    state: &NotebookState,
    toaster: Toaster,
    share_id: String,
) {
    if !state.draft_dirty.get_untracked() {
        toaster.toast(
            ToastIntent::Success,
            "Nothing to Discard",
            "Your draft already matches the published copy.",
            4,
        );
        return;
    }
    let confirmed = web_sys::window().is_some_and(|w| {
        w.confirm_with_message(
            "Discard your draft and return to the published copy? \
             Unpushed changes will be lost.",
        )
        .unwrap_or(false)
    });
    if !confirmed {
        return;
    }
    leptos::task::spawn_local(async move {
        match crate::server_fns::discard_mutable_draft(share_id).await {
            Ok(true) => {
                Toaster::toast_after_reload(
                    ToastIntent::Success,
                    "Draft Discarded",
                    "Back to the published copy.",
                    4,
                );
                if let Some(window) = web_sys::window() {
                    let _ = window.location().reload();
                }
            }
            // The server declined (PRD-0064: an unpublished notebook's draft
            // IS its content, so there is nothing to revert to). The menu
            // item is hidden in that state, so this is a stale page rather
            // than a normal path; reloading would still show the same text
            // under a toast claiming a revert.
            Ok(false) => toaster.toast(
                ToastIntent::Success,
                "Nothing to Discard",
                "This notebook has no published copy to return to.",
                5,
            ),
            Err(e) => toaster.toast(ToastIntent::Error, "Discard Failed", format!("{e}"), 6),
        }
    });
}

/// Unpublish (PRD-0064): clear the published copy server-side. The notebook
/// stays in the owner's account, at the same URL, still editable — publishing
/// again is a Push.
///
/// This replaced a flow that wrote the notebook into `IndexedDB`, deleted the
/// share, and navigated to `/local/{uuid}`: for one moment that local write
/// was the ONLY surviving copy, which is why it carried a load-bearing flush
/// and a confirm about losing it. Clearing the published copy in place
/// removes that moment rather than guarding it. Download .ironpad is how you
/// take a local file home; Delete is how you remove it from the account.
#[cfg(feature = "hydrate")]
pub(super) fn unpublish_current_notebook(
    state: &NotebookState,
    toaster: Toaster,
    share_id: String,
) {
    let confirmed = web_sys::window().is_some_and(|w| {
        w.confirm_with_message(
            "Unpublish this notebook? Its link stops working for readers. It \
             stays in your account, and you can publish it again.",
        )
        .unwrap_or(false)
    });
    if !confirmed {
        return;
    }
    toaster.toast(
        ToastIntent::Info,
        "Unpublishing…",
        "Removing the published copy.",
        3,
    );
    let state = *state;
    leptos::task::spawn_local(async move {
        match crate::server_fns::unpublish_mutable_share(share_id).await {
            Ok(()) => {
                // Back to the unpublished state, in place: no navigation, no
                // remount. The draft is now ahead of nothing, which is what
                // re-arms the toolbar button as "Publish".
                let _ = state.share_published.try_set(false);
                let _ = state.draft_dirty.try_set(true);
                toaster.toast(
                    ToastIntent::Success,
                    "Unpublished",
                    "The link no longer resolves. Your notebook is still here.",
                    5,
                );
            }
            Err(e) => toaster.toast(ToastIntent::Error, "Unpublish Failed", format!("{e}"), 6),
        }
    });
}

/// Delete an account notebook (PRD-0064): the share, its draft, its published
/// copy, and its grants all go, then home. Local mode's Delete removes the
/// `IndexedDB` record; this is the same act for the other storage class, and
/// it is the only destructive one now that Unpublish leaves the notebook in
/// place.
#[cfg(feature = "hydrate")]
pub(super) fn delete_mutable_current_notebook(toaster: Toaster, share_id: String) {
    let confirmed = web_sys::window().is_some_and(|w| {
        w.confirm_with_message(
            "Delete this notebook from your account? Its link stops working \
             and this cannot be undone. Download .ironpad first if you want \
             to keep a copy.",
        )
        .unwrap_or(false)
    });
    if !confirmed {
        return;
    }
    toaster.toast(
        ToastIntent::Info,
        "Deleting…",
        "Removing this notebook from your account.",
        3,
    );
    leptos::task::spawn_local(async move {
        match crate::server_fns::delete_mutable_share(share_id).await {
            Ok(()) => {
                // Hard navigation: the notebook this page edits is gone.
                Toaster::toast_after_reload(
                    ToastIntent::Success,
                    "Deleted",
                    "The notebook is no longer in your account.",
                    4,
                );
                if let Some(window) = web_sys::window() {
                    let _ = window.location().set_href("/");
                }
            }
            Err(e) => toaster.toast(ToastIntent::Error, "Delete Failed", format!("{e}"), 6),
        }
    });
}

// ── Download .ironpad ───────────────────────────────────────────────────────

/// Download the current notebook as a `.ironpad` file, serialized from the
/// live model. Serializing (rather than exporting the `IndexedDB` record)
/// makes the flow identical in Local and `ServerDraft` modes — a published
/// notebook has no `IndexedDB` record to export, which used to make this
/// menu item silently do nothing in the mutable editor.
#[cfg(feature = "hydrate")]
pub(super) fn download_current_notebook(state: &NotebookState, toaster: Toaster) {
    let state = *state;
    leptos::task::spawn_local(async move {
        let Some(mut nb) = flush_and_read_notebook(&state).await else {
            return;
        };
        // Downloads carry the outputs too (PRD-0056) — this is how a public
        // blog notebook gets committed outputs.
        if let Some(texts) = state.cell_display_texts.try_get_untracked() {
            nb.embed_saved_outputs(&texts, ironpad_common::types::SAVED_OUTPUT_BUDGET_BYTES);
        }
        match serde_json::to_string_pretty(&nb) {
            Ok(json) => super::export::trigger_ironpad_download(&json, &nb.title),
            Err(e) => toaster.toast(
                ToastIntent::Error,
                "Download Failed",
                format!("Failed to serialize: {e}"),
                5,
            ),
        }
    });
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Save to Account is a MOVE, and one thing it moves is unrecoverable:
    /// `deleteNotebook` in `storage.js` takes the notebook's history ring with
    /// the record, server-side history is an explicit PRD-0064 non-goal, and
    /// Unpublish no longer returns anything to `/local`. There is no route
    /// back, so the disclosure has to survive a copy edit.
    #[test]
    fn save_to_account_confirm_names_what_the_move_costs() {
        let copy = SAVE_TO_ACCOUNT_CONFIRM.to_ascii_lowercase();
        assert!(
            copy.contains("version history"),
            "the confirm must say the local version history does not come along"
        );
        assert!(
            copy.contains("deleted"),
            "the confirm must say the browser-local copy is deleted"
        );
        assert!(
            copy.contains("publish"),
            "the confirm must say the notebook is not published by this act"
        );
    }

    /// User-visible strings carry no em-dashes (repo-wide style pass).
    #[test]
    fn save_to_account_confirm_has_no_em_dash() {
        assert!(!SAVE_TO_ACCOUNT_CONFIRM.contains('—'));
    }
}
