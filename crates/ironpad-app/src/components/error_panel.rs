/// Dedicated error panel for displaying formatted compiler diagnostics.
///
/// Renders diagnostics below the editor when compilation fails, with:
/// - Color-coded severity badges (error / warning / note)
/// - Source spans with line numbers
/// - Clickable links to the Rust error index for known error codes
/// - Collapsible panel header with error/warning counts
use ironpad_common::{Diagnostic, Severity};
use leptos::prelude::*;

// ── Error panel component ───────────────────────────────────────────────────

/// A rich error panel that displays compiler diagnostics grouped by severity.
///
/// Shows a collapsible header with summary counts and renders each diagnostic
/// with formatted spans and error code links.  Intended to replace the output
/// panel when compilation errors are present.
#[component]
pub fn ErrorPanel(
    /// The list of diagnostics to display.
    #[prop(into)]
    diagnostics: Vec<Diagnostic>,
) -> impl IntoView {
    let collapsed = RwSignal::new(false);

    let error_count = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warning_count = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    let note_count = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Note)
        .count();

    let summary = build_summary(error_count, warning_count, note_count);

    view! {
        <div class="ironpad-error-panel">
            <div
                class="ironpad-error-panel-header"
                on:click=move |_| collapsed.update(|c| *c = !*c)
            >
                <span class="ironpad-error-panel-toggle">
                    {move || if collapsed.get() { "▸" } else { "▾" }}
                </span>
                <span class="ironpad-error-panel-title">"Diagnostics"</span>
                <span class="ironpad-error-panel-summary">{summary}</span>
            </div>

            {move || {
                if collapsed.get() {
                    return view! { <div /> }.into_any();
                }

                let diags = diagnostics.clone();
                view! {
                    <div class="ironpad-error-panel-body">
                        <For
                            each=move || diags.clone()
                            key=|d| format!("{:?}:{}", d.severity, d.message)
                            let:diag
                        >
                            <ErrorDiagnosticItem diagnostic=diag />
                        </For>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

// ── Individual diagnostic item ──────────────────────────────────────────────

/// Renders a single compiler diagnostic with severity badge, message,
/// optional error code link, and source span details.
#[component]
fn ErrorDiagnosticItem(diagnostic: Diagnostic) -> impl IntoView {
    let severity_class = match diagnostic.severity {
        Severity::Error => "ironpad-error-diag--error",
        Severity::Warning => "ironpad-error-diag--warning",
        Severity::Note => "ironpad-error-diag--note",
    };

    let severity_label = match diagnostic.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
    };

    let error_code_link = diagnostic.code.as_ref().and_then(|code| {
        // Only Rust error codes matching `E\d+` are linkable.
        if code.starts_with('E') && code.len() > 1 && code[1..].chars().all(|c| c.is_ascii_digit())
        {
            Some((
                format!("https://doc.rust-lang.org/error_codes/{code}.html"),
                code.clone(),
            ))
        } else {
            // Non-linkable codes (e.g., lint names like "unused_variables")
            // are still displayed inline.
            None
        }
    });

    let display_code = diagnostic.code.clone();
    let spans = diagnostic.spans.clone();

    view! {
        <div class=format!("ironpad-error-diag {severity_class}")>
            // Severity badge
            <div class="ironpad-error-diag-header">
                <span class="ironpad-error-diag-severity">{severity_label}</span>

                // Error code: linked if it's an E-code, plain text otherwise
                {match error_code_link {
                    Some((url, code)) => {
                        view! {
                            <a
                                class="ironpad-error-diag-code ironpad-error-diag-code--link"
                                href=url
                                target="_blank"
                                rel="noopener noreferrer"
                            >
                                {format!("[{code}]")}
                            </a>
                        }.into_any()
                    }
                    None => {
                        match display_code {
                            Some(code) => view! {
                                <span class="ironpad-error-diag-code">
                                    {format!("[{code}]")}
                                </span>
                            }.into_any(),
                            None => view! { <span /> }.into_any(),
                        }
                    }
                }}
            </div>

            // Message text
            <div class="ironpad-error-diag-message">{diagnostic.message.clone()}</div>

            // Source spans with line numbers
            {if !spans.is_empty() {
                view! {
                    <div class="ironpad-error-diag-spans">
                        <For
                            each=move || spans.clone()
                            key=|s| format!("{}:{}:{}:{}", s.line_start, s.col_start, s.line_end, s.col_end)
                            let:span
                        >
                            <SpanItem span=span />
                        </For>
                    </div>
                }.into_any()
            } else {
                view! { <span /> }.into_any()
            }}
        </div>
    }
}

// ── Span item ───────────────────────────────────────────────────────────────

/// Renders a single source span with line/column location and optional label.
#[component]
fn SpanItem(span: ironpad_common::Span) -> impl IntoView {
    let location = if span.line_start == span.line_end {
        format!(
            "line {}:{}-{}",
            span.line_start, span.col_start, span.col_end
        )
    } else {
        format!(
            "lines {}:{} – {}:{}",
            span.line_start, span.col_start, span.line_end, span.col_end
        )
    };

    view! {
        <div class="ironpad-error-span">
            <span class="ironpad-error-span-location">{location}</span>
            {span.label.map(|label| view! {
                <span class="ironpad-error-span-label">{label}</span>
            })}
        </div>
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Build a human-readable summary string like "2 errors, 1 warning".
fn build_summary(errors: usize, warnings: usize, notes: usize) -> String {
    let mut parts = Vec::new();

    if errors > 0 {
        parts.push(format!(
            "{errors} error{}",
            if errors == 1 { "" } else { "s" }
        ));
    }
    if warnings > 0 {
        parts.push(format!(
            "{warnings} warning{}",
            if warnings == 1 { "" } else { "s" }
        ));
    }
    if notes > 0 {
        parts.push(format!("{notes} note{}", if notes == 1 { "" } else { "s" }));
    }

    if parts.is_empty() {
        "no diagnostics".to_string()
    } else {
        parts.join(", ")
    }
}
