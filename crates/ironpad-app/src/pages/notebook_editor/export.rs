#[cfg(any(feature = "hydrate", test))]
use std::collections::HashMap;
#[cfg(any(feature = "hydrate", test))]
use std::fmt::Write;

#[cfg(any(feature = "hydrate", test))]
use ironpad_common::{CellType, IronpadNotebook};

#[cfg(any(feature = "hydrate", test))]
use crate::components::markdown_cell::render_markdown;
#[cfg(any(feature = "hydrate", test))]
use crate::components::output_render::{html_escape, render_table_html, DisplayPanel};

// ── Export HTML helpers ─────────────────────────────────────────────────────

#[cfg(any(feature = "hydrate", test))]
const EXPORT_CSS: &str = r"
:root { color-scheme: dark; }
* { margin: 0; padding: 0; box-sizing: border-box; }
body {
    background: #1a1a2e; color: #e0e0e0; font-family: -apple-system, BlinkMacSystemFont,
    'Segoe UI', Roboto, sans-serif; line-height: 1.6; padding: 2rem; max-width: 960px; margin: 0 auto;
}
h1.notebook-title {
    color: #fff; font-size: 1.8rem; margin-bottom: 1.5rem;
    padding-bottom: 0.5rem; border-bottom: 1px solid #3a3a5c;
}
.cell { margin-bottom: 1.5rem; }
.cell-label {
    font-size: 0.75rem; color: #888; text-transform: uppercase;
    letter-spacing: 0.05em; margin-bottom: 0.25rem;
}
pre.code-block {
    background: #16213e; color: #e0e0e0; padding: 1rem; border-radius: 6px;
    overflow-x: auto; font-family: 'Fira Code', 'Cascadia Code', 'Consolas', monospace;
    font-size: 0.9rem; line-height: 1.5; border: 1px solid #2a2a4a;
}
.cell-output {
    background: #0f1a2e; border: 1px solid #2a2a4a; border-top: none;
    border-radius: 0 0 6px 6px; padding: 0.75rem 1rem; font-size: 0.85rem;
}
.cell-output pre { white-space: pre-wrap; word-wrap: break-word; color: #b0b0b0; }
.cell-output .output-label {
    font-size: 0.7rem; color: #666; text-transform: uppercase;
    letter-spacing: 0.05em; margin-bottom: 0.25rem;
}
.markdown-content {
    background: #16213e; padding: 1rem 1.25rem; border-radius: 6px; border: 1px solid #2a2a4a;
}
.markdown-content h1, .markdown-content h2, .markdown-content h3,
.markdown-content h4, .markdown-content h5, .markdown-content h6 {
    color: #fff; margin: 0.75em 0 0.5em;
}
.markdown-content h1 { font-size: 1.5rem; }
.markdown-content h2 { font-size: 1.3rem; }
.markdown-content h3 { font-size: 1.15rem; }
.markdown-content p { margin: 0.5em 0; }
.markdown-content a { color: #64b5f6; text-decoration: none; }
.markdown-content a:hover { text-decoration: underline; }
.markdown-content code {
    background: #0f1a2e; padding: 0.15em 0.4em; border-radius: 3px;
    font-family: 'Fira Code', 'Cascadia Code', 'Consolas', monospace; font-size: 0.9em;
}
.markdown-content pre { background: #0f1a2e; padding: 0.75rem; border-radius: 4px; overflow-x: auto; }
.markdown-content pre code { background: none; padding: 0; }
.markdown-content ul, .markdown-content ol { padding-left: 1.5rem; margin: 0.5em 0; }
.markdown-content li { margin: 0.25em 0; }
.markdown-content blockquote {
    border-left: 3px solid #3a3a5c; padding-left: 1rem;
    color: #aaa; margin: 0.5em 0;
}
.markdown-content table {
    border-collapse: collapse; width: 100%; margin: 0.75em 0;
}
.markdown-content th, .markdown-content td {
    border: 1px solid #3a3a5c; padding: 0.4rem 0.75rem; text-align: left;
}
.markdown-content th { background: #1a1a3e; color: #fff; font-weight: 600; }
.markdown-content tr:nth-child(even) { background: #12192e; }
.markdown-content img { max-width: 100%; border-radius: 4px; }
.output-html { padding: 0.75rem; }
.output-svg { text-align: center; padding: 0.75rem; }
.output-svg svg { max-width: 100%; height: auto; }
.footer {
    margin-top: 3rem; padding-top: 1rem; border-top: 1px solid #3a3a5c;
    font-size: 0.75rem; color: #666; text-align: center;
}
";

/// Build a self-contained HTML document from a notebook and its cached display texts.
#[cfg(any(feature = "hydrate", test))]
pub(super) fn build_export_html(
    nb: &IronpadNotebook,
    display_texts: &HashMap<String, String>,
) -> String {
    let mut html = String::with_capacity(8192);

    // Document header.
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    html.push_str("<meta charset=\"UTF-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n");
    // Defense in depth: the export is a self-contained static document
    // (inline styles, no scripts), so a strict CSP that forbids scripts and
    // restricts other loads costs nothing and neutralizes any injection that
    // slips past the per-field escaping below if this file is ever re-hosted.
    html.push_str(
        "<meta http-equiv=\"Content-Security-Policy\" \
         content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:\">\n",
    );
    let _ = writeln!(html, "<title>{}</title>", html_escape(&nb.title));
    html.push_str("<style>\n");
    html.push_str(EXPORT_CSS);
    html.push_str("</style>\n</head>\n<body>\n");

    // Title.
    let _ = writeln!(
        html,
        "<h1 class=\"notebook-title\">{}</h1>",
        html_escape(&nb.title)
    );

    // Cells.
    for cell in &nb.cells {
        html.push_str("<div class=\"cell\">\n");
        let _ = writeln!(
            html,
            "<div class=\"cell-label\">{}</div>",
            html_escape(&cell.label)
        );

        match cell.cell_type {
            CellType::Code => {
                let _ = writeln!(
                    html,
                    "<pre class=\"code-block\"><code>{}</code></pre>",
                    html_escape(&cell.source)
                );

                // Include cached output if available.
                if let Some(display_json) = display_texts.get(&cell.id) {
                    if let Ok(panels) = serde_json::from_str::<Vec<DisplayPanel>>(display_json) {
                        html.push_str("<div class=\"cell-output\">\n");
                        html.push_str("<div class=\"output-label\">Output</div>\n");
                        for panel in &panels {
                            match panel {
                                DisplayPanel::Text(text) => {
                                    let _ = writeln!(html, "<pre>{}</pre>", html_escape(text));
                                }
                                DisplayPanel::Html(h) => {
                                    // Sanitize untrusted cell output in the exported file too.
                                    let safe = crate::sanitize::sanitize_html(h);
                                    let _ =
                                        writeln!(html, "<div class=\"output-html\">{safe}</div>");
                                }
                                DisplayPanel::Svg(s) => {
                                    let safe = crate::sanitize::sanitize_svg(s);
                                    let _ =
                                        writeln!(html, "<div class=\"output-svg\">{safe}</div>");
                                }
                                DisplayPanel::Markdown(md) => {
                                    let rendered = render_markdown(md);
                                    let _ = writeln!(
                                        html,
                                        "<div class=\"ironpad-markdown-cell-preview\">{rendered}</div>"
                                    );
                                }
                                DisplayPanel::Table { headers, rows } => {
                                    html.push_str(&render_table_html(headers, rows));
                                    html.push('\n');
                                }
                                DisplayPanel::Interactive { kind, config } => {
                                    render_interactive_static(&mut html, kind, config);
                                }
                                DisplayPanel::BlobImage {
                                    mime_type,
                                    base64_data,
                                    width,
                                    height,
                                } => {
                                    // Static HTML export: inline the data URL since
                                    // Blob URLs are not available in a static file.
                                    // The panel is cell-emitted (untrusted), so the
                                    // attribute-spliced fields are escaped — an
                                    // unescaped mime_type like `x" onerror="…` would
                                    // break out of the src attribute (XSS).
                                    let _ = writeln!(
                                        html,
                                        "<div class=\"output-html\">\
                                         <img src=\"data:{};base64,{}\" \
                                         width=\"{width}\" height=\"{height}\" \
                                         style=\"image-rendering: pixelated;\" />\
                                         </div>",
                                        html_escape(mime_type),
                                        html_escape(base64_data)
                                    );
                                }
                                DisplayPanel::Animation {
                                    width,
                                    height,
                                    fps,
                                    frame_count,
                                    ..
                                } => {
                                    let _ = writeln!(
                                        html,
                                        "<div class=\"output-html\">\
                                         <em>Animation: {frame_count} frames at {fps} fps ({width}×{height})</em>\
                                         </div>"
                                    );
                                }
                                DisplayPanel::Simulation {
                                    width, height, fps, ..
                                } => {
                                    let _ = writeln!(
                                        html,
                                        "<div class=\"output-html\">\
                                         <em>Simulation at {fps} fps ({width}×{height})</em>\
                                         </div>"
                                    );
                                }
                                DisplayPanel::LiveView {
                                    fps,
                                    kind,
                                    content: _,
                                } => {
                                    let _ = writeln!(
                                        html,
                                        "<div class=\"output-html\">\
                                         <em>LiveView at {fps} fps ({})</em>\
                                         </div>",
                                        html_escape(kind)
                                    );
                                }
                            }
                        }
                        html.push_str("</div>\n");
                    }
                }
            }
            CellType::Markdown => {
                let rendered = render_markdown(&cell.source);
                let _ = writeln!(html, "<div class=\"markdown-content\">{rendered}</div>");
            }
        }

        html.push_str("</div>\n");
    }

    // Footer.
    html.push_str("<div class=\"footer\">Exported from <strong>ironpad</strong></div>\n");
    html.push_str("</body>\n</html>");

    html
}

/// Render an interactive widget as a static HTML representation for export.
#[cfg(any(feature = "hydrate", test))]
fn render_interactive_static(html: &mut String, kind: &str, config: &str) {
    let cfg: serde_json::Value = serde_json::from_str(config).unwrap_or_default();
    let label = cfg.get("label").and_then(|v| v.as_str()).unwrap_or("");

    match kind {
        "slider" | "number" => {
            let default = cfg
                .get("default")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);
            let label_text = if label.is_empty() {
                kind.to_owned()
            } else {
                label.to_owned()
            };
            let _ = writeln!(
                html,
                "<div class=\"ironpad-interactive-static\"><strong>{}</strong>: {default}</div>",
                html_escape(&label_text)
            );
        }
        "dropdown" => {
            let default = cfg.get("default").and_then(|v| v.as_str()).unwrap_or("");
            let label_text = if label.is_empty() { "dropdown" } else { label };
            let _ = writeln!(
                html,
                "<div class=\"ironpad-interactive-static\"><strong>{}</strong>: {}</div>",
                html_escape(label_text),
                html_escape(default)
            );
        }
        "checkbox" | "switch" => {
            let default = cfg
                .get("default")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            // The export builds HTML, so the same icon_svg_markup the
            // components use serves it directly — one wrapper, two paths.
            let icon = crate::components::icon::icon_svg_markup(if default {
                crate::components::icons::CHECKBOX_CHECKED
            } else {
                crate::components::icons::CHECKBOX
            });
            let _ = writeln!(
                html,
                "<div class=\"ironpad-interactive-static\">{icon} {}</div>",
                html_escape(label)
            );
        }
        "text_input" => {
            let default = cfg.get("default").and_then(|v| v.as_str()).unwrap_or("");
            let label_text = if label.is_empty() { "text" } else { label };
            let _ = writeln!(
                html,
                "<div class=\"ironpad-interactive-static\"><strong>{}</strong>: {}</div>",
                html_escape(label_text),
                html_escape(default)
            );
        }
        _ => {
            let _ = writeln!(
                html,
                "<div class=\"ironpad-interactive-static\">[{} widget]</div>",
                html_escape(kind)
            );
        }
    }
}

/// Trigger a browser file download with the given content, MIME type, and filename.
#[cfg(feature = "hydrate")]
fn trigger_download(content: &str, mime_type: &str, filename: &str) {
    use wasm_bindgen::JsCast;

    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };

    // Build a Blob from the content string.
    let parts = js_sys::Array::new();
    parts.push(&wasm_bindgen::JsValue::from_str(content));

    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime_type);

    let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts) else {
        return;
    };

    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };

    // Create a temporary <a> element to trigger the download.
    let anchor: web_sys::HtmlAnchorElement = match document
        .create_element("a")
        .ok()
        .and_then(|el| el.dyn_into::<web_sys::HtmlAnchorElement>().ok())
    {
        Some(a) => a,
        None => return,
    };

    anchor.set_href(&url);
    anchor.set_download(filename);
    anchor.set_attribute("style", "display:none").ok();

    if let Some(body) = document.body() {
        let _ = body.append_child(&anchor);
        anchor.click();
        let _ = body.remove_child(&anchor);
    }

    let _ = web_sys::Url::revoke_object_url(&url);
}

