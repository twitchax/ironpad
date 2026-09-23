use crate::components::collapsible_section::CollapsibleSection;
use crate::components::icons;
use leptos::prelude::*;

use crate::components::monaco_editor::MonacoEditor;
use crate::components::toaster::{ToastIntent, Toaster};
use crate::model::NotebookModel;

use super::state::{persist_with_saving_floor, NotebookState};

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
    let (icon, label) = match kind {
        SharedEditorKind::Dependencies => (icons::SHARED, "Shared Dependencies (Cargo.toml)"),
        SharedEditorKind::Source => (icons::EDIT, "Shared Source (shared.rs)"),
    };

    view! {
        <CollapsibleSection icon=icon label=label>
            <SharedEditorPanel kind=kind />
        </CollapsibleSection>
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
    let toaster = Toaster::expect_context();

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

        let meta = match kind {
            SharedEditorKind::Dependencies => ironpad_common::protocol::NotebookMetaPatch {
                shared_cargo_toml: Some(Some(content)),
                ..Default::default()
            },
            SharedEditorKind::Source => ironpad_common::protocol::NotebookMetaPatch {
                shared_source: Some(Some(content)),
                ..Default::default()
            },
        };
        let mutation = ironpad_common::protocol::Mutation::NotebookUpdateMeta { meta };

        if model
            .apply(mutation, ironpad_common::protocol::ClientId::browser())
            .is_err()
        {
            return;
        }

        let dispatch_saved_toast = move || {
            toaster.toast(
                ToastIntent::Success,
                toast_title,
                "Changes will apply on next cell compile.",
                3,
            );
        };

        persist_with_saving_floor(&state, saving, dispatch_saved_toast);
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
