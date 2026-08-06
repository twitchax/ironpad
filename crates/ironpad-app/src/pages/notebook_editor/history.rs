//! Version-history panel for private notebooks (PRD-0058).
//!
//! Lists the notebook's snapshot ring (captured at the `storage.js` save
//! choke point) and restores any entry, confirm-gated. Restore is undoable:
//! the current state is flushed, persisted, and force-snapshotted FIRST, so
//! the pre-restore version lands in history; the restore itself then writes
//! the snapshot to `IndexedDB` and hard-reloads — the same "remount is the
//! one reliable way to rebuild editor state" precedent Discard Draft uses.
//! `Local` mode only; published notebooks have draft/push semantics instead.

use leptos::prelude::*;

use crate::components::toaster::{ToastIntent, Toaster};

use super::state::NotebookState;

/// Panel-local snapshot metadata, ungated so the SSR build can type the
/// signal (the `storage::client` source type is hydrate-only).
#[derive(Clone, Debug)]
struct SnapshotMeta {
    saved_at: f64,
    title: String,
    cell_count: u32,
}

/// The history overlay: list of snapshots with per-entry Restore.
#[component]
pub(super) fn HistoryPanel(open: RwSignal<bool>) -> impl IntoView {
    let state = expect_context::<NotebookState>();
    let entries: RwSignal<Vec<SnapshotMeta>> = RwSignal::new(Vec::new());
    let loaded = RwSignal::new(false);

    // (Re)load the list every time the panel opens.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        loaded.set(false);
        #[cfg(feature = "hydrate")]
        {
            let id = state.notebook_id.get_untracked();
            leptos::task::spawn_local(async move {
                let list: Vec<SnapshotMeta> = crate::storage::client::list_history(&id)
                    .await
                    .into_iter()
                    .map(|e| SnapshotMeta {
                        saved_at: e.saved_at,
                        title: e.title,
                        cell_count: e.cell_count,
                    })
                    .collect();
                // try_: the page can be disposed while the read is in flight.
                let _ = entries.try_set(list);
                let _ = loaded.try_set(true);
            });
        }
    });

    view! {
        {move || {
            if !open.get() {
                return view! { <span /> }.into_any();
            }
            view! {
                <div class="ironpad-history-overlay" on:click=move |_| open.set(false)>
                    <div class="ironpad-history-panel" on:click=|ev| ev.stop_propagation()>
                        <div class="ironpad-history-header">
                            <span class="ironpad-history-title">"Version History"</span>
                            <button
                                class="ironpad-history-close"
                                on:click=move |_| open.set(false)
                            >
                                "✕"
                            </button>
                        </div>
                        <p class="ironpad-history-hint">
                            "Snapshots are taken as you save (at most one every five \
                             minutes) and kept on this device only. Restoring saves \
                             your current version to history first."
                        </p>
                        {move || {
                            if !loaded.get() {
                                return view! {
                                    <p class="ironpad-history-empty">"Loading…"</p>
                                }.into_any();
                            }
                            let list = entries.get();
                            if list.is_empty() {
                                return view! {
                                    <p class="ironpad-history-empty">
                                        "No snapshots yet — they appear as you edit."
                                    </p>
                                }.into_any();
                            }
                            list.into_iter().map(|entry| {
                                let when = format_age(entry.saved_at);
                                let meta = format!(
                                    "{} · {} cell{}",
                                    entry.title,
                                    entry.cell_count,
                                    if entry.cell_count == 1 { "" } else { "s" },
                                );
                                let saved_at = entry.saved_at;
                                view! {
                                    <div class="ironpad-history-entry">
                                        <div class="ironpad-history-entry-meta">
                                            <span class="ironpad-history-entry-when">{when}</span>
                                            <span class="ironpad-history-entry-title">{meta}</span>
                                        </div>
                                        <button
                                            class="ironpad-history-restore"
                                            on:click=move |_| {
                                                restore_snapshot(&state, Toaster::expect_context(), saved_at);
                                            }
                                        >
                                            "Restore"
                                        </button>
                                    </div>
                                }
                            }).collect_view().into_any()
                        }}
                    </div>
                </div>
            }
            .into_any()
        }}
    }
}

/// Human age for a snapshot timestamp (epoch ms), relative to now.
fn format_age(saved_at_ms: f64) -> String {
    #[cfg(feature = "hydrate")]
    let now_ms = js_sys::Date::now();
    #[cfg(not(feature = "hydrate"))]
    let now_ms = saved_at_ms;

    let secs = ((now_ms - saved_at_ms) / 1000.0).max(0.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let secs = secs as u64;
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

/// Confirm-gated restore: flush + persist + force-snapshot the CURRENT state
/// (the undoable half), write the chosen snapshot into `IndexedDB`, and
/// hard-reload so the editor remounts from it.
#[allow(unused_variables)]
fn restore_snapshot(state: &NotebookState, toaster: Toaster, saved_at: f64) {
    #[cfg(feature = "hydrate")]
    {
        let confirmed = web_sys::window().is_some_and(|w| {
            w.confirm_with_message(
                "Restore this snapshot? Your current version is saved to \
                 history first, so this can be undone.",
            )
            .unwrap_or(false)
        });
        if !confirmed {
            return;
        }
        let state = *state;
        leptos::task::spawn_local(async move {
            let Some(id) = state.notebook_id.try_get_untracked() else {
                return;
            };
            // Read the target snapshot FIRST, before the pre-restore writes:
            // the history ring is capped, so persisting + force-snapshotting
            // the current version can prune the oldest entry — which is the
            // one being restored when the user picks it at a full ring. Doing
            // the writes first destroyed the chosen snapshot instead of
            // restoring it.
            let Some(json) = crate::storage::client::get_history_snapshot(&id, saved_at).await
            else {
                toaster.toast(
                    ToastIntent::Error,
                    "Restore Failed",
                    "That snapshot could not be read.",
                    5,
                );
                return;
            };
            // Now flush in-progress editor content and force a pre-restore
            // snapshot, so the restore itself is undoable. Safe to prune the
            // ring now — the target JSON is already in hand.
            let _ = super::state::persist_notebook_durable(&state).await;
            crate::storage::client::snapshot_now(&id).await;
            match serde_json::from_str::<ironpad_common::IronpadNotebook>(&json) {
                Ok(nb) => {
                    if let Err(e) = crate::storage::client::save_notebook(&nb).await {
                        toaster.toast(
                            ToastIntent::Error,
                            "Restore Failed",
                            format!("Could not save the snapshot: {e:?}"),
                            5,
                        );
                        return;
                    }
                    // Remount from IndexedDB — the one reliable way to
                    // rebuild editor state (Discard Draft precedent).
                    Toaster::toast_after_reload(
                        ToastIntent::Success,
                        "Restored",
                        "The snapshot is now your current version.",
                        4,
                    );
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().reload();
                    }
                }
                Err(e) => {
                    toaster.toast(
                        ToastIntent::Error,
                        "Restore Failed",
                        format!("Snapshot is not a valid notebook: {e}"),
                        5,
                    );
                }
            }
        });
    }
}
