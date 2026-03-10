/// Markdown cell component — renders markdown as HTML in preview mode,
/// switches to a Monaco editor (language="markdown") on double-click.
use leptos::prelude::*;
use pulldown_cmark::{html::push_html, Options, Parser};

use crate::components::monaco_editor::{MonacoEditor, MonacoEditorHandle};

// ── Markdown rendering ──────────────────────────────────────────────────────

/// Render markdown source to an HTML string using pulldown-cmark.
pub fn render_markdown(source: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    let parser = Parser::new_ext(source, opts);
    let mut html = String::new();
    push_html(&mut html, parser);
    html
}

// ── Component ───────────────────────────────────────────────────────────────

/// A markdown cell that toggles between preview (rendered HTML) and edit
/// (Monaco editor with `language="markdown"`) modes.
///
/// - **Preview mode** (default): rendered markdown via `inner_html`.
///   Double-click switches to edit mode.
/// - **Edit mode**: Monaco editor. Escape saves changes and
///   switches back to preview mode.
#[component]
pub fn MarkdownCell(
    /// The markdown source text.
    #[prop(into)]
    source: String,
    /// Fires with the updated markdown source when the editor loses focus or
    /// Escape is pressed.
    on_change: Callback<String>,
    /// Unique cell identifier (used for keying).
    #[prop(into)]
    cell_id: String,
) -> impl IntoView {
    let editing = RwSignal::new(false);
    let current_source = RwSignal::new(source.clone());

    // Preview HTML derived from current source.
    let preview_html = Memo::new(move |_| render_markdown(&current_source.get()));

    // Handle for the Monaco editor instance.
    let editor_handle: RwSignal<Option<MonacoEditorHandle>> = RwSignal::new(None);

    // Switch to edit mode on double-click.
    let on_dblclick = move |_| {
        editing.set(true);
    };

    // Commit changes and return to preview mode.
    let commit = move || {
        if let Some(handle) = editor_handle.get_untracked() {
            let val = handle.get_value();
            current_source.set(val.clone());
            on_change.run(val);
        }
        editing.set(false);
    };

    // Editor on_change keeps the signal in sync.
    let editor_on_change = Callback::new(move |val: String| {
        current_source.set(val);
    });

    // Register Escape keybinding once the editor mounts.
    #[cfg(feature = "hydrate")]
    {
        let commit_for_escape = commit;
        Effect::new(move || {
            if !editing.get() {
                return;
            }
            let Some(handle) = editor_handle.get() else {
                return;
            };

            let cb = wasm_bindgen::closure::Closure::<dyn Fn()>::new(move || {
                commit_for_escape();
            });
            let f: js_sys::Function = {
                use wasm_bindgen::JsCast;
                cb.as_ref().unchecked_ref::<js_sys::Function>().clone()
            };
            cb.forget();

            // Monaco keybinding: Escape = 9 (KeyCode.Escape).
            handle.add_action("ironpad.markdown.escape", &[9], &f);
        });
    }

    // Suppress unused warnings during SSR.
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (&editor_handle, &commit, &cell_id);
    }

    // Consume cell_id to avoid unused warning (reserved for future keying).
    let _ = &cell_id;

    view! {
        <div class="ironpad-markdown-cell">
            {move || {
                if editing.get() {
                    view! {
                        <div class="ironpad-markdown-cell-editor">
                            <MonacoEditor
                                initial_value=current_source.get_untracked()
                                language="markdown"
                                on_change=editor_on_change
                                handle=editor_handle
                            />
                        </div>
                        <div class="ironpad-markdown-cell-hint">
                            "Press Escape to save and return to preview"
                        </div>
                    }.into_any()
                } else {
                    let html = preview_html.get();
                    if html.trim().is_empty() {
                        view! {
                            <div class="ironpad-markdown-cell-preview"
                                 on:dblclick=on_dblclick>
                                <p class="ironpad-placeholder">
                                    "Double-click to edit markdown…"
                                </p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="ironpad-markdown-cell-preview"
                                 on:dblclick=on_dblclick
                                 inner_html=html>
                            </div>
                        }.into_any()
                    }
                }
            }}
        </div>
    }
}
