use leptos::prelude::*;
use thaw::{Button, ButtonAppearance, Toast, ToastBody, ToastTitle, ToasterInjection};

use crate::components::monaco_editor::MonacoEditor;
use crate::model::NotebookModel;

#[cfg(not(feature = "hydrate"))]
use super::state::persist_notebook;
use super::state::NotebookState;

/// Minimum time (ms) the Saving… state stays visible: the `IndexedDB` write
/// usually completes imperceptibly fast, so the indicator is floored at
/// the longer of the actual write or this.
#[cfg(feature = "hydrate")]
const MIN_SAVING_MS: i32 = 500;

// ── Shared editor appendix (generic) ────────────────────────────────────────

/// Which notebook-level shared field this panel edits.
#[derive(Clone, Copy)]
pub(super) enum SharedEditorKind {
    Dependencies,
    Source,
}

/// One collapsed-by-default appendix section for a notebook-level shared text
/// field, rendered below the cell list — the cells are the story, the shared
/// code is the footnotes (mirroring the view-only pages). Expanding lazily
/// mounts the editor panel. Edit-mode only: view mode renders the public
/// pages' read-only appendix via `ViewOnlyNotebook`.
#[component]
pub(super) fn SharedEditorSection(kind: SharedEditorKind) -> impl IntoView {
    let label = match kind {
        SharedEditorKind::Dependencies => "\u{2b21} Shared Dependencies (Cargo.toml)",
        SharedEditorKind::Source => "\u{270e} Shared Source (shared.rs)",
    };

    let collapsed = RwSignal::new(true);
    let toggle_icon = move || if collapsed.get() { "▸" } else { "▾" };

    view! {
        <div class="view-only-shared-section">
            <button
                class="view-only-shared-header"
                on:click=move |_| collapsed.update(|c| *c = !*c)
            >
                <span class="ironpad-output-toggle">{toggle_icon}</span>
                <span>{label}</span>
            </button>
            {move || {
                (!collapsed.get())
                    .then(|| {
                        view! {
                            <div class="view-only-shared-body">
                                <SharedEditorPanel kind=kind />
                            </div>
                        }
                    })
            }}
        </div>
    }
}

/// The editor body of a shared appendix section: a Monaco editor plus (in
/// edit mode) a Save action.
#[component]
fn SharedEditorPanel(kind: SharedEditorKind) -> impl IntoView {
    let (default_content, toast_title, language) = match kind {
        SharedEditorKind::Dependencies => {
            (SHARED_DEPS_DEFAULT, "Shared dependencies saved", "toml")
        }
        SharedEditorKind::Source => (SHARED_SOURCE_DEFAULT, "Shared source saved", "rust"),
    };

    let state = expect_context::<NotebookState>();
    let model = expect_context::<NotebookModel>();
    let toaster = ToasterInjection::expect_context();

    let initial_value = match kind {
        SharedEditorKind::Dependencies => state.shared_cargo_toml.get_untracked(),
        SharedEditorKind::Source => state.shared_source.get_untracked(),
    };

    let editor_text = RwSignal::new(initial_value.unwrap_or_else(|| default_content.to_string()));
    let saving = RwSignal::new(false);

    let on_save = move |_| {
        // A save is already in flight; the button is disabled, but guard
        // against programmatic double-fires too.
        if saving.get_untracked() {
            return;
        }
        let content = editor_text.get_untracked();

        let mutation = match kind {
            SharedEditorKind::Dependencies => {
                ironpad_common::protocol::Mutation::NotebookUpdateMeta {
                    title: None,
                    shared_cargo_toml: Some(Some(content)),
                    shared_source: None,
                    reactive_mode: None,
                }
            }
            SharedEditorKind::Source => ironpad_common::protocol::Mutation::NotebookUpdateMeta {
                title: None,
                shared_cargo_toml: None,
                shared_source: Some(Some(content)),
                reactive_mode: None,
            },
        };

        if model
            .apply(mutation, ironpad_common::protocol::ClientId::browser())
            .is_err()
        {
            return;
        }

        let dispatch_saved_toast = move || {
            toaster.dispatch_toast(
                move || {
                    view! {
                        <Toast>
                            <ToastTitle>{toast_title}</ToastTitle>
                            <ToastBody>"Changes will apply on next cell compile."</ToastBody>
                        </Toast>
                    }
                },
                thaw::ToastOptions::default()
                    .with_intent(thaw::ToastIntent::Success)
                    .with_timeout(std::time::Duration::from_secs(3)),
            );
        };

        // Await the actual IndexedDB write (so the toast means "durably
        // saved", not "save dispatched"), and floor the Saving… indicator
        // at MIN_SAVING_MS so the feedback is perceptible — the write is
        // usually far faster than a blink.
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
                // The panel may have been disposed mid-save (navigation,
                // section collapse); try_set keeps this continuation
                // panic-free. The toaster is app-level and outlives us,
                // and the save DID land, so the toast stays truthful.
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
            <div class="ironpad-shared-deps-editor-wrapper">
                <MonacoEditor
                    initial_value=editor_text.get_untracked()
                    language=language
                    on_change=Callback::new(move |val: String| {
                        editor_text.set(val);
                    })
                />
            </div>
            <div class="ironpad-shared-editor-actions">
                <Button
                    appearance=ButtonAppearance::Primary
                    on_click=on_save
                    disabled=Signal::derive(move || saving.get())
                >
                    {move || if saving.get() { "Saving\u{2026}" } else { "Save" }}
                </Button>
            </div>
        </div>
    }
}

// ── Default content ─────────────────────────────────────────────────────────

const SHARED_DEPS_DEFAULT: &str = "\
[dependencies]
# Add shared dependencies here.
# These will be available in all cells.
# Cell-level dependencies override shared ones.

[profile.release]
# Optimized for fast compilation (interactive notebook use).
opt-level = 1
lto = false
codegen-units = 16
";

const SHARED_SOURCE_DEFAULT: &str = "\
// Shared source module.
// Code here is available in all cells as `shared::*`.
// Example:
//   pub fn greet(name: &str) -> String {
//       format!(\"Hello, {name}!\")
//   }
";
