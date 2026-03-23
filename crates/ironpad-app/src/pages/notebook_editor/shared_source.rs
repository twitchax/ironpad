use leptos::prelude::*;

use super::shared_editor_panel::{SharedEditorKind, SharedEditorPanel};

/// Panel for editing the notebook-level shared Rust source (`src/shared.rs`).
#[component]
pub(super) fn SharedSourcePanel() -> impl IntoView {
    view! { <SharedEditorPanel kind=SharedEditorKind::Source /> }
}
