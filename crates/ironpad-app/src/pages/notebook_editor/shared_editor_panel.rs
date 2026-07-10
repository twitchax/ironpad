use leptos::prelude::*;
use thaw::{
    Button, ButtonAppearance, Card, CardHeader, Toast, ToastBody, ToastTitle, ToasterInjection,
};

use crate::components::monaco_editor::MonacoEditor;
use crate::model::NotebookModel;

use super::state::{persist_notebook, NotebookState};

// ── Shared editor panel (generic) ───────────────────────────────────────────

/// Which notebook-level shared field this panel edits.
#[derive(Clone, Copy)]
pub(super) enum SharedEditorKind {
    Dependencies,
    Source,
}

/// A reusable panel for editing a notebook-level shared text field
/// (shared Cargo.toml or shared Rust source) with a Monaco editor.
#[component]
pub(super) fn SharedEditorPanel(kind: SharedEditorKind) -> impl IntoView {
    let (default_content, card_class, title_label, toast_title, language) = match kind {
        SharedEditorKind::Dependencies => (
            SHARED_DEPS_DEFAULT,
            "ironpad-shared-deps",
            "\u{2b21} Shared Dependencies (Cargo.toml)",
            "Shared dependencies saved",
            "toml",
        ),
        SharedEditorKind::Source => (
            SHARED_SOURCE_DEFAULT,
            "ironpad-shared-source-panel",
            "\u{270e} Shared Source (shared.rs)",
            "Shared source saved",
            "rust",
        ),
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
        <Card class=card_class>
            <CardHeader>
                <div class="ironpad-shared-deps-header">
                    <span class="ironpad-shared-deps-title">{title_label}</span>
                    <Button
                        appearance=ButtonAppearance::Primary
                        on_click=on_save
                        disabled=Signal::derive(move || saving.get())
                    >
                        {move || if saving.get() { "Saving\u{2026}" } else { "Save" }}
                    </Button>
                </div>
            </CardHeader>
            <div class="ironpad-shared-deps-editor-wrapper">
                <MonacoEditor
                    initial_value=editor_text.get_untracked()
                    language=language
                    on_change=Callback::new(move |val: String| {
                        editor_text.set(val);
                    })
                />
            </div>
        </Card>
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
