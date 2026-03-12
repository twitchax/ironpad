#[cfg(feature = "hydrate")]
use std::collections::HashMap;

#[cfg(feature = "hydrate")]
use ironpad_common::{CellType, IronpadNotebook};

#[cfg(feature = "hydrate")]
use crate::components::markdown_cell::render_markdown;

// ── Display panels ──────────────────────────────────────────────────────────

/// Display panel types matching ironpad-cell's DisplayPanel enum.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) enum DisplayPanel {
    Text(String),
    Html(String),
    Svg(String),
    Markdown(String),
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Interactive UI widget (slider, dropdown, checkbox, etc.).
    Interactive {
        kind: String,
        config: String,
    },
}

// ── Export HTML helpers ─────────────────────────────────────────────────────

#[cfg(feature = "hydrate")]
const EXPORT_CSS: &str = r#"
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
"#;

/// Build a self-contained HTML document from a notebook and its cached display texts.
#[cfg(feature = "hydrate")]
pub(super) fn build_export_html(
    nb: &IronpadNotebook,
    display_texts: &HashMap<String, String>,
) -> String {
    let mut html = String::with_capacity(8192);

    // Document header.
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    html.push_str("<meta charset=\"UTF-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n");
    html.push_str(&format!("<title>{}</title>\n", html_escape(&nb.title)));
    html.push_str("<style>\n");
    html.push_str(EXPORT_CSS);
    html.push_str("</style>\n</head>\n<body>\n");

    // Title.
    html.push_str(&format!(
        "<h1 class=\"notebook-title\">{}</h1>\n",
        html_escape(&nb.title)
    ));

    // Cells.
    for cell in &nb.cells {
        html.push_str("<div class=\"cell\">\n");
        html.push_str(&format!(
            "<div class=\"cell-label\">{}</div>\n",
            html_escape(&cell.label)
        ));

        match cell.cell_type {
            CellType::Code => {
                html.push_str(&format!(
                    "<pre class=\"code-block\"><code>{}</code></pre>\n",
                    html_escape(&cell.source)
                ));

                // Include cached output if available.
                if let Some(display_json) = display_texts.get(&cell.id) {
                    if let Ok(panels) = serde_json::from_str::<Vec<DisplayPanel>>(display_json) {
                        html.push_str("<div class=\"cell-output\">\n");
                        html.push_str("<div class=\"output-label\">Output</div>\n");
                        for panel in &panels {
                            match panel {
                                DisplayPanel::Text(text) => {
                                    html.push_str(&format!("<pre>{}</pre>\n", html_escape(text)));
                                }
                                DisplayPanel::Html(h) => {
                                    html.push_str(&format!(
                                        "<div class=\"output-html\">{h}</div>\n"
                                    ));
                                }
                                DisplayPanel::Svg(s) => {
                                    html.push_str(&format!(
                                        "<div class=\"output-svg\">{s}</div>\n"
                                    ));
                                }
                                DisplayPanel::Markdown(md) => {
                                    let rendered = render_markdown(md);
                                    html.push_str(&format!(
                                        "<div class=\"ironpad-markdown-cell-preview\">{rendered}</div>\n"
                                    ));
                                }
                                DisplayPanel::Table { headers, rows } => {
                                    html.push_str(
                                        "<table class=\"ironpad-output-table\"><thead><tr>",
                                    );
                                    for h in headers {
                                        html.push_str(&format!("<th>{}</th>", html_escape(h)));
                                    }
                                    html.push_str("</tr></thead><tbody>");
                                    for row in rows {
                                        html.push_str("<tr>");
                                        for cell in row {
                                            html.push_str(&format!(
                                                "<td>{}</td>",
                                                html_escape(cell)
                                            ));
                                        }
                                        html.push_str("</tr>");
                                    }
                                    html.push_str("</tbody></table>\n");
                                }
                                DisplayPanel::Interactive { kind, config } => {
                                    render_interactive_static(&mut html, kind, config);
                                }
                            }
                        }
                        html.push_str("</div>\n");
                    }
                }
            }
            CellType::Markdown => {
                let rendered = render_markdown(&cell.source);
                html.push_str(&format!(
                    "<div class=\"markdown-content\">{rendered}</div>\n"
                ));
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
#[cfg(feature = "hydrate")]
fn render_interactive_static(html: &mut String, kind: &str, config: &str) {
    let cfg: serde_json::Value = serde_json::from_str(config).unwrap_or_default();
    let label = cfg.get("label").and_then(|v| v.as_str()).unwrap_or("");

    match kind {
        "slider" | "number" => {
            let default = cfg.get("default").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let label_text = if label.is_empty() {
                kind.to_owned()
            } else {
                label.to_owned()
            };
            html.push_str(&format!(
                "<div class=\"ironpad-interactive-static\"><strong>{}</strong>: {default}</div>\n",
                html_escape(&label_text)
            ));
        }
        "dropdown" => {
            let default = cfg.get("default").and_then(|v| v.as_str()).unwrap_or("");
            let label_text = if label.is_empty() { "dropdown" } else { label };
            html.push_str(&format!(
                "<div class=\"ironpad-interactive-static\"><strong>{}</strong>: {}</div>\n",
                html_escape(label_text),
                html_escape(default)
            ));
        }
        "checkbox" | "switch" => {
            let default = cfg
                .get("default")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let icon = if default { "☑" } else { "☐" };
            html.push_str(&format!(
                "<div class=\"ironpad-interactive-static\">{icon} {}</div>\n",
                html_escape(label)
            ));
        }
        "text_input" => {
            let default = cfg.get("default").and_then(|v| v.as_str()).unwrap_or("");
            let label_text = if label.is_empty() { "text" } else { label };
            html.push_str(&format!(
                "<div class=\"ironpad-interactive-static\"><strong>{}</strong>: {}</div>\n",
                html_escape(label_text),
                html_escape(default)
            ));
        }
        _ => {
            html.push_str(&format!(
                "<div class=\"ironpad-interactive-static\">[{kind} widget]</div>\n"
            ));
        }
    }
}

/// Minimal HTML entity escaping for text content.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render a table as an HTML `<table>` string with the `ironpad-output-table` class.
pub(super) fn render_table_html(headers: &[String], rows: &[Vec<String>]) -> String {
    let mut html = String::from("<table class=\"ironpad-output-table\"><thead><tr>");
    for h in headers {
        html.push_str(&format!("<th>{}</th>", html_escape(h)));
    }
    html.push_str("</tr></thead><tbody>");
    for row in rows {
        html.push_str("<tr>");
        for cell in row {
            html.push_str(&format!("<td>{}</td>", html_escape(cell)));
        }
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table>");
    html
}

/// Render a table as tab-separated values for clipboard copy.
pub(super) fn render_table_tsv(headers: &[String], rows: &[Vec<String>]) -> String {
    let mut tsv = headers.join("\t");
    for row in rows {
        tsv.push('\n');
        tsv.push_str(&row.join("\t"));
    }
    tsv
}

/// Trigger a browser file download from an HTML string.
#[cfg(feature = "hydrate")]
pub(super) fn trigger_html_download(html_content: &str, title: &str) {
    use wasm_bindgen::JsCast;

    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };

    // Build a Blob from the HTML string.
    let parts = js_sys::Array::new();
    parts.push(&wasm_bindgen::JsValue::from_str(html_content));

    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("text/html;charset=utf-8");

    let blob = match web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts) {
        Ok(b) => b,
        Err(_) => return,
    };

    let url = match web_sys::Url::create_object_url_with_blob(&blob) {
        Ok(u) => u,
        Err(_) => return,
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
    let filename = format!(
        "{}.html",
        title
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            })
            .collect::<String>()
    );
    anchor.set_download(&filename);
    anchor.set_attribute("style", "display:none").ok();

    if let Some(body) = document.body() {
        let _ = body.append_child(&anchor);
        anchor.click();
        let _ = body.remove_child(&anchor);
    }

    let _ = web_sys::Url::revoke_object_url(&url);
}
