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

    // ── T-034: Focused error-type tests ─────────────────────────────────

    // ── Syntax error payloads ───────────────────────────────────────────

    /// Syntax error: missing semicolon.
    const SYNTAX_MISSING_SEMI_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0 (path+file:///tmp/cell)","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error: expected `;`\n","children":[],"code":null,"level":"error","message":"expected `;`","spans":[{"byte_end":180,"byte_start":179,"column_end":18,"column_start":17,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"expected `;`","line_end":7,"line_start":7,"suggested_replacement":null,"suggestion_applicability":null,"text":[{"highlight_end":18,"highlight_start":17,"text":"    let x = 42"}]}]}}"#;

    /// Syntax error: unexpected closing brace (reported on the wrapper's closing `}`).
    /// In this scenario the error span falls on the closing brace of `cell_main`,
    /// which is one line past the end of the user code. For a 3-line user snippet
    /// the closing brace is at wrapper line 4 + 3 + 1 = 8.
    const SYNTAX_UNEXPECTED_BRACE_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error: unexpected closing delimiter: `}`\n","children":[],"code":null,"level":"error","message":"unexpected closing delimiter: `}`","spans":[{"byte_end":250,"byte_start":249,"column_end":2,"column_start":1,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"unexpected closing delimiter","line_end":8,"line_start":8,"suggested_replacement":null,"suggestion_applicability":null,"text":[{"highlight_end":2,"highlight_start":1,"text":"}"}]}]}}"#;

    /// Syntax error: unmatched opening delimiter with a span crossing lines.
    /// Simulates `{` opened on user line 1 (wrapper line 5) never closed,
    /// error reported spanning lines 5–9 in the wrapper.
    const SYNTAX_UNCLOSED_DELIMITER_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error: unclosed delimiter\n","children":[],"code":null,"level":"error","message":"unclosed delimiter","spans":[{"byte_end":300,"byte_start":160,"column_end":1,"column_start":5,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"unclosed delimiter","line_end":9,"line_start":5,"suggested_replacement":null,"suggestion_applicability":null,"text":[]}]}}"#;

    // ── Borrow checker error payloads ───────────────────────────────────

    /// E0382: use of moved value.
    const BORROW_USE_AFTER_MOVE_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error[E0382]: use of moved value: `s`\n","children":[],"code":{"code":"E0382","explanation":null},"level":"error","message":"use of moved value: `s`","spans":[{"byte_end":220,"byte_start":215,"column_end":10,"column_start":5,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"value used here after move","line_end":9,"line_start":9,"suggested_replacement":null,"suggestion_applicability":null,"text":[{"highlight_end":10,"highlight_start":5,"text":"    println!(\"{}\", s);"}]},{"byte_end":190,"byte_start":180,"column_end":20,"column_start":10,"expansion":null,"file_name":"src/lib.rs","is_primary":false,"label":"value moved here","line_end":7,"line_start":7,"suggested_replacement":null,"suggestion_applicability":null,"text":[]}]}}"#;

    /// E0505: cannot move out of borrowed content.
    const BORROW_MOVE_WHILE_BORROWED_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error[E0505]: cannot move out of `v` because it is borrowed\n","children":[],"code":{"code":"E0505","explanation":null},"level":"error","message":"cannot move out of `v` because it is borrowed","spans":[{"byte_end":250,"byte_start":240,"column_end":15,"column_start":5,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"move out of `v` occurs here","line_end":10,"line_start":10,"suggested_replacement":null,"suggestion_applicability":null,"text":[{"highlight_end":15,"highlight_start":5,"text":"    drop(v);"}]}]}}"#;

    // ── Lifetime error payloads ─────────────────────────────────────────

    /// E0106: missing lifetime specifier.
    const LIFETIME_ERROR_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error[E0106]: missing lifetime specifier\n","children":[],"code":{"code":"E0106","explanation":null},"level":"error","message":"missing lifetime specifier","spans":[{"byte_end":200,"byte_start":190,"column_end":25,"column_start":15,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"expected named lifetime parameter","line_end":6,"line_start":6,"suggested_replacement":null,"suggestion_applicability":null,"text":[{"highlight_end":25,"highlight_start":15,"text":"    fn foo(x: &str) -> &str {"}]}]}}"#;

    // ── Column offset edge-case payloads ────────────────────────────────

    /// Error at column 1 — first character of user code line.
    const COLUMN_ONE_ERROR_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error: expected item\n","children":[],"code":null,"level":"error","message":"expected item, found `42`","spans":[{"byte_end":161,"byte_start":160,"column_end":2,"column_start":1,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"expected item","line_end":5,"line_start":5,"suggested_replacement":null,"suggestion_applicability":null,"text":[{"highlight_end":2,"highlight_start":1,"text":"42"}]}]}}"#;

    /// Error at a high column offset (deeply indented code).
    const HIGH_COLUMN_ERROR_JSON: &str = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error[E0308]: mismatched types\n","children":[],"code":{"code":"E0308","explanation":null},"level":"error","message":"mismatched types","spans":[{"byte_end":400,"byte_start":370,"column_end":60,"column_start":30,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"expected `u64`, found `&str`","line_end":12,"line_start":12,"suggested_replacement":null,"suggestion_applicability":null,"text":[{"highlight_end":60,"highlight_start":30,"text":"                              \"this is deeply indented\""}]}]}}"#;

    // ── Syntax error tests ──────────────────────────────────────────────

    #[test]
    fn parses_syntax_error_missing_semicolon() {
        let diags = parse_diagnostics(SYNTAX_MISSING_SEMI_JSON);

        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(diags[0].message.contains("expected `;`"));
        // Syntax errors typically have no error code.
        assert!(diags[0].code.is_none());
        assert_eq!(diags[0].spans.len(), 1);

        let span = &diags[0].spans[0];
        // Wrapper line 7 - 4 = user line 3.
        assert_eq!(span.line_start, 3);
        assert_eq!(span.line_end, 3);
        assert_eq!(span.col_start, 17);
        assert_eq!(span.col_end, 18);
        assert_eq!(span.label.as_deref(), Some("expected `;`"));
    }

    #[test]
    fn parses_syntax_error_unexpected_closing_brace() {
        let diags = parse_diagnostics(SYNTAX_UNEXPECTED_BRACE_JSON);

        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(diags[0].message.contains("unexpected closing delimiter"));
        assert_eq!(diags[0].spans.len(), 1);

        let span = &diags[0].spans[0];
        // Wrapper line 8 - 4 = user line 4 (past end of 3-line user code).
        // This is expected: errors on the wrapper closing brace map to
        // one line past the user code.
        assert_eq!(span.line_start, 4);
        assert_eq!(span.line_end, 4);
        assert_eq!(span.col_start, 1);
        assert_eq!(span.col_end, 2);
    }

    #[test]
    fn parses_syntax_error_unclosed_delimiter_multiline() {
        let diags = parse_diagnostics(SYNTAX_UNCLOSED_DELIMITER_JSON);

        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("unclosed delimiter"));
        assert_eq!(diags[0].spans.len(), 1);

        let span = &diags[0].spans[0];
        // Wrapper lines 5–9 map to user lines 1–5.
        assert_eq!(span.line_start, 1);
        assert_eq!(span.line_end, 5);
        assert_eq!(span.col_start, 5);
        assert_eq!(span.col_end, 1);
    }

    // ── Borrow checker error tests ──────────────────────────────────────

    #[test]
    fn parses_use_after_move_error() {
        let diags = parse_diagnostics(BORROW_USE_AFTER_MOVE_JSON);

        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(diags[0].message.contains("use of moved value"));
        assert_eq!(diags[0].code.as_deref(), Some("E0382"));

        // Only the primary span (line 9, "value used here after move") is kept.
        // The secondary span (line 7, "value moved here") is filtered out.
        assert_eq!(diags[0].spans.len(), 1);

        let span = &diags[0].spans[0];
        // Wrapper line 9 - 4 = user line 5.
        assert_eq!(span.line_start, 5);
        assert_eq!(span.line_end, 5);
        assert_eq!(span.col_start, 5);
        assert_eq!(span.col_end, 10);
        assert_eq!(span.label.as_deref(), Some("value used here after move"));
    }

    #[test]
    fn parses_move_while_borrowed_error() {
        let diags = parse_diagnostics(BORROW_MOVE_WHILE_BORROWED_JSON);

        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(diags[0].code.as_deref(), Some("E0505"));

        assert_eq!(diags[0].spans.len(), 1);

        let span = &diags[0].spans[0];
        // Wrapper line 10 - 4 = user line 6.
        assert_eq!(span.line_start, 6);
        assert_eq!(span.line_end, 6);
        assert_eq!(span.col_start, 5);
        assert_eq!(span.col_end, 15);
        assert_eq!(span.label.as_deref(), Some("move out of `v` occurs here"));
    }

    // ── Lifetime error tests ────────────────────────────────────────────

    #[test]
    fn parses_lifetime_error() {
        let diags = parse_diagnostics(LIFETIME_ERROR_JSON);

        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(diags[0].code.as_deref(), Some("E0106"));
        assert!(diags[0].message.contains("missing lifetime specifier"));

        assert_eq!(diags[0].spans.len(), 1);

        let span = &diags[0].spans[0];
        // Wrapper line 6 - 4 = user line 2.
        assert_eq!(span.line_start, 2);
        assert_eq!(span.line_end, 2);
        assert_eq!(span.col_start, 15);
        assert_eq!(span.col_end, 25);
        assert_eq!(
            span.label.as_deref(),
            Some("expected named lifetime parameter")
        );
    }

    // ── Column offset tests ─────────────────────────────────────────────

    #[test]
    fn column_offsets_preserved_at_column_one() {
        let diags = parse_diagnostics(COLUMN_ONE_ERROR_JSON);

        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].spans.len(), 1);

        let span = &diags[0].spans[0];
        // Wrapper line 5 - 4 = user line 1.
        assert_eq!(span.line_start, 1);
        assert_eq!(span.line_end, 1);
        // Columns must pass through unchanged since user code is not indented
        // inside the wrapper.
        assert_eq!(span.col_start, 1);
        assert_eq!(span.col_end, 2);
    }

    #[test]
    fn column_offsets_preserved_at_high_column() {
        let diags = parse_diagnostics(HIGH_COLUMN_ERROR_JSON);

        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].spans.len(), 1);

        let span = &diags[0].spans[0];
        // Wrapper line 12 - 4 = user line 8.
        assert_eq!(span.line_start, 8);
        assert_eq!(span.line_end, 8);
        // High column values must survive adjustment untouched.
        assert_eq!(span.col_start, 30);
        assert_eq!(span.col_end, 60);
    }

    // ── Mixed-output integration test ───────────────────────────────────

    #[test]
    fn mixed_error_types_in_single_compilation() {
        // Simulates a realistic compilation that produces multiple diagnostics:
        // a syntax error, a type error, a warning, and a note.
        let combined = format!(
            "{}\n{}\n{}\n{}\n{}",
            ARTIFACT_JSON, SYNTAX_MISSING_SEMI_JSON, TYPE_ERROR_JSON, WARNING_JSON, NOTE_JSON,
        );
        let diags = parse_diagnostics(&combined);

        // Artifact skipped. Remaining 4 are: syntax error, type error, warning, note-error.
        assert_eq!(diags.len(), 4);
        assert_eq!(diags[0].severity, Severity::Error); // syntax
        assert_eq!(diags[1].severity, Severity::Error); // type
        assert_eq!(diags[2].severity, Severity::Warning); // warning
        assert_eq!(diags[3].severity, Severity::Error); // note (level=error)

        // Verify span mapping is correct for each.
        assert_eq!(diags[0].spans[0].line_start, 3); // syntax: line 7-4=3
        assert_eq!(diags[1].spans[0].line_start, 2); // type: line 6-4=2
        assert_eq!(diags[2].spans[0].line_start, 3); // warning: line 7-4=3
        assert!(diags[3].spans.is_empty()); // note has no spans
    }

    // ── adjust_span unit tests for T-034 edge cases ─────────────────────

    #[test]
    fn adjust_span_closing_brace_line() {
        // The wrapper's closing `}` is one line past the user code.
        // For a 5-line user snippet, closing brace is at wrapper line 10.
        let span = RustcSpan {
            file_name: "src/lib.rs".to_string(),
            line_start: 10,
            line_end: 10,
            column_start: 1,
            column_end: 2,
            is_primary: true,
            label: Some("closing brace".to_string()),
        };

        let adjusted = adjust_span(span).unwrap();
        // 10 - 4 = 6, which is one past the 5-line user code. This is expected.
        assert_eq!(adjusted.line_start, 6);
        assert_eq!(adjusted.line_end, 6);
        assert_eq!(adjusted.col_start, 1);
        assert_eq!(adjusted.col_end, 2);
    }

    #[test]
    fn adjust_span_single_char_column_range() {
        // Error on a single character (e.g., a stray `@` on user line 1).
        let span = RustcSpan {
            file_name: "src/lib.rs".to_string(),
            line_start: 5,
            line_end: 5,
            column_start: 10,
            column_end: 11,
            is_primary: true,
            label: Some("unexpected character".to_string()),
        };

        let adjusted = adjust_span(span).unwrap();
        assert_eq!(adjusted.line_start, 1);
        assert_eq!(adjusted.line_end, 1);
        assert_eq!(adjusted.col_start, 10);
        assert_eq!(adjusted.col_end, 11);
    }

    #[test]
    fn adjust_span_spanning_from_preamble_into_user_code_is_rejected() {
        // A span that starts in the preamble (line 3) and ends in user code (line 6).
        // This should be rejected because adjusted_start = 3 - 4 = underflow → None.
        let span = RustcSpan {
            file_name: "src/lib.rs".to_string(),
            line_start: 3,
            line_end: 6,
            column_start: 1,
            column_end: 10,
            is_primary: true,
            label: Some("crosses preamble boundary".to_string()),
        };

        // checked_sub(4) on line_start=3 returns None, so span is rejected.
        assert!(adjust_span(span).is_none());
    }
}