/// Sanitize a title for use as a filename, replacing non-alphanumeric characters with dashes.
#[cfg(feature = "hydrate")]
fn sanitize_filename(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Trigger a browser file download from an HTML string.
#[cfg(feature = "hydrate")]
pub(super) fn trigger_html_download(html_content: &str, title: &str) {
    let filename = format!("{}.html", sanitize_filename(title));
    trigger_download(html_content, "text/html;charset=utf-8", &filename);
}

/// Trigger a browser file download from a JSON string with `.ironpad` extension.
#[cfg(feature = "hydrate")]
pub(super) fn trigger_ironpad_download(json_content: &str, title: &str) {
    let filename = format!("{}.ironpad", sanitize_filename(title));
    trigger_download(json_content, "application/json;charset=utf-8", &filename);
}

#[cfg(test)]
mod tests {
    use super::build_export_html;
    use ironpad_common::{CellType, IronpadCell, IronpadNotebook};
    use std::collections::HashMap;

    fn code_cell(id: &str) -> IronpadCell {
        IronpadCell {
            id: id.to_string(),
            order: 0,
            label: "c".to_string(),
            cell_type: CellType::Code,
            source: "40 + 2".to_string(),
            cargo_toml: None,
            shared: false,
            collapsed: false,
            output_collapsed: false,
            version: 0,
            saved_output: None,
        }
    }

    /// Cell-emitted panel fields are untrusted; the export arms that splice
    /// them must escape, or a crafted `mime_type` / `kind` is XSS in the file.
    #[test]
    fn export_escapes_untrusted_panel_fields() {
        let mut nb = IronpadNotebook::new("t");
        nb.cells = vec![code_cell("a"), code_cell("b"), code_cell("c")];
        let mut dt = HashMap::new();
        // BlobImage: mime_type breaks out of the src attribute if unescaped.
        dt.insert(
            "a".to_string(),
            r#"[{"BlobImage":{"mime_type":"x\" onerror=\"alert(1)","base64_data":"AAA\"><script>x","width":1,"height":1}}]"#
                .to_string(),
        );
        // LiveView kind spliced into text.
        dt.insert(
            "b".to_string(),
            r#"[{"LiveView":{"fps":30,"kind":"<img src=x onerror=alert(2)>","content":""}}]"#
                .to_string(),
        );
        // Unknown interactive widget kind → fallback arm.
        dt.insert(
            "c".to_string(),
            r#"[{"Interactive":{"kind":"</div><script>alert(3)</script>","config":"{}"}}]"#
                .to_string(),
        );
        let html = build_export_html(&nb, &dt);

        // None of the raw payloads survive.
        assert!(
            !html.contains("onerror=\"alert(1)"),
            "BlobImage mime unescaped"
        );
        assert!(!html.contains("<script>x"), "BlobImage data unescaped");
        assert!(
            !html.contains("<img src=x onerror=alert(2)>"),
            "LiveView kind unescaped"
        );
        assert!(
            !html.contains("<script>alert(3)</script>"),
            "widget kind unescaped"
        );
        // The escaped forms are present instead.
        assert!(html.contains("onerror=&quot;alert(1)"));
        assert!(html.contains("&lt;img src=x onerror=alert(2)&gt;"));
        // And the export ships a script-forbidding CSP.
        assert!(html.contains("Content-Security-Policy"));
        assert!(html.contains("default-src 'none'"));
    }
}
