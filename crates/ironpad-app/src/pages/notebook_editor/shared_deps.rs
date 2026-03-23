use leptos::prelude::*;

use super::shared_editor_panel::{SharedEditorKind, SharedEditorPanel};

/// Panel for editing the notebook-level shared Cargo.toml.
#[component]
pub(super) fn SharedDepsPanel() -> impl IntoView {
    view! { <SharedEditorPanel kind=SharedEditorKind::Dependencies /> }
}
