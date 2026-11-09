//! Notebook lifecycle workflows: Share Immutable, Save to Account, Share
//! Mutable, Push, Discard Draft, Unpublish, Delete (both storage classes),
//! Export HTML, and Download .ironpad.
//!
//! Extracted from the editor component (`mod.rs`) so every serialize-flow
//! shares the same flush discipline (PRD-0032 T-007) — the one inline flow
//! that predated this module (Unpublish) was also the one that missed the
//! flush and could drop the last debounce-window of typing. The toolbar's
//! menu items only call into here: no workflow is written in view code.

use leptos::prelude::*;

use crate::components::toaster::{ToastIntent, Toaster};
use crate::server_fns::share_notebook;

use super::state::NotebookState;

// ── Flush + serialize ───────────────────────────────────────────────────────

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
    let mut nb = state.flush_and_read().await?;
    // Embed the author's last-run outputs into the OUTGOING copy only
    // (PRD-0056): the model and the debounced autosaves stay lean.
    if let Some(texts) = state.cell_display_texts.try_get_untracked() {
        super::state::embed_saved_outputs_within_cap(&mut nb, &texts);
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
    let tags = state
        .cell_outputs
        .try_with_untracked(|outputs| share_tags(&nb.cells, outputs))
        .unwrap_or_else(|| vec![String::new(); nb.cells.len()]);
    Some((json, tags))
}

/// Positional type tags for a share or Push (one per cell, empty when
/// unrun/non-code), which let the server snapshot compiled blobs.
///
/// The server keys cell `i` with `tags[..i]`, so these must be EXACTLY the
/// tags the compile path cache-keyed with: THE shared projection
/// (`previous_cell_types`) over the whole notebook, never a hand-rolled copy
/// that could drift from it (the old one skipped its `is_code` gate).
fn share_tags<C: crate::components::executor::PipingCell, S: std::hash::BuildHasher>(
    cells: &[C],
    outputs: &std::collections::HashMap<String, super::state::CellOutputData, S>,
) -> Vec<String> {
    crate::components::executor::previous_cell_types(cells, cells.len(), outputs)
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

/// What moving a notebook into the account costs, said by the confirm of
/// both moves (Save to Account, Share Mutable).
///
/// Both are confirm-gated because the move is lossy in a way nothing else on
/// the page says: `deleteNotebook` takes the notebook's version-history ring
/// (PRD-0058, up to 30 snapshots) with it, server-side history is an explicit
/// PRD-0064 non-goal, and Unpublish no longer hands a notebook back to
/// `/local`. So the snapshots are gone at the moment of the click, and the
/// confirm is the only place a user can still decide otherwise. A macro
/// rather than a const, so `concat!` can build each confirm from it.
#[cfg(any(feature = "hydrate", test))]
macro_rules! move_loses_local_history {
    () => {
        "The copy in this browser is deleted, and its local version history goes \
         with it: snapshots are a local-notebook feature and do not follow it to \
         the server."
    };
}

/// Confirm copy for Save to Account.
#[cfg(any(feature = "hydrate", test))]
pub(super) const SAVE_TO_ACCOUNT_CONFIRM: &str = concat!(
    "Move this notebook into your account? ",
    move_loses_local_history!(),
    " Your notebook stays private until you publish it."
);

/// Confirm copy for Share Mutable: the same move, plus a publish.
#[cfg(any(feature = "hydrate", test))]
pub(super) const SHARE_MUTABLE_CONFIRM: &str = concat!(
    "Publish this notebook at a link anyone can open? It moves into your account. ",
    move_loses_local_history!()
);

/// The toast after a move into the account: the flow's own success, or, when
/// the local copy survived its delete, the leftover-copy warning instead.
#[cfg(any(feature = "hydrate", test))]
fn move_outcome(
    local_copy_survived: bool,
    moved_title: &'static str,
    moved_body: String,
) -> (ToastIntent, &'static str, String) {
    if local_copy_survived {
        (
            ToastIntent::Info,
            "Saved, With a Leftover Copy",
            "The notebook is in your account, but the copy in this browser could \
             not be removed. Delete it from the home page so you do not end up \
             editing two of them."
                .to_string(),
        )
    } else {
        (ToastIntent::Success, moved_title, moved_body)
    }
}

/// The second half of both moves into the account, once the server holds the
/// notebook: delete the local copy, check that the delete took, and
/// hard-navigate to `/mutable/{share_id}` (the URL and storage class both
/// change) with the outcome toast riding sessionStorage across it.
///
/// Move, never copy, but only claim the move happened: a delete that fails
/// leaves a second, silently diverging copy of the notebook on the home
/// page, which is exactly the reconciliation problem PRD-0054 removed, so it
/// is named rather than swallowed. (`delete_notebook` logs and returns
/// nothing, so the surviving record is the only evidence.) The caller halts
/// persistence first, or a late save would write the record straight back.
#[cfg(feature = "hydrate")]
async fn finish_move_to_account(
    uuid: &str,
    share_id: &str,
    moved_title: &'static str,
    moved_body: String,
) {
    crate::storage::client::delete_notebook(uuid).await;
    // Only a clean "no such record" proves the delete: a read that fails
    // says nothing, so it counts as a survivor.
    let local_copy_survived = !matches!(crate::storage::client::get_notebook(uuid).await, Ok(None));
    if local_copy_survived {
        leptos::logging::warn!(
            "local notebook {uuid} survived the delete after moving it to the account"
        );
    }
    let (intent, title, body) = move_outcome(local_copy_survived, moved_title, moved_body);
    Toaster::toast_after_reload(intent, title, body, 6);
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href(&format!("/mutable/{share_id}"));
    }
}

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
    if !crate::components::dialog::confirm(SAVE_TO_ACCOUNT_CONFIRM) {
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
                // Halted first: a save debounce firing after the delete
                // would write the record straight back.
                state.halt_persistence();
                finish_move_to_account(
                    &uuid,
                    &share_id,
                    "Saved to Your Account",
                    "Only you can see it until you publish it.".to_string(),
                )
                .await;
            }
            Err(e) => toaster.toast(ToastIntent::Error, "Save Failed", format!("{e}"), 6),
        }
    });
}

