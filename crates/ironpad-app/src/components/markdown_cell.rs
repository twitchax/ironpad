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
/// - **Edit mode**: Monaco editor. Every edit reaches the notebook through
///   `on_change` as it is typed, exactly as a code cell's does. Escape or
///   clicking away (blur) switches back to preview mode, and so does a
///   flush (Ctrl+S, entering view mode), even mid-edit.
#[component]
#[allow(clippy::needless_pass_by_value)]
pub fn MarkdownCell(
    /// The cell's markdown source, live: an edit from elsewhere (an agent)
    /// reaches the preview, and the next editor, once no editor is open.
    #[prop(into)]
    source: Signal<String>,
    /// Fires with the editor's full text on every edit. The parent debounces
    /// it into the notebook (CellItem's `on_source_change`), so a markdown
    /// cell has the same save window as a code cell.
    on_change: Callback<String>,
    /// Bumped when a save/flush is requested (Ctrl+S, entering view mode).
    /// An open editor closes on each bump; the parent's own flush writes the
    /// text, which `on_change` has already handed it.
    #[prop(into)]
    flush_generation: Signal<u64>,
) -> impl IntoView {
    let editing = RwSignal::new(false);
    let current_source = RwSignal::new(source.get_untracked());

    // Preview HTML derived from current source.
    let preview_html = Memo::new(move |_| render_markdown(&current_source.get()));

    // Handle for the Monaco editor instance.
    let editor_handle: RwSignal<Option<MonacoEditorHandle>> = RwSignal::new(None);

    // The editor's wrapper. Monaco's find widget (Ctrl+F), command palette
    // (F1) and context menu all take focus inside it, so focus moving within
    // it is not a click away.
    let wrapper = NodeRef::<leptos::html::Div>::new();

    // Pull an outside edit into the preview. Never under an open editor, and
    // `editing` is tracked so an edit that landed meanwhile shows on close.
    // The editor's own typing comes back through here too, already equal.
    Effect::new(move || {
        let latest = source.get();
        if !editing.get() && latest != current_source.get_untracked() {
            current_source.set(latest);
        }
    });

    // Switch to edit mode on double-click.
    let on_dblclick = move |_| {
        editing.set(true);
    };

    // Return to preview mode, once (disposing the editor fires a late
    // focusout after it has closed). Closing writes nothing:
    // `editor_on_change` already handed every edit to the notebook, and a
    // value read back from a closing (or not yet loaded) editor is exactly
    // what must never land on the cell.
    let commit = move || {
        if editing.try_get_untracked() == Some(true) {
            editing.set(false);
        }
    };

    // The ONE writer: every edit updates the preview and goes to the
    // notebook through the parent's debounced save, so typed text survives a
    // reload or a closed tab without the editor ever closing.
    let editor_on_change = Callback::new(move |val: String| {
        current_source.set(val.clone());
        on_change.run(val);
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

            // Monaco keybinding: Escape = 9 (KeyCode.Escape). Not while find
            // is open: a dynamic binding outranks Monaco's closeFindWidget, so
            // without the precondition Escape closed the cell, not the find bar.
            handle.add_action_when("ironpad.markdown.escape", &[9], "!findWidgetVisible", &f);
        });
    }

    // Close an open editor whenever a flush is requested (Ctrl+S, the
    // view-mode toggle). Its text is already with the parent, whose own flush
    // writes it into the notebook.
    #[cfg(feature = "hydrate")]
    {
        let commit_for_flush = commit;
        let prev_gen = RwSignal::new(flush_generation.get_untracked());
        Effect::new(move || {
            let gen = flush_generation.get();
            if gen == prev_gen.get_untracked() {
                return;
            }
            prev_gen.set(gen);
            if editing.get_untracked() {
                commit_for_flush();
            }
        });
    }

    // Commit on blur (clicking away from the editor), but only once focus has
    // really left the wrapper. Treating focus moving into a Monaco widget as a
    // click away closed the editor under the widget the author had just
    // opened. A widget closing on Escape hides its focused input before Monaco
    // refocuses the editor, so that focusout names no related target although
    // focus never leaves: with none, decide on the next tick from where focus
    // actually landed.
    let on_focusout = move |ev: leptos::ev::FocusEvent| {
        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::JsCast;
            let inside = move |node: Option<web_sys::Node>| {
                node.is_some_and(|node| {
                    wrapper
                        .try_get_untracked()
                        .flatten()
                        .is_some_and(|el| el.contains(Some(&node)))
                })
            };
            let Some(related) = ev.related_target() else {
                let settle = wasm_bindgen::closure::Closure::once_into_js(move || {
                    let active = web_sys::window()
                        .and_then(|w| w.document())
                        .and_then(|d| d.active_element())
                        .map(Into::into);
                    if !inside(active) {
                        commit();
                    }
                });
                if let Some(win) = web_sys::window() {
                    let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
                        settle.unchecked_ref(),
                        0,
                    );
                }
                return;
            };
            if inside(related.dyn_into::<web_sys::Node>().ok()) {
                return;
            }
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = ev;
        commit();
    };

    // A right-button mousedown moves focus out before Monaco's context menu
    // opens (Monaco does not prevent it), and that focusout carries no
    // related target to recognize. Keep focus where it is instead.
    let on_mousedown = move |ev: leptos::ev::MouseEvent| {
        if ev.button() == 2 {
            ev.prevent_default();
        }
    };

    // Suppress unused warnings during SSR.
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (&editor_handle, &commit, &flush_generation);
    }

    view! {
        <div class="ironpad-markdown-cell">
            {move || {
                if editing.get() {
                    view! {
                        <div
                            class="ironpad-markdown-cell-editor"
                            node_ref=wrapper
                            on:focusout=on_focusout
                            on:mousedown=on_mousedown
                        >
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
    fn heading_attributes_cannot_borrow_an_app_class() {
        // ENABLE_HEADING_ATTRIBUTES turns `{.cls}` into `class="cls"`, which
        // the sanitizer's class allowlist must strip like a raw-HTML class.
        let html = render_markdown("# T {.ironpad-history-overlay}");
        assert!(html.contains("<h1"), "kept the heading: {html}");
        assert!(
            !html.contains("ironpad-history-overlay"),
            "stripped the borrowed class: {html}"
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
