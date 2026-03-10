use ironpad_common::CellType;
use leptos::prelude::*;
use thaw::{Skeleton, SkeletonItem};

// ── Add cell button ─────────────────────────────────────────────────────────

/// "Add Cell" buttons (Code / Markdown), rendered between cells and at the end
/// of the list.
#[component]
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
                "+ Code"
            </button>
            <button class="ironpad-add-cell-btn ironpad-add-cell-btn--markdown" on:click=on_add_markdown>
                "+ Markdown"
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
        <Skeleton class="ironpad-cell-skeleton">
            <div class="ironpad-cell-skeleton-header">
                <SkeletonItem class="ironpad-skeleton-badge" />
                <SkeletonItem class="ironpad-skeleton-label" />
                <SkeletonItem class="ironpad-skeleton-status" />
            </div>
            <SkeletonItem class="ironpad-skeleton-editor" />
        </Skeleton>
    }
}
