use leptos::prelude::*;
#[cfg(feature = "hydrate")]
use leptos::web_sys;

// ── Layout context ──────────────────────────────────────────────────────────

/// Shared reactive state between the layout shell and child pages.
///
/// Child components (e.g., the notebook editor) update these signals to
/// reflect the current page state in the header and status bar.
#[derive(Clone, Copy)]
pub struct LayoutContext {
    /// Notebook title shown in the header center. `None` when on the home page.
    pub notebook_title: RwSignal<Option<String>>,
    /// Fires when a save is requested (Ctrl+S, title commit). Child pages
    /// watch this and flush + persist.
    pub save_generation: RwSignal<u64>,
    /// Total cell count displayed in the status bar.
    pub cell_count: RwSignal<usize>,
    /// Epoch milliseconds of the last save (used to compute relative time).
    pub last_save_time: RwSignal<Option<f64>>,
    /// Compiler/toolchain version string.
    pub compiler_version: RwSignal<String>,
}

impl LayoutContext {
    fn new() -> Self {
        Self {
            notebook_title: RwSignal::new(None),
            save_generation: RwSignal::new(0),
            cell_count: RwSignal::new(0),
            last_save_time: RwSignal::new(None),
            compiler_version: RwSignal::new(crate::CELL_TOOLCHAIN.to_string()),
        }
    }
}

// ── App layout ──────────────────────────────────────────────────────────────

/// Top-level app layout: header, scrollable content area, and status bar.
#[component]
pub fn AppLayout(children: Children) -> impl IntoView {
    let ctx = LayoutContext::new();
    provide_context(ctx);

    let location = leptos_router::hooks::use_location();
    let show_footer = Memo::new(move |_| location.pathname.get() != "/");
    // Chrome-less embed mode (PRD-0039): /embed/* routes render only the page
    // content. Path-derived so SSR and client navigation agree; LayoutContext
    // is still provided, so pages that touch it keep working.
    let embed_mode = Memo::new(move |_| location.pathname.get().starts_with("/embed/"));

    view! {
        <div class=move || {
            if embed_mode.get() {
                // The embed variant lets the document grow with its content:
                // the height reporter measures scrollHeight, which stays
                // pinned at the iframe size if the layout scrolls internally.
                "ironpad-root-layout ironpad-root-layout--embed"
            } else {
                "ironpad-root-layout"
            }
        }>
            {move || (!embed_mode.get()).then(|| view! {
                <header class="ironpad-header">
                    <HeaderContent ctx />
                </header>
            })}

            <div class="ironpad-content">
                {children()}
            </div>

            {move || (show_footer.get() && !embed_mode.get()).then(|| view! {
                <footer class="ironpad-status-bar">
                    <StatusBar ctx />
                </footer>
            })}
        </div>
    }
}

// ── Header ──────────────────────────────────────────────────────────────────

/// Apply the given theme: set/remove `data-theme="light"` on `<html>`, persist
/// the choice to `localStorage['ironpad-theme']`, and update the Monaco theme.
#[cfg(feature = "hydrate")]
fn apply_theme(is_light: bool) {
    use wasm_bindgen::JsCast as _;

    let Some(window) = web_sys::window() else {
        return;
    };

    if let Some(html) = window.document().and_then(|d| d.document_element()) {
        if is_light {
            let _ = html.set_attribute("data-theme", "light");
        } else {
            let _ = html.remove_attribute("data-theme");
        }
    }

    if let Some(ls) = window.local_storage().ok().flatten() {
        let _ = ls.set_item("ironpad-theme", if is_light { "light" } else { "dark" });
    }

    if let Some(monaco) = js_sys::Reflect::get(&window, &"IronpadMonaco".into())
        .ok()
        .filter(|v| !v.is_undefined())
    {
        if let Ok(set_theme) = js_sys::Reflect::get(&monaco, &"setTheme".into()) {
            if set_theme.is_function() {
                let f: js_sys::Function = set_theme.unchecked_into();
                let theme = if is_light {
                    "ironpad-light"
                } else {
                    "ironpad-dark"
                };
                let _ = f.call1(&wasm_bindgen::JsValue::NULL, &theme.into());
            }
        }
    }
}

