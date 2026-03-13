use leptos::prelude::*;
use thaw::{
    Button, ButtonAppearance, Card, CardHeader, Toast, ToastBody, ToastTitle, ToasterInjection,
};

use crate::components::monaco_editor::MonacoEditor;
use crate::model::NotebookModel;

use super::state::{persist_notebook, NotebookState};

// ── Shared dependencies panel ───────────────────────────────────────────────

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

/// Panel for editing the notebook-level shared Cargo.toml.
#[component]
pub(super) fn SharedDepsPanel() -> impl IntoView {
    let state = expect_context::<NotebookState>();
    let model = expect_context::<NotebookModel>();
    let toaster = ToasterInjection::expect_context();

    let editor_text = RwSignal::new(
        state
            .shared_cargo_toml
            .get_untracked()
            .unwrap_or_else(|| SHARED_DEPS_DEFAULT.to_string()),
    );
    let saving = RwSignal::new(false);

    let on_save = move |_| {
        let content = editor_text.get_untracked();

        // Update via the model (handles stale marking; the notebook Effect
        // syncs the convenience signals automatically).
        if model
            .apply(
                ironpad_common::protocol::Mutation::NotebookUpdateMeta {
                    title: None,
                    shared_cargo_toml: Some(Some(content)),
                    shared_source: None,
                },
                ironpad_common::protocol::ClientId::browser(),
            )
            .is_ok()
        {
            persist_notebook(&state);
        }

        let toaster = toaster;
        toaster.dispatch_toast(
            move || {
                view! {
                    <Toast>
                        <ToastTitle>"Shared dependencies saved"</ToastTitle>
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
        <Card class="ironpad-shared-deps">
            <CardHeader>
                <div class="ironpad-shared-deps-header">
                    <span class="ironpad-shared-deps-title">"📦 Shared Dependencies (Cargo.toml)"</span>
                    <Button
                        appearance=ButtonAppearance::Primary
                        on_click=on_save
                        disabled=Signal::derive(move || saving.get())
                    >
                        {move || if saving.get() { "Saving…" } else { "Save" }}
                    </Button>
                </div>
            </CardHeader>
            <div class="ironpad-shared-deps-editor-wrapper">
                <MonacoEditor
                    initial_value=editor_text.get_untracked()
                    language="toml"
                    on_change=Callback::new(move |val: String| {
                        editor_text.set(val);
                    })
                />
            </div>
        </Card>
    }
}
