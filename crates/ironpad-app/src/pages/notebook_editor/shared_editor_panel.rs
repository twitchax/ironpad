use crate::components::collapsible_section::CollapsibleSection;
use crate::components::icons;
use leptos::prelude::*;

use crate::components::monaco_editor::{MonacoEditor, MonacoEditorHandle};
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

    // The notebook field this panel edits.
    let field = match kind {
        SharedEditorKind::Dependencies => state.shared_cargo_toml,
        SharedEditorKind::Source => state.shared_source,
    };

    let editor_text = RwSignal::new(
        field
            .get_untracked()
            .unwrap_or_else(|| default_content.to_string()),
    );
    let saving = RwSignal::new(false);
    // Typed text the model does not have yet. The notebook's own flushes
    // (Ctrl+S, Share, Push, Preview) and a collapse commit it, as well as
    // Save; nothing commits while it is clear, so opening the panel never
    // writes the default template into a notebook.
    let dirty = RwSignal::new(false);
    let editor_handle: RwSignal<Option<MonacoEditorHandle>> = RwSignal::new(None);

    // Apply the editor's text to the model; whether it took.
    let commit = move || {
        let Some(content) = editor_text.try_get_untracked() else {
            return false;
        };
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
        let applied = model
            .apply(mutation, ironpad_common::protocol::ClientId::browser())
            .is_ok();
        if applied {
            let _ = dirty.try_set(false);
        }
        applied
    };

    #[cfg(feature = "hydrate")]
    {
        // Every notebook flush commits typed text, the way it commits each
        // cell's unsaved panes. Whoever flushed persists; if the page goes
        // first, the editor's teardown write does.
        let prev_gen = RwSignal::new(state.save_generation.get_untracked());
        Effect::new(move || {
            let gen = state.save_generation.get();
            if gen == prev_gen.get_untracked() {
                return;
            }
            prev_gen.set(gen);
            if dirty.get_untracked() && commit() {
                state.mark_persist_pending();
            }
        });

        // Collapsing the section unmounts this panel: commit and persist the
        // typed text rather than dropping it. (Preview unmounts it too, after
        // its flush has already committed.)
        on_cleanup(move || {
            if dirty.try_get_untracked() == Some(true) && commit() {
                super::state::persist_notebook(&state);
            }
        });

        // An edit from elsewhere (an agent's `notebook update`) reaches the
        // mounted editor, unless the author has typed over it.
        Effect::new(move || {
            let latest = field.get().unwrap_or_else(|| default_content.to_string());
            if dirty.get_untracked() {
                return;
            }
            super::cell_item::refresh_pane(editor_text, editor_handle.get_untracked(), latest);
        });
    }

    let on_change = Callback::new(move |val: String| {
        // Dirty only when it differs from the model: the refresh above writes
        // the model's own value into the buffer, which is not an edit.
        let differs = field.with_untracked(|f| f.as_deref().unwrap_or(default_content) != val);
        dirty.set(differs);
        editor_text.set(val);
    });

    let on_save = move |_| {
        // A save is already in flight; the button is disabled, but guard
        // against programmatic double-fires too.
        if saving.get_untracked() {
            return;
        }
        if !commit() {
            return;
        }

        let dispatch_saved_toast = move |landed: bool| {
            if !landed {
                state.toast_not_saved();
                return;
            }
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
                    on_change=on_change
                    handle=editor_handle
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
