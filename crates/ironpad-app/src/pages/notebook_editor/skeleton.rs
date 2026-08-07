use ironpad_common::CellType;
use leptos::prelude::*;

use crate::components::icon::IconLabel;
use crate::components::icons;

// ── Add cell button ─────────────────────────────────────────────────────────

/// "Add Cell" buttons (Code / Markdown), rendered between cells and at the end
/// of the list.
#[component]
#[allow(clippy::needless_pass_by_value)]
pub(super) fn AddCellButton(
    after_cell_id: Option<String>,
    on_add: Callback<(Option<String>, CellType)>,
) -> impl IntoView {
    let after_code = after_cell_id.clone();
    let after_md = after_cell_id.clone();
    let on_add_code = move |_| {
        on_add.run((after_code.clone(), CellType::Code));
    };
    let on_add_markdown = move |_| {
        on_add.run((after_md.clone(), CellType::Markdown));
    };

    view! {
        <div class="ironpad-add-cell-row">
            <button class="ironpad-add-cell-btn" on:click=on_add_code>
                <IconLabel icon=icons::ADD label="Code"/>
            </button>
            <button class="ironpad-add-cell-btn ironpad-add-cell-btn--markdown" on:click=on_add_markdown>
                <IconLabel icon=icons::ADD label="Markdown"/>
            </button>
        </div>
    }
}

// ── Notebook editor skeleton ────────────────────────────────────────────────

/// Skeleton placeholder shown while the notebook is loading.
#[component]
pub(super) fn NotebookEditorSkeleton() -> impl IntoView {
    view! {
        <div class="ironpad-cell-list">
            <CellSkeleton />
            <CellSkeleton />
        </div>
    }
}

/// Skeleton placeholder for a single cell card.
#[component]
fn CellSkeleton() -> impl IntoView {
    view! {
        <div class="ironpad-cell-skeleton">
            <div class="ironpad-cell-skeleton-header">
                <div class="ironpad-skeleton-item ironpad-skeleton-badge" />
                <div class="ironpad-skeleton-item ironpad-skeleton-label" />
                <div class="ironpad-skeleton-item ironpad-skeleton-status" />
            </div>
            <div class="ironpad-skeleton-item ironpad-skeleton-editor" />
        </div>
    }
}
