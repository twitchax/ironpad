//! Rustc JSON diagnostic parser.
//!
//! Parses the `--message-format=json` output produced by `cargo build` and
//! converts compiler messages into [`ironpad_common::Diagnostic`] types.
//!
//! Span line numbers are adjusted by subtracting the wrapper preamble offset
//! (see [`super::scaffold::WRAPPER_PREAMBLE_LINES`]) so that diagnostics
//! reference the user's original source lines rather than the generated `lib.rs`.

use ironpad_common::{Diagnostic, Severity, Span};
use serde::Deserialize;

use super::scaffold::WRAPPER_PREAMBLE_LINES;

// ── Rustc JSON Schema (subset) ──────────────────────────────────────────────

/// Top-level JSON object emitted by `cargo build --message-format=json`.
///
/// Each line of stdout is one of these. We only care about `"compiler-message"`.
#[derive(Deserialize)]
struct CargoMessage {
    reason: String,
    message: Option<RustcMessage>,
}

/// The compiler diagnostic message payload.
#[derive(Deserialize)]
struct RustcMessage {
    message: String,
    level: String,
    code: Option<RustcCode>,
    spans: Vec<RustcSpan>,
}

/// Optional error code attached to a diagnostic.
#[derive(Deserialize)]
struct RustcCode {
    code: String,
}

