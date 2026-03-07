use leptos::prelude::*;
use thaw::{Button, ButtonAppearance, Layout, LayoutHeader, LayoutPosition};

// ── Layout context ──────────────────────────────────────────────────────────

/// Shared reactive state between the layout shell and child pages.
///
/// Child components (e.g., the notebook editor) update these signals to
/// reflect the current page state in the header and status bar.
#[derive(Clone, Copy)]
pub struct LayoutContext {
    /// Notebook title shown in the header center. `None` when on the home page.
    pub notebook_title: RwSignal<Option<String>>,
    /// Whether to show the save button in the header.
    pub show_save_button: RwSignal<bool>,
    /// Fires when the user clicks the save button. Child pages watch this.
    pub save_generation: RwSignal<u64>,
    /// Total cell count displayed in the status bar.
    pub cell_count: RwSignal<usize>,
    /// Human-readable "last saved" timestamp for the status bar.
    pub last_save_time: RwSignal<Option<String>>,
    /// Compiler/toolchain version string.
    pub compiler_version: RwSignal<String>,
}

impl LayoutContext {
    fn new() -> Self {
        Self {
            notebook_title: RwSignal::new(None),
            show_save_button: RwSignal::new(false),
            save_generation: RwSignal::new(0),
            cell_count: RwSignal::new(0),
            last_save_time: RwSignal::new(None),
            compiler_version: RwSignal::new("stable".to_string()),
        }
    }
}

// ── App layout ──────────────────────────────────────────────────────────────

/// Top-level app layout: header, scrollable content area, and status bar.
#[component]
pub fn AppLayout(children: Children) -> impl IntoView {
    let ctx = LayoutContext::new();
    provide_context(ctx);

    view! {
        <Layout position=LayoutPosition::Absolute class="ironpad-root-layout">
            <LayoutHeader class="ironpad-header">
                <HeaderContent ctx />
            </LayoutHeader>

            <div class="ironpad-content">
                {children()}
            </div>

            <footer class="ironpad-status-bar">
                <StatusBar ctx />
            </footer>
        </Layout>
    }
}

// ── Header ──────────────────────────────────────────────────────────────────

#[component]
fn HeaderContent(ctx: LayoutContext) -> impl IntoView {
    let on_save = move |_| {
        ctx.save_generation.update(|g| *g += 1);
    };

    view! {
        <div class="ironpad-header-left">
            <a href="/" class="ironpad-brand">"ironpad"</a>
        </div>

        <div class="ironpad-header-center">
            {move || ctx.notebook_title.get().map(|title| {
                view! { <span class="ironpad-notebook-title">{title}</span> }
            })}
        </div>

        <div class="ironpad-header-right">
            {move || ctx.show_save_button.get().then(|| {
                view! {
                    <Button
                        appearance=ButtonAppearance::Primary
                        on_click=on_save
                    >
                        "Save"
                    </Button>
                }
            })}
        </div>
    }
}

// ── Status bar ──────────────────────────────────────────────────────────────

#[component]
fn StatusBar(ctx: LayoutContext) -> impl IntoView {
    view! {
        <div class="ironpad-status-bar-inner">
            <span class="ironpad-status-item">
                "Status: Ready"
            </span>
            <span class="ironpad-status-separator">"|"</span>
            <span class="ironpad-status-item">
                "Compiler: "
                {move || ctx.compiler_version.get()}
            </span>
            <span class="ironpad-status-separator">"|"</span>
            <span class="ironpad-status-item">
                "Cells: "
                {move || ctx.cell_count.get().to_string()}
            </span>
            {move || ctx.last_save_time.get().map(|time| {
                view! {
                    <span class="ironpad-status-separator">"|"</span>
                    <span class="ironpad-status-item">
                        "Saved: "
                        {time}
                    </span>
                }
            })}
        </div>
    }
}
