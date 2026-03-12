use leptos::prelude::*;
#[cfg(feature = "hydrate")]
use leptos::web_sys;
use thaw::{Button, ButtonAppearance};

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
    /// Epoch milliseconds of the last save (used to compute relative time).
    pub last_save_time: RwSignal<Option<f64>>,
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
        <div class="ironpad-root-layout">
            <header class="ironpad-header">
                <HeaderContent ctx />
            </header>

            <div class="ironpad-content">
                {children()}
            </div>

            <footer class="ironpad-status-bar">
                <StatusBar ctx />
            </footer>
        </div>
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

    let save_title = Action::new(move |_new_title: &String| {
        async move {
            // Title is persisted client-side via IndexedDB; no server call needed.
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

    // ── Theme toggle state ──────────────────────────────────────────────

    let is_light_theme = RwSignal::new(false);

    #[cfg(feature = "hydrate")]
    {
        if let Some(window) = web_sys::window() {
            let stored = window
                .local_storage()
                .ok()
                .flatten()
                .and_then(|ls| ls.get_item("ironpad-theme").ok().flatten());
            if stored.as_deref() == Some("light") {
                is_light_theme.set(true);
            }
        }
    }

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
            <div class="ironpad-theme-toggle">
                <button
                    class=move || if is_light_theme.get() { "ironpad-theme-toggle-segment" } else { "ironpad-theme-toggle-segment ironpad-theme-toggle-segment--active" }
                    title="Dark mode"
                    on:click=move |_| {
                        #[cfg(feature = "hydrate")]
                        {
                            use wasm_bindgen::JsCast as _;

                            is_light_theme.set(false);
                            if let Some(doc) = web_sys::window()
                                .and_then(|w| w.document())
                            {
                                if let Some(html) = doc.document_element() {
                                    let _ = html.remove_attribute("data-theme");
                                }
                            }
                            if let Some(ls) = web_sys::window()
                                .and_then(|w| w.local_storage().ok().flatten())
                            {
                                let _ = ls.set_item("ironpad-theme", "dark");
                            }
                            if let Some(monaco) = js_sys::Reflect::get(
                                &web_sys::window().unwrap(),
                                &"IronpadMonaco".into(),
                            )
                            .ok()
                            .filter(|v| !v.is_undefined())
                            {
                                if let Ok(set_theme) =
                                    js_sys::Reflect::get(&monaco, &"setTheme".into())
                                {
                                    if set_theme.is_function() {
                                        let f: js_sys::Function = set_theme.unchecked_into();
                                        let _ = f.call1(&wasm_bindgen::JsValue::NULL, &"ironpad-dark".into());
                                    }
                                }
                            }
                        }
                    }
                >
                    "🌙"
                </button>
                <button
                    class=move || if is_light_theme.get() { "ironpad-theme-toggle-segment ironpad-theme-toggle-segment--active" } else { "ironpad-theme-toggle-segment" }
                    title="Light mode"
                    on:click=move |_| {
                        #[cfg(feature = "hydrate")]
                        {
                            use wasm_bindgen::JsCast as _;

                            is_light_theme.set(true);
                            if let Some(doc) = web_sys::window()
                                .and_then(|w| w.document())
                            {
                                if let Some(html) = doc.document_element() {
                                    let _ = html.set_attribute("data-theme", "light");
                                }
                            }
                            if let Some(ls) = web_sys::window()
                                .and_then(|w| w.local_storage().ok().flatten())
                            {
                                let _ = ls.set_item("ironpad-theme", "light");
                            }
                            if let Some(monaco) = js_sys::Reflect::get(
                                &web_sys::window().unwrap(),
                                &"IronpadMonaco".into(),
                            )
                            .ok()
                            .filter(|v| !v.is_undefined())
                            {
                                if let Ok(set_theme) =
                                    js_sys::Reflect::get(&monaco, &"setTheme".into())
                                {
                                    if set_theme.is_function() {
                                        let f: js_sys::Function = set_theme.unchecked_into();
                                        let _ = f.call1(&wasm_bindgen::JsValue::NULL, &"ironpad-light".into());
                                    }
                                }
                            }
                        }
                    }
                >
                    "☀"
                </button>
            </div>
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

/// Format an epoch-ms timestamp as a human-readable relative string.
#[cfg(feature = "hydrate")]
fn format_relative_time(epoch_ms: f64, now_ms: f64) -> String {
    let diff_secs = ((now_ms - epoch_ms) / 1000.0).max(0.0) as u64;

    match diff_secs {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{}s ago", diff_secs),
        60..=3599 => {
            let mins = diff_secs / 60;
            format!("{}m ago", mins)
        }
        3600..=86399 => {
            let hours = diff_secs / 3600;
            format!("{}h ago", hours)
        }
        _ => {
            let days = diff_secs / 86400;
            format!("{}d ago", days)
        }
    }
}

#[component]
fn StatusBar(ctx: LayoutContext) -> impl IntoView {
    // Tick counter that bumps every 30 s so the relative timestamp refreshes.
    let tick = RwSignal::new(0u64);

    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::prelude::*;

        let closure = Closure::<dyn Fn()>::new(move || {
            tick.update(|t| *t += 1);
        });
        let _ = web_sys::window()
            .unwrap()
            .set_interval_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                30_000,
            );
        closure.forget();
    }

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
            {move || {
                // Touch `tick` so the closure re-runs on each interval.
                let _ = tick.get();
                ctx.last_save_time.get().map(|epoch_ms| {
                    let relative = {
                        #[cfg(feature = "hydrate")]
                        { format_relative_time(epoch_ms, js_sys::Date::now()) }
                        #[cfg(not(feature = "hydrate"))]
                        { let _ = epoch_ms; "just now".to_string() }
                    };
                    view! {
                        <span class="ironpad-status-separator">"|"</span>
                        <span class="ironpad-status-item">"Saved: " {relative}</span>
                    }
                })
            }}
        </div>
    }
}