/// A source span reported by rustc.
#[derive(Deserialize)]
struct RustcSpan {
    file_name: String,
    line_start: u32,
    line_end: u32,
    column_start: u32,
    column_end: u32,
    is_primary: bool,
    label: Option<String>,
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Parse cargo's JSON stdout into a list of [`Diagnostic`]s.
///
/// Each line is parsed independently; malformed lines are silently skipped
/// (cargo may emit non-JSON progress lines to stdout in some configurations).
///
/// Only `"compiler-message"` entries with level `error`, `warning`, or `note`
/// are returned. Span line numbers are adjusted for the wrapper preamble.
pub fn parse_diagnostics(cargo_stdout: &str) -> Vec<Diagnostic> {
    cargo_stdout.lines().filter_map(parse_single_line).collect()
}

// ── Internal Helpers ────────────────────────────────────────────────────────

/// Attempt to parse a single JSON line into a [`Diagnostic`].
fn parse_single_line(line: &str) -> Option<Diagnostic> {
    let msg: CargoMessage = serde_json::from_str(line).ok()?;

    if msg.reason != "compiler-message" {
        return None;
    }

    let rustc_msg = msg.message?;

    let severity = match rustc_msg.level.as_str() {
        "error" | "error: internal compiler error" => Severity::Error,
        "warning" => Severity::Warning,
        "note" | "help" => Severity::Note,
        // Skip levels we don't map (e.g. "failure-note").
        _ => return None,
    };

    // Extract the error code if present.
    let code = rustc_msg.code.as_ref().map(|c| c.code.clone());

    let message = rustc_msg.message;

    // Only include primary spans from src/lib.rs (the wrapped user code file).
    let spans: Vec<Span> = rustc_msg
        .spans
        .into_iter()
        .filter(|s| s.is_primary && s.file_name == "src/lib.rs")
        .filter_map(adjust_span)
        .collect();

    Some(Diagnostic {
        message,
        severity,
        spans,
        code,
    })
}

/// Adjust a rustc span's line numbers by subtracting the wrapper preamble
/// offset, converting from generated-file coordinates to user-code coordinates.
///
/// Returns `None` if the span falls entirely within the preamble (i.e., the
/// error is in the auto-generated wrapper, not user code).
fn adjust_span(span: RustcSpan) -> Option<Span> {
    // Lines are 1-based in rustc output.
    let adjusted_start = span.line_start.checked_sub(WRAPPER_PREAMBLE_LINES)?;
    let adjusted_end = span.line_end.saturating_sub(WRAPPER_PREAMBLE_LINES);

    // If the adjusted start is 0, the span starts in the preamble.
    if adjusted_start == 0 {
        return None;
    }

    Some(Span {
        line_start: adjusted_start,
        line_end: adjusted_end.max(adjusted_start),
        col_start: span.column_start,
        col_end: span.column_end,
        label: span.label,
    })
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Sample rustc JSON payloads ──────────────────────────────────────

    /// A typical type-error diagnostic from rustc.
    const TYPE_ERROR_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0 (path+file:///tmp/cell)","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error[E0308]: mismatched types\n","children":[],"code":{"code":"E0308","explanation":"Expected type did not match the received type."},"level":"error","message":"mismatched types","spans":[{"byte_end":200,"byte_start":190,"column_end":15,"column_start":5,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"expected `i32`, found `&str`","line_end":6,"line_start":6,"suggested_replacement":null,"suggestion_applicability":null,"text":[{"highlight_end":15,"highlight_start":5,"text":"    \"hello\""}]}]}}"#;

    /// A warning diagnostic (unused variable).
    const WARNING_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"warning: unused variable: `x`\n","children":[{"children":[],"code":null,"level":"help","message":"if this is intentional, prefix it with an underscore: `_x`","rendered":null,"spans":[]}],"code":{"code":"unused_variables","explanation":null},"level":"warning","message":"unused variable: `x`","spans":[{"byte_end":150,"byte_start":149,"column_end":10,"column_start":9,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":null,"line_end":7,"line_start":7,"suggested_replacement":null,"suggestion_applicability":null,"text":[{"highlight_end":10,"highlight_start":9,"text":"    let x = 42;"}]}]}}"#;

    /// A non-message line (compiler artifact).
    const ARTIFACT_JSON: &str = r#"{"reason":"compiler-artifact","package_id":"serde 1.0.0","manifest_path":"/reg/serde/Cargo.toml","target":{"kind":["lib"],"crate_types":["lib"],"name":"serde","src_path":"/reg/serde/src/lib.rs","edition":"2021","doc":true,"doctest":true,"test":true},"profile":{"opt_level":"3","debuginfo":0,"debug_assertions":false,"overflow_checks":false,"test":false},"features":["default","derive","std"],"filenames":["/tmp/target/release/libserde.rlib"],"executable":null,"fresh":true}"#;

    /// A note-level message (e.g., aborting due to previous error).
    const NOTE_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error: aborting due to 1 previous error\n","children":[],"code":null,"level":"error","message":"aborting due to 1 previous error","spans":[]}}"#;

    /// A warning with a span in the preamble (line 2, inside the wrapper).
    const PREAMBLE_SPAN_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"warning: something in preamble\n","children":[],"code":null,"level":"warning","message":"something in preamble","spans":[{"byte_end":30,"byte_start":20,"column_end":10,"column_start":1,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"in preamble","line_end":2,"line_start":2,"suggested_replacement":null,"suggestion_applicability":null,"text":[]}]}}"#;

    /// An error with a span in a dependency file (not src/lib.rs).
    const DEPENDENCY_SPAN_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error: something\n","children":[],"code":null,"level":"error","message":"error in dependency","spans":[{"byte_end":100,"byte_start":90,"column_end":10,"column_start":5,"expansion":null,"file_name":"/home/user/.cargo/registry/src/some-crate/src/lib.rs","is_primary":true,"label":"here","line_end":10,"line_start":10,"suggested_replacement":null,"suggestion_applicability":null,"text":[]}]}}"#;

    /// Multi-span error (e.g., borrow checker with two spans).
    const MULTI_SPAN_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error[E0502]: cannot borrow\n","children":[],"code":{"code":"E0502","explanation":null},"level":"error","message":"cannot borrow `v` as mutable because it is also borrowed as immutable","spans":[{"byte_end":200,"byte_start":190,"column_end":20,"column_start":5,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"mutable borrow occurs here","line_end":8,"line_start":8,"suggested_replacement":null,"suggestion_applicability":null,"text":[]},{"byte_end":150,"byte_start":140,"column_end":15,"column_start":5,"expansion":null,"file_name":"src/lib.rs","is_primary":false,"label":"immutable borrow occurs here","line_end":6,"line_start":6,"suggested_replacement":null,"suggestion_applicability":null,"text":[]}]}}"#;

    // ── parse_diagnostics ───────────────────────────────────────────────

    #[test]
    fn parses_type_error() {
        let diags = parse_diagnostics(TYPE_ERROR_JSON);

        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(diags[0].message.contains("mismatched types"));
        assert_eq!(diags[0].code.as_deref(), Some("E0308"));
        assert_eq!(diags[0].spans.len(), 1);

        let span = &diags[0].spans[0];
        // Original line 6 - WRAPPER_PREAMBLE_LINES (4) = line 2 in user code.
        assert_eq!(span.line_start, 2);
        assert_eq!(span.line_end, 2);
        assert_eq!(span.col_start, 5);
        assert_eq!(span.col_end, 15);
        assert_eq!(span.label.as_deref(), Some("expected `i32`, found `&str`"));
    }

    #[test]
    fn parses_warning() {
        let diags = parse_diagnostics(WARNING_JSON);

        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Warning);
        assert!(diags[0].message.contains("unused variable"));
        assert_eq!(diags[0].code.as_deref(), Some("unused_variables"));
        assert_eq!(diags[0].spans.len(), 1);

        let span = &diags[0].spans[0];
        // Original line 7 - 4 = line 3 in user code.
        assert_eq!(span.line_start, 3);
        assert_eq!(span.line_end, 3);
    }

    #[test]
    fn skips_artifact_lines() {
        let diags = parse_diagnostics(ARTIFACT_JSON);
        assert!(diags.is_empty());
    }

    #[test]
    fn parses_note_level_as_error_when_level_is_error() {
        // "aborting due to 1 previous error" has level "error" with no spans.
        let diags = parse_diagnostics(NOTE_JSON);

        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(diags[0].message.contains("aborting"));
        assert!(diags[0].spans.is_empty());
    }

    #[test]
    fn filters_preamble_spans() {
        let diags = parse_diagnostics(PREAMBLE_SPAN_JSON);

        assert_eq!(diags.len(), 1);
        // Span at line 2 is within the 4-line preamble, so it should be filtered out.
        assert!(diags[0].spans.is_empty());
    }

    #[test]
    fn filters_dependency_file_spans() {
        let diags = parse_diagnostics(DEPENDENCY_SPAN_JSON);

        assert_eq!(diags.len(), 1);
        // Span is in a dependency file, not src/lib.rs — should be filtered.
        assert!(diags[0].spans.is_empty());
    }

    #[test]
    fn only_includes_primary_spans() {
        let diags = parse_diagnostics(MULTI_SPAN_JSON);

        assert_eq!(diags.len(), 1);
        // The second span (line 6) is not primary — only primary spans are included.
        assert_eq!(diags[0].spans.len(), 1);

        let span = &diags[0].spans[0];
        // Original line 8 - 4 = line 4 in user code.
        assert_eq!(span.line_start, 4);
        assert_eq!(span.line_end, 4);
        assert_eq!(span.label.as_deref(), Some("mutable borrow occurs here"));
    }

    #[test]
    fn handles_multiline_cargo_output() {
        let combined = format!(
            "{}\n{}\n{}\n{}",
            ARTIFACT_JSON, TYPE_ERROR_JSON, WARNING_JSON, NOTE_JSON
        );
        let diags = parse_diagnostics(&combined);

        // Artifact skipped, type error + warning + note-error = 3 diagnostics.
        assert_eq!(diags.len(), 3);
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(diags[1].severity, Severity::Warning);
        assert_eq!(diags[2].severity, Severity::Error);
    }

    #[test]
    fn handles_empty_input() {
        let diags = parse_diagnostics("");
        assert!(diags.is_empty());
    }

    #[test]
    fn handles_malformed_json_lines() {
        let input = "not json at all\n{\"bad\": true}\n";
        let diags = parse_diagnostics(input);
        assert!(diags.is_empty());
    }

    #[test]
    fn line_adjustment_preserves_multiline_spans() {
        // Simulate a span that crosses multiple user-code lines.
        let span = RustcSpan {
            file_name: "src/lib.rs".to_string(),
            line_start: 6,
            line_end: 10,
            column_start: 1,
            column_end: 20,
            is_primary: true,
            label: Some("multiline".to_string()),
        };

        let adjusted = adjust_span(span).unwrap();
        // 6 - 4 = 2, 10 - 4 = 6
        assert_eq!(adjusted.line_start, 2);
        assert_eq!(adjusted.line_end, 6);
        assert_eq!(adjusted.col_start, 1);
        assert_eq!(adjusted.col_end, 20);
    }

    // ── adjust_span edge cases ──────────────────────────────────────────

    #[test]
    fn adjust_span_at_exact_preamble_boundary() {
        // Line 4 is the last preamble line (WRAPPER_PREAMBLE_LINES = 4).
        // 4 - 4 = 0, which means it's still in the preamble.
        let span = RustcSpan {
            file_name: "src/lib.rs".to_string(),
            line_start: 4,
            line_end: 4,
            column_start: 1,
            column_end: 10,
            is_primary: true,
            label: None,
        };
        assert!(adjust_span(span).is_none());
    }

    #[test]
    fn adjust_span_first_user_line() {
        // Line 5 is the first user code line (5 - 4 = 1).
        let span = RustcSpan {
            file_name: "src/lib.rs".to_string(),
            line_start: 5,
            line_end: 5,
            column_start: 1,
            column_end: 30,
            is_primary: true,
            label: Some("first user line".to_string()),
        };
        let adjusted = adjust_span(span).unwrap();
        assert_eq!(adjusted.line_start, 1);
        assert_eq!(adjusted.line_end, 1);
    }
}