#[component]
fn HeaderContent(ctx: LayoutContext) -> impl IntoView {
    // ── Inline-editable title state ─────────────────────────────────────

    let editing = RwSignal::new(false);
    let edit_start_title = RwSignal::new(None::<String>);

    let commit_edit = move || {
        editing.set(false);
        // Persist only if the title changed. Bumping save_generation triggers
        // the editor's save watcher (notebook_editor/mod.rs), which applies
        // NotebookUpdateMeta { title } through the model and persists to IndexedDB.
        if ctx.notebook_title.get_untracked() != edit_start_title.get_untracked() {
            ctx.save_generation.update(|g| *g += 1);
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
            ctx.notebook_title.set(edit_start_title.get_untracked());
            editing.set(false);
        }
    };

    // ── Theme toggle state ──────────────────────────────────────────────

    let is_light_theme = RwSignal::new(false);

    #[cfg(feature = "hydrate")]
    {
        let stored = web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .and_then(|ls| ls.get_item("ironpad-theme").ok().flatten());
        if stored.as_deref() == Some("light") {
            is_light_theme.set(true);
            // Apply the stored theme on mount so a light-theme reload doesn't
            // render the page — and Monaco — in the default dark theme until
            // the user re-toggles.
            apply_theme(true);
        }
    }

    view! {
        <div class="ironpad-header-left">
            <a href="/" class="ironpad-brand">"ironpad"</a>
        </div>

        <div class="ironpad-header-center">
            // The branch closure must NOT track the title's *content*: every
            // keystroke sets ctx.notebook_title, and tracking it here remounted
            // the <input> per keystroke — whose focus effect then select()ed
            // everything, so the next key replaced the whole field with one
            // character. The memo only fires on presence changes, and the
            // input is uncontrolled (initial value at mount; it mounts fresh
            // each time editing starts).
            {
                let has_title = Memo::new(move |_| ctx.notebook_title.with(Option::is_some));
                move || {
                    match (has_title.get(), editing.get()) {
                        (true, true) => {
                            view! {
                                <input
                                    class="ironpad-header-title-input"
                                    type="text"
                                    prop:value=ctx.notebook_title.get_untracked().unwrap_or_default()
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
                        (true, false) => {
                            view! {
                                <span
                                    class="ironpad-notebook-title ironpad-notebook-title--editable"
                                    on:click=move |_| {
                                        edit_start_title.set(ctx.notebook_title.get_untracked());
                                        editing.set(true);
                                    }
                                >
                                    {move || ctx.notebook_title.get().unwrap_or_default()}
                                </span>
                            }.into_any()
                        }
                        _ => {
                            view! { <span /> }.into_any()
                        }
                    }
                }
            }
        </div>

        <div class="ironpad-header-right">
            <a
                class="ironpad-github-link"
                href="https://github.com/twitchax/ironpad"
                target="_blank"
                rel="noopener noreferrer"
                title="View source on GitHub"
                aria-label="View ironpad source on GitHub"
            >
                <svg
                    viewBox="0 0 24 24"
                    width="20"
                    height="20"
                    fill="currentColor"
                    aria-hidden="true"
                >
                    <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12"/>
                </svg>
            </a>
            <div class="ironpad-theme-toggle">
                <button
                    class=move || if is_light_theme.get() { "ironpad-theme-toggle-segment" } else { "ironpad-theme-toggle-segment ironpad-theme-toggle-segment--active" }
                    title="Dark mode"
                    on:click=move |_| {
                        #[cfg(feature = "hydrate")]
                        {
                            is_light_theme.set(false);
                            apply_theme(false);
                        }
                    }
                >
                    "☾"
                </button>
                <button
                    class=move || if is_light_theme.get() { "ironpad-theme-toggle-segment ironpad-theme-toggle-segment--active" } else { "ironpad-theme-toggle-segment" }
                    title="Light mode"
                    on:click=move |_| {
                        #[cfg(feature = "hydrate")]
                        {
                            is_light_theme.set(true);
                            apply_theme(true);
                        }
                    }
                >
                    "☼"
                </button>
            </div>
        </div>
    }
}

// ── Status bar ──────────────────────────────────────────────────────────────

/// Format an epoch-ms timestamp as a human-readable relative string.
#[cfg(feature = "hydrate")]
fn format_relative_time(epoch_ms: f64, now_ms: f64) -> String {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "value is clamped to non-negative and fits comfortably in u64"
    )]
    let diff_secs = ((now_ms - epoch_ms) / 1000.0).max(0.0) as u64;

    match diff_secs {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{diff_secs}s ago"),
        60..=3599 => {
            let mins = diff_secs / 60;
            format!("{mins}m ago")
        }
        3600..=86399 => {
            let hours = diff_secs / 3600;
            format!("{hours}h ago")
        }
        _ => {
            let days = diff_secs / 86400;
            format!("{days}d ago")
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
        let interval_id = web_sys::window()
            .unwrap()
            .set_interval_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                30_000,
            )
            .ok();
        // Store the closure in the reactive arena so it's dropped on dispose
        // (freeing its JS callback), and clear the interval on unmount so a
        // remount (StatusBar is conditionally rendered) doesn't leave a timer
        // ticking a disposed signal.
        StoredValue::new_local(closure);
        on_cleanup(move || {
            if let (Some(id), Some(win)) = (interval_id, web_sys::window()) {
                win.clear_interval_with_handle(id);
            }
        });
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
