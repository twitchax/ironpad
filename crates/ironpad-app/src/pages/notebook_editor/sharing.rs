//! Notebook lifecycle workflows: Share Immutable, Share Mutable, Push,
//! Discard Draft, Unpublish, and Download .ironpad.
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
    let nb = flush_and_read_notebook(state).await?;
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
            let uuid = state.notebook_id.get_untracked();
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
        // claims success. Abort loudly instead — the autosave retry loop
        // keeps trying in the background.
        #[cfg(feature = "hydrate")]
        if !super::state::persist_notebook_durable(&state).await {
            toaster.toast(
                ToastIntent::Error,
                "Push Failed",
                "Your draft could not be saved to the server, so nothing was \
                 published. It will keep retrying; push again once the draft \
                 indicator clears.",
                6,
            );
            return;
        }
        match crate::server_fns::push_mutable(share_id, Some(tags)).await {
            Ok(promoted) => {
                let _ = state.draft_dirty.try_set(false);
                if promoted {
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
            Ok(()) => {
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
            Err(e) => toaster.toast(ToastIntent::Error, "Discard Failed", format!("{e}"), 6),
        }
    });
}

/// Unpublish (PRD-0054): save the current content into the private
/// `IndexedDB` store FIRST, then delete the share, then hard-navigate to
/// `/local/{uuid}`.
///
/// The flush before the local save is load-bearing: the model lags open
/// editors by up to the cell debounce, and once the share (published AND
/// draft) is deleted, the local save is the only surviving copy of the
/// notebook — an unflushed keystroke here would be gone permanently.
#[cfg(feature = "hydrate")]
pub(super) fn unpublish_current_notebook(
    state: &NotebookState,
    toaster: Toaster,
    share_id: String,
) {
    let confirmed = web_sys::window().is_some_and(|w| {
        w.confirm_with_message(
            "Unpublish this notebook? The public link stops working and it \
             returns to your private list.",
        )
        .unwrap_or(false)
    });
    if !confirmed {
        return;
    }
    let state = *state;
    leptos::task::spawn_local(async move {
        let Some(nb) = flush_and_read_notebook(&state).await else {
            return;
        };
        let Some(uuid) = state.notebook_id.try_get_untracked() else {
            return;
        };
        // Bring the content home BEFORE deleting the share so a failure
        // can't strand the notebook nowhere.
        if let Err(e) = crate::storage::client::save_notebook(&nb).await {
            toaster.toast(
                ToastIntent::Error,
                "Unpublish Failed",
                format!("Could not save locally: {e:?}"),
                6,
            );
            return;
        }
        match crate::server_fns::delete_mutable_share(share_id).await {
            Ok(()) => {
                // Hard navigation: the storage class and URL both change
                // (PRD-0054).
                Toaster::toast_after_reload(
                    ToastIntent::Success,
                    "Unpublished",
                    "Back in your private list.",
                    4,
                );
                if let Some(window) = web_sys::window() {
                    let _ = window.location().set_href(&format!("/local/{uuid}"));
                }
            }
            Err(e) => toaster.toast(ToastIntent::Error, "Unpublish Failed", format!("{e}"), 6),
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
        let Some(nb) = flush_and_read_notebook(&state).await else {
            return;
        };
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
