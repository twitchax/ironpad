use ironpad_common::CellType;
use leptos::prelude::*;

use crate::components::icon::IconLabel;
use crate::components::icons;

// ── Add cell button ─────────────────────────────────────────────────────────

/// "Add Cell" buttons (Code / Markdown / Linux), rendered between cells and at
/// the end of the list.
///
/// Every button carries its own `--<kind>` modifier class, including Code.
/// The e2e suite selects these by class rather than by label (the prose moved
/// once already and took 52 specs with it), and Code used to be "the one
/// without `--markdown`" — a negation that a third kind silently broadens
/// into "Code or Linux". A positive class per kind cannot be widened by
/// adding a fourth.
#[component]
#[allow(clippy::needless_pass_by_value)]
pub(super) fn AddCellButton(
    after_cell_id: Option<String>,
    on_add: Callback<(Option<String>, CellType)>,
) -> impl IntoView {
    let after_code = after_cell_id.clone();
    let after_md = after_cell_id.clone();
    let after_linux = after_cell_id.clone();
    let on_add_code = move |_| {
        on_add.run((after_code.clone(), CellType::Code));
    };
    let on_add_markdown = move |_| {
        on_add.run((after_md.clone(), CellType::Markdown));
    };
    let on_add_linux = move |_| {
        on_add.run((after_linux.clone(), CellType::Linux));
    };

    view! {
        <div class="ironpad-add-cell-row">
            <button class="ironpad-add-cell-btn ironpad-add-cell-btn--code" on:click=on_add_code>
                <IconLabel icon=icons::ADD label="Code"/>
            </button>
            <button class="ironpad-add-cell-btn ironpad-add-cell-btn--markdown" on:click=on_add_markdown>
                <IconLabel icon=icons::ADD label="Markdown"/>
            </button>
            <button
                class="ironpad-add-cell-btn ironpad-add-cell-btn--linux"
                title="A whole Rust program run as a Linux process in the browser"
                on:click=on_add_linux
            >
                <IconLabel icon=icons::ADD label="Linux"/>
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
