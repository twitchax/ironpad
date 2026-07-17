use leptos::prelude::*;
use thaw::{Button, ButtonAppearance, Toast, ToastBody, ToastTitle, ToasterInjection};

use crate::components::monaco_editor::MonacoEditor;
use crate::model::NotebookModel;

use super::state::{persist_notebook, NotebookState};

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
/// mounts the editor panel; in view mode the section is read-only and hidden
/// entirely when the field is empty, matching the public/shared appendix.
#[component]
pub(super) fn SharedEditorSection(kind: SharedEditorKind) -> impl IntoView {
    let state = expect_context::<NotebookState>();

    let (label, content_signal) = match kind {
        SharedEditorKind::Dependencies => (
            "\u{2b21} Shared Dependencies (Cargo.toml)",
            state.shared_cargo_toml,
        ),
        SharedEditorKind::Source => ("\u{270e} Shared Source (shared.rs)", state.shared_source),
    };

    let collapsed = RwSignal::new(true);
    let toggle_icon = move || if collapsed.get() { "▸" } else { "▾" };

    // Edit mode always shows the section (it's how shared code gets added in
    // the first place); view mode only shows it when there is content.
    let visible = move || {
        !state.is_view_mode.get()
            || content_signal
                .get()
                .is_some_and(|content| !content.trim().is_empty())
    };

    view! {
        <Show when=visible>
            <div class="view-only-shared-section">
                <button
                    class="view-only-shared-header"
                    on:click=move |_| collapsed.update(|c| *c = !*c)
                >
                    <span class="ironpad-output-toggle">{toggle_icon}</span>
                    <span>{label}</span>
                </button>
                // Reading is_view_mode here re-mounts the panel on mode
                // switches, so it picks up the right read-only state and the
                // freshest content in one place.
                {move || {
                    (!collapsed.get())
                        .then(|| {
                            view! {
                                <div class="view-only-shared-body">
                                    <SharedEditorPanel
                                        kind=kind
                                        read_only=state.is_view_mode.get()
                                    />
                                </div>
                            }
                        })
                }}
            </div>
        </Show>
    }
}

/// The editor body of a shared appendix section: a Monaco editor plus (in
/// edit mode) a Save action.
#[component]
fn SharedEditorPanel(kind: SharedEditorKind, read_only: bool) -> impl IntoView {
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
        let content = editor_text.get_untracked();

        let mutation = match kind {
            SharedEditorKind::Dependencies => {
                ironpad_common::protocol::Mutation::NotebookUpdateMeta {
                    title: None,
                    shared_cargo_toml: Some(Some(content)),
                    shared_source: None,
                    reactive_mode: None,
                    expand_code: None,
                }
            }
            SharedEditorKind::Source => ironpad_common::protocol::Mutation::NotebookUpdateMeta {
                title: None,
                shared_cargo_toml: None,
                shared_source: Some(Some(content)),
                reactive_mode: None,
                expand_code: None,
            },
        };

        if model
            .apply(mutation, ironpad_common::protocol::ClientId::browser())
            .is_ok()
        {
            persist_notebook(&state);
        }

        let toaster = toaster;
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

    view! {
        <div class="ironpad-shared-editor-body">
            <div class="ironpad-shared-deps-editor-wrapper">
                <MonacoEditor
                    initial_value=editor_text.get_untracked()
                    language=language
                    on_change=Callback::new(move |val: String| {
                        editor_text.set(val);
                    })
                    read_only=read_only
                />
            </div>
            {(!read_only)
                .then(|| {
                    view! {
                        <div class="ironpad-shared-editor-actions">
                            <Button
                                appearance=ButtonAppearance::Primary
                                on_click=on_save
                                disabled=Signal::derive(move || saving.get())
                            >
                                {move || if saving.get() { "Saving\u{2026}" } else { "Save" }}
                            </Button>
                        </div>
                    }
                })}
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
