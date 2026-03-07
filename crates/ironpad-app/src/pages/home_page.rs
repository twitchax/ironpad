use ironpad_common::NotebookSummary;
use leptos::prelude::*;
use leptos_router::{hooks::use_navigate, NavigateOptions};
use thaw::{Button, ButtonAppearance, Card, CardHeader, Spinner};

use crate::components::app_layout::LayoutContext;
use crate::server_fns::{create_notebook, list_notebooks};

// ── Home page ───────────────────────────────────────────────────────────────

/// Home page showing all notebooks with a create button.
#[component]
pub fn HomePage() -> impl IntoView {
    // Reset layout context for home page.

    let ctx = expect_context::<LayoutContext>();
    ctx.notebook_title.set(None);
    ctx.show_save_button.set(false);
    ctx.cell_count.set(0);
    ctx.last_save_time.set(None);

    // Load notebook list.

    let notebooks = Resource::new(|| (), |_| list_notebooks());

    // Create-notebook action.

    let create_action =
        Action::new(|_: &()| async move { create_notebook("Untitled Notebook".to_string()).await });

    // Navigate to newly created notebook on success.

    let navigate = use_navigate();
    Effect::new(move || {
        if let Some(Ok(manifest)) = create_action.value().get() {
            navigate(
                &format!("/notebook/{}", manifest.id),
                NavigateOptions::default(),
            );
        }
    });

    view! {
        <div class="ironpad-home">
            <div class="ironpad-home-header">
                <h1>"Your Notebooks"</h1>
                <Button
                    appearance=ButtonAppearance::Primary
                    on_click=move |_| { create_action.dispatch(()); }
                >
                    "+ New Notebook"
                </Button>
            </div>

            <Suspense fallback=move || view! {
                <div class="ironpad-home-loading">
                    <Spinner label="Loading notebooks..." />
                </div>
            }>
                {move || Suspend::new(async move {
                    match notebooks.await {
                        Ok(list) if list.is_empty() => view! {
                            <div class="ironpad-home-empty">
                                <p>"No notebooks yet."</p>
                                <p>"Create one to get started!"</p>
                            </div>
                        }.into_any(),

                        Ok(list) => view! {
                            <div class="ironpad-notebook-grid">
                                {list.into_iter().map(|nb| {
                                    view! { <NotebookCard summary=nb /> }
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_any(),

                        Err(e) => view! {
                            <p class="ironpad-error">
                                {format!("Failed to load notebooks: {e}")}
                            </p>
                        }.into_any(),
                    }
                })}
            </Suspense>
        </div>
    }
}

// ── Notebook card ───────────────────────────────────────────────────────────

/// A single notebook card for the home page grid.
#[component]
fn NotebookCard(summary: NotebookSummary) -> impl IntoView {
    let href = format!("/notebook/{}", summary.id);
    let cell_label = if summary.cell_count == 1 {
        "cell"
    } else {
        "cells"
    };
    let cell_text = format!("{} {}", summary.cell_count, cell_label);
    let updated = summary
        .updated_at
        .format("%b %d, %Y at %H:%M UTC")
        .to_string();

    view! {
        <a href=href class="ironpad-notebook-card-link">
            <Card class="ironpad-notebook-card">
                <CardHeader>
                    <span class="ironpad-notebook-card-title">{summary.title}</span>
                </CardHeader>
                <div class="ironpad-notebook-card-body">
                    <span class="ironpad-notebook-card-cells">{cell_text}</span>
                    <span class="ironpad-notebook-card-updated">{updated}</span>
                </div>
            </Card>
        </a>
    }
}
