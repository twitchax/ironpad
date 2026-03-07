use leptos::prelude::*;
use thaw::{Button, ButtonAppearance, Layout, LayoutHeader, LayoutPosition};

use crate::server_fns::update_notebook;

// ── Save status ─────────────────────────────────────────────────────────────

/// Visual state of the save button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStatus {
    Idle,
    Saving,
    Saved,
}

// ── Layout context ──────────────────────────────────────────────────────────

/// Shared reactive state between the layout shell and child pages.
///
/// Child components (e.g., the notebook editor) update these signals to
/// reflect the current page state in the header and status bar.
#[derive(Clone, Copy)]
pub struct LayoutContext {
    /// Notebook title shown in the header center. `None` when on the home page.
    pub notebook_title: RwSignal<Option<String>>,
    /// Notebook UUID string, needed for title‐save calls from the header.
    pub notebook_id: RwSignal<Option<String>>,
    /// Whether to show the save button in the header.
    pub show_save_button: RwSignal<bool>,
    /// Fires when the user clicks the save button. Child pages watch this.
    pub save_generation: RwSignal<u64>,
    /// Total cell count displayed in the status bar.
    pub cell_count: RwSignal<usize>,
    /// Human-readable "last saved" timestamp for the status bar.
    pub last_save_time: RwSignal<Option<String>>,
    /// Visual state of the save button (Idle → Saving → Saved → Idle).
    pub save_status: RwSignal<SaveStatus>,
    /// Compiler/toolchain version string.
    pub compiler_version: RwSignal<String>,
}

impl LayoutContext {
    fn new() -> Self {
        Self {
            notebook_title: RwSignal::new(None),
            notebook_id: RwSignal::new(None),
            show_save_button: RwSignal::new(false),
            save_generation: RwSignal::new(0),
            cell_count: RwSignal::new(0),
            last_save_time: RwSignal::new(None),
            save_status: RwSignal::new(SaveStatus::Idle),
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

    let save_label = move || match ctx.save_status.get() {
        SaveStatus::Idle => "Save",
        SaveStatus::Saving => "Saving…",
        SaveStatus::Saved => "Saved ✓",
    };

    let save_disabled = Signal::derive(move || ctx.save_status.get() == SaveStatus::Saving);

    // ── Inline-editable title state ─────────────────────────────────────

    let editing = RwSignal::new(false);

    let save_title = Action::new(move |new_title: &String| {
        let nb_id = ctx.notebook_id.get_untracked();
        let new_title = new_title.clone();
        async move {
            if let Some(id) = nb_id {
                let _ = update_notebook(id, new_title).await;
            }
        }
    });

    let commit_edit = move || {
        editing.set(false);
        if let Some(current) = ctx.notebook_title.get_untracked() {
            save_title.dispatch(current);
        }
    };

    let on_title_blur = move |_| {
        commit_edit();
    };

    let on_title_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" {
            ev.prevent_default();
            commit_edit();
        } else if ev.key() == "Escape" {
            editing.set(false);
        }
    };

    view! {
        <div class="ironpad-header-left">
            <a href="/" class="ironpad-brand">"ironpad"</a>
        </div>

        <div class="ironpad-header-center">
            {move || {
                match (ctx.notebook_title.get(), editing.get()) {
                    (Some(_title), true) => {
                        view! {
                            <input
                                class="ironpad-header-title-input"
                                type="text"
                                prop:value=move || ctx.notebook_title.get().unwrap_or_default()
                                on:input=move |ev| {
                                    let val = event_target_value(&ev);
                                    ctx.notebook_title.set(Some(val));
                                }
                                on:blur=on_title_blur
                                on:keydown=on_title_keydown
                                autofocus=true
                                node_ref={
                                    let input_ref = NodeRef::<leptos::html::Input>::new();
                                    // Focus the input after it renders.
                                    Effect::new(move || {
                                        if let Some(el) = input_ref.get() {
                                            let _ = el.focus();
                                            el.select();
                                        }
                                    });
                                    input_ref
                                }
                            />
                        }.into_any()
                    }
                    (Some(title), false) => {
                        view! {
                            <span
                                class="ironpad-notebook-title ironpad-notebook-title--editable"
                                on:click=move |_| editing.set(true)
                            >
                                {title}
                            </span>
                        }.into_any()
                    }
                    _ => {
                        view! { <span /> }.into_any()
                    }
                }
            }}
        </div>

        <div class="ironpad-header-right">
            {move || ctx.show_save_button.get().then(|| {
                view! {
                    <Button
                        appearance=ButtonAppearance::Primary
                        on_click=on_save
                        disabled=save_disabled
                    >
                        {save_label}
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
