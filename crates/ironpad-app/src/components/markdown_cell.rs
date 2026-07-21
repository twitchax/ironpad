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
    opts.insert(Options::ENABLE_MATH);
    let parser = Parser::new_ext(source, opts);
    let mut html = String::new();
    push_html(&mut html, parser);
    // pulldown_cmark passes raw HTML in the source straight through, so a
    // markdown cell (or Markdown output panel) could smuggle <script>/onerror
    // into a viewer's origin in a shared/public notebook. Sanitize the result.
    crate::sanitize::sanitize_html(&html)
}

// ── Component ───────────────────────────────────────────────────────────────

/// A markdown cell that toggles between preview (rendered HTML) and edit
/// (Monaco editor with `language="markdown"`) modes.
///
/// - **Preview mode** (default): rendered markdown via `inner_html`.
///   Double-click switches to edit mode.
/// - **Edit mode**: Monaco editor. Escape or clicking away (blur) saves
///   changes and switches back to preview mode. Switching the notebook to
///   view mode also forces a commit back to preview, even mid-edit.
#[component]
#[allow(clippy::needless_pass_by_value)]
pub fn MarkdownCell(
    /// The markdown source text.
    #[prop(into)]
    source: String,
    /// Fires with the updated markdown source when the editor loses focus or
    /// Escape is pressed.
    on_change: Callback<String>,
    /// When the notebook is in view mode, the cell must render (never stay
    /// in the Monaco editor).
    #[prop(into)]
    is_view_mode: Signal<bool>,
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
            // Store the closure in the reactive arena instead of forgetting it:
            // the Effect's owner drops it on re-run (after the prior editor is
            // disposed) and on unmount, rather than leaking one per edit-entry.
            StoredValue::new_local(cb);

            // Monaco keybinding: Escape = 9 (KeyCode.Escape).
            handle.add_action("ironpad.markdown.escape", &[9], &f);
        });
    }

    // Force a commit back to preview if the notebook switches to view mode
    // while this cell is mid-edit. Uses `commit` (not `editing.set(false)`)
    // so the in-progress edit is saved, not discarded.
    #[cfg(feature = "hydrate")]
    {
        let commit_for_view = commit;
        Effect::new(move || {
            if is_view_mode.get() && editing.get_untracked() {
                commit_for_view();
            }
        });
    }

    // Commit on blur (clicking away from the editor).
    let commit_for_blur = commit;

    // Suppress unused warnings during SSR.
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (&editor_handle, &commit, &is_view_mode);
    }

    view! {
        <div class="ironpad-markdown-cell">
            {move || {
                if editing.get() {
                    view! {
                        <div class="ironpad-markdown-cell-editor" on:focusout=move |_| commit_for_blur()>
                            <MonacoEditor
                                initial_value=current_source.get_untracked()
                                language="markdown"
                                on_change=editor_on_change
                                handle=editor_handle
                            />
                        </div>
                        <div class="ironpad-markdown-cell-hint">
                            "Press Escape or click away to save"
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

#[cfg(test)]
mod tests {
    use super::render_markdown;

    #[test]
    fn renders_markdown_formatting() {
        let html = render_markdown("# Title\n\nSome **bold** text.");
        assert!(html.contains("<h1"), "kept heading: {html}");
        assert!(html.contains("<strong>bold</strong>"), "kept bold: {html}");
    }

    #[test]
    fn preserves_math_span_classes_for_katex() {
        // pulldown-cmark (ENABLE_MATH) emits `<span class="math math-inline">tex</span>`
        // for `$...$` and `math-display` for `$$...$$`. The KaTeX bridge
        // (public/katex/render-math.js) locates them via the `span.math`
        // selector, so sanitization MUST keep those classes or math silently
        // renders as raw LaTeX.
        let inline = render_markdown(r"Euler: $e^{i\pi}+1=0$");
        assert!(
            inline.contains("math-inline"),
            "kept inline math span class for KaTeX: {inline}"
        );

        let display = render_markdown(r"$$\int_0^1 x\,dx = \tfrac12$$");
        assert!(
            display.contains("math-display"),
            "kept display math span class for KaTeX: {display}"
        );
    }

    #[test]
    fn fenced_code_keeps_language_class_for_prism() {
        // pulldown-cmark emits `<pre><code class="language-rust">` for a fenced
        // block with an info string, and the language class MUST survive
        // sanitization — the Prism bridge (public/prism/highlight-code.js)
        // finds blocks via `code[class*="language-"]`, so losing the class
        // silently disables syntax highlighting.
        let html = render_markdown("```rust\nlet x = 42;\n```");
        assert!(
            html.contains(r#"class="language-rust""#),
            "kept language class for Prism: {html}"
        );
    }

    #[test]
    fn strips_raw_html_in_markdown_source() {
        // pulldown_cmark passes raw HTML through; sanitization must strip the
        // active parts so a shared/public markdown cell can't XSS a viewer.
        let html =
            render_markdown("hello <script>steal()</script> <img src=x onerror=\"steal()\">");
        assert!(html.contains("hello"), "kept text: {html}");
        assert!(!html.contains("<script"), "stripped script: {html}");
        assert!(!html.contains("onerror"), "stripped onerror: {html}");
        assert!(!html.contains("steal"), "stripped payload: {html}");
    }
}