// ── Mutable shares (PRD-0049 / PRD-0054) ────────────────────────────────────

/// Convert the current notebook into a mutable share (PRD-0054): confirm,
/// create the share server-side under the signed-in account, DELETE the
/// local copy (the notebook now lives at its public URL), and hard-navigate
/// to `/mutable/{id}`, which mounts the `ServerDraft` editor. The same move
/// as Save to Account, with the same confirm about what it costs.
pub(super) fn share_mutable_current_notebook(state: &NotebookState, toaster: Toaster) {
    let state = *state;
    // The confirm is browser-only; this fn also compiles for the server.
    #[cfg(feature = "hydrate")]
    if !crate::components::dialog::confirm(SHARE_MUTABLE_CONFIRM) {
        return;
    }
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
                    // would only drift (PRD-0054). Halted first, so a late
                    // save cannot write it back.
                    state.halt_persistence();
                    let url = format!("{}/mutable/{share_id}", window_origin());
                    copy_to_clipboard(&url).await;
                    finish_move_to_account(&uuid, &share_id, "Published! Link Copied", url).await;
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
        if !super::state::persist_notebook_durable(&state, true).await {
            // A REFUSED draft (PRD-0064) is not a retry away from working, so
            // the advice differs: whatever blocks the autosave blocks the
            // push, and telling the owner to wait for an indicator that will
            // never clear is how this failure became undiagnosable.
            let body = match state.draft_save_state.try_get_untracked() {
                Some(super::state::DraftSaveState::Refused(kind)) => kind.push_failed_body(),
                _ => {
                    "Your draft could not be saved to the server, so nothing was \
                     published. It will keep retrying; push again once the draft \
                     indicator clears."
                }
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
    if !crate::components::dialog::confirm(
        "Discard your draft and return to the published copy? \
         Unpushed changes will be lost.",
    ) {
        return;
    }
    // Before the POST, synchronously: every save debounce that came due
    // behind the blocking confirm fires the moment this handler returns, and
    // a draft save landing after the discard would put the discarded edits
    // back under a "Draft Discarded" toast. The halt stops new writes (and a
    // cell debounce that fires mid-discard), the epoch bump retires the
    // scheduled ones, and the drain below waits out one already on the wire.
    state.halt_persistence();
    let _ = state.draft_save_epoch.try_update_untracked(|e| *e += 1);
    let state = *state;
    // The page stays on a failure, so it must go back to saving, including
    // the edits whose scheduled save the epoch bump retired.
    let resume = move || {
        state.resume_persistence();
        if state.draft_dirty.try_get_untracked() == Some(true) {
            super::state::persist_notebook(&state);
        }
    };
    leptos::task::spawn_local(async move {
        if !super::state::drain_inflight_drafts(&state, None).await {
            return;
        }
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
            Ok(false) => {
                resume();
                toaster.toast(
                    ToastIntent::Success,
                    "Nothing to Discard",
                    "This notebook has no published copy to return to.",
                    5,
                );
            }
            Err(e) => {
                resume();
                toaster.toast(ToastIntent::Error, "Discard Failed", format!("{e}"), 6);
            }
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
    if !crate::components::dialog::confirm(
        "Unpublish this notebook? Its link stops working for readers. It \
         stays in your account, and you can publish it again.",
    ) {
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

/// Delete a local notebook: confirm, remove its `IndexedDB` record (its
/// version-history ring goes with it, PRD-0058), then go home. The Local-mode
/// twin of [`delete_mutable_current_notebook`].
#[cfg(feature = "hydrate")]
pub(super) fn delete_local_current_notebook(
    state: &NotebookState,
    navigate: impl Fn(&str, leptos_router::NavigateOptions) + 'static,
) {
    let Some(id) = state.notebook_id.try_get_untracked() else {
        return;
    };
    if !crate::components::dialog::confirm(crate::components::dialog::DELETE_NOTEBOOK_CONFIRM) {
        return;
    }
    // Synchronously, before anything else runs: a cell's save debounce that
    // came due behind the blocking confirm fires the moment this handler
    // returns, and its write would land after the delete and resurrect the
    // record.
    state.halt_persistence();
    leptos::task::spawn_local(async move {
        crate::storage::client::delete_notebook(&id).await;
        navigate("/", leptos_router::NavigateOptions::default());
    });
}

/// Delete an account notebook (PRD-0064): the share, its draft, its published
/// copy, and its grants all go, then home. Local mode's Delete removes the
/// `IndexedDB` record; this is the same act for the other storage class, and
/// it is the only destructive one now that Unpublish leaves the notebook in
/// place.
#[cfg(feature = "hydrate")]
pub(super) fn delete_mutable_current_notebook(toaster: Toaster, share_id: String) {
    if !crate::components::dialog::confirm(
        "Delete this notebook from your account? Its link stops working \
         and this cannot be undone. Download .ironpad first if you want \
         to keep a copy.",
    ) {
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

// ── Export HTML ─────────────────────────────────────────────────────────────

/// Export the current notebook, with its last-run outputs, as a standalone
/// HTML file. Flushes first, so "type then immediately Export" does not
/// produce a stale artifact (PRD-0032 T-007).
#[cfg(feature = "hydrate")]
pub(super) fn export_html_current_notebook(state: &NotebookState) {
    let state = *state;
    leptos::task::spawn_local(async move {
        let Some(nb) = state.flush_and_read().await else {
            return;
        };
        // try_: disposal can land during the flush's yield.
        let Some(display_texts) = state.cell_display_texts.try_get_untracked() else {
            return;
        };
        let html = super::export::build_export_html(&nb, &display_texts);
        super::export::trigger_html_download(&html, &nb.title);
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
        let Some(mut nb) = state.flush_and_read().await else {
            return;
        };
        // Downloads carry the outputs too (PRD-0056) — this is how a public
        // blog notebook gets committed outputs.
        if let Some(texts) = state.cell_display_texts.try_get_untracked() {
            super::state::embed_saved_outputs_within_cap(&mut nb, &texts);
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

    use std::collections::HashMap;

    use ironpad_common::{CellManifest, CellType};

    use crate::components::executor::previous_cell_types;
    use crate::components::output_render::CellOutputData;

    fn manifest(id: &str, cell_type: CellType, shared: bool) -> CellManifest {
        CellManifest {
            id: id.to_string(),
            order: 0,
            label: id.to_string(),
            cell_type,
            shared,
            collapsed: false,
            output_collapsed: false,
        }
    }

    fn tagged(tag: &str) -> CellOutputData {
        CellOutputData {
            bytes: vec![1],
            type_tag: Some(tag.to_string()),
        }
    }

    /// The server snapshots cell `i` under the key it hashes from
    /// `tags[..i]`; a share tag that differs from what the compile path
    /// cache-keyed with makes every snapshot miss. Every prefix must be the
    /// compile path's `previous_cell_types` for that cell.
    #[test]
    fn share_tags_prefixes_are_the_compile_path_projection() {
        let cells = vec![
            manifest("code", CellType::Code, false),
            // Only Code cells ever carry a tag: an output entry on anything
            // else is exactly what a hand-rolled projection would leak.
            manifest("md", CellType::Markdown, false),
            manifest("linux", CellType::Linux, false),
            manifest("shared", CellType::Code, true),
            manifest("unrun", CellType::Code, false),
            manifest("last", CellType::Code, false),
        ];
        let outputs: HashMap<String, CellOutputData> = [
            ("code", tagged("u32")),
            ("md", tagged("leaked")),
            ("linux", tagged("leaked")),
            ("last", tagged("String")),
        ]
        .into_iter()
        .map(|(id, data)| (id.to_string(), data))
        .collect();

        let tags = share_tags(&cells, &outputs);
        assert_eq!(tags.len(), cells.len(), "one tag per cell");
        for i in 0..=cells.len() {
            assert_eq!(
                tags[..i],
                previous_cell_types(&cells, i, &outputs)[..],
                "prefix {i} forks from the compile path's cache key"
            );
        }
        assert_eq!(tags, ["u32", "", "", "", "", "String"]);
    }

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

    /// Share Mutable deletes the same history ring through the same
    /// `deleteNotebook`, so its confirm has to say so too.
    #[test]
    fn share_mutable_confirm_names_the_history_loss() {
        let copy = SHARE_MUTABLE_CONFIRM.to_ascii_lowercase();
        assert!(copy.contains("version history"), "{copy}");
        assert!(copy.contains("deleted"), "{copy}");
        assert!(copy.contains("publish"), "{copy}");
    }

    /// User-visible strings carry no em-dashes (repo-wide style pass).
    #[test]
    fn save_to_account_confirm_has_no_em_dash() {
        assert!(!SAVE_TO_ACCOUNT_CONFIRM.contains('—'));
        assert!(!SHARE_MUTABLE_CONFIRM.contains('—'));
    }

    #[test]
    fn a_move_toasts_its_own_success() {
        let (intent, title, body) = move_outcome(
            false,
            "Published! Link Copied",
            "https://x/mutable/1".into(),
        );
        assert_eq!(intent, ToastIntent::Success);
        assert_eq!(title, "Published! Link Copied");
        assert_eq!(body, "https://x/mutable/1");
    }

    #[test]
    fn a_surviving_local_copy_is_named_whichever_move_left_it() {
        for title in ["Saved to Your Account", "Published! Link Copied"] {
            let (intent, shown, body) = move_outcome(true, title, "unused".into());
            assert_eq!(intent, ToastIntent::Info);
            assert_eq!(shown, "Saved, With a Leftover Copy");
            assert!(body.contains("home page"), "{body}");
        }
    }
}
