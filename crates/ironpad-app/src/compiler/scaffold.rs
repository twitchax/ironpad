//! Micro-crate scaffolding for cell compilation.
//!
//! Given a cell's source code and Cargo.toml, this module writes a complete
//! micro-crate to disk that can be compiled to WASM with `cargo build`.

use std::path::{Path, PathBuf};

// ── Public API ───────────────────────────────────────────────────────────────

/// Scaffold a micro-crate for a single cell compilation.
///
/// Creates the directory structure:
///
/// ```text
/// {cache_dir}/workspaces/{session_id}/{cell_id}/
///   Cargo.toml
///   src/
///     lib.rs
/// ```
///
/// Returns `(crate_dir, preamble_lines, is_async)`:
/// - `crate_dir`: path to the micro-crate root directory
/// - `preamble_lines`: number of lines before user code (for diagnostic mapping)
/// - `is_async`: whether the cell wrapper is async (source contains `.await`)
#[allow(clippy::too_many_arguments)]
pub fn scaffold_micro_crate(
    cache_dir: &Path,
    ironpad_cell_path: &Path,
    session_id: &str,
    cell_id: &str,
    source: &str,
    cargo_toml: &str,
    previous_cell_types: &[Option<String>],
    shared_cargo_toml: Option<&str>,
) -> anyhow::Result<(PathBuf, u32, bool)> {
    let crate_dir = cache_dir.join("workspaces").join(session_id).join(cell_id);

    let src_dir = crate_dir.join("src");
    std::fs::create_dir_all(&src_dir)?;

    // Resolve ironpad-cell path to absolute so the micro-crate can find it
    // regardless of the working directory used by `cargo build`.
    let absolute_cell_path = std::fs::canonicalize(ironpad_cell_path).unwrap_or_else(|_| {
        // Fall back to the raw path if canonicalize fails (e.g. path doesn't exist yet).
        ironpad_cell_path.to_path_buf()
    });

    let generated_cargo_toml =
        generate_cargo_toml(cell_id, cargo_toml, &absolute_cell_path, shared_cargo_toml);
    std::fs::write(crate_dir.join("Cargo.toml"), generated_cargo_toml)?;

    let (lib_rs, preamble_lines, is_async) = generate_lib_rs(source, previous_cell_types);
    std::fs::write(src_dir.join("lib.rs"), lib_rs)?;

    Ok((crate_dir, preamble_lines, is_async))
}

// ── Cargo.toml Generation ────────────────────────────────────────────────────

/// Build a complete `Cargo.toml` for the micro-crate.
///
/// Merges the user-provided dependency lines with the required scaffolding
/// (package metadata, `cdylib` crate type, `ironpad-cell` path dependency).
/// When `shared_cargo_toml` is provided, its dependencies are merged first,
/// then cell-level deps override any shared dep with the same crate name.
///
/// Extra sections (e.g. `[profile.release]`) from the shared Cargo.toml are
/// also forwarded into the generated Cargo.toml, giving users control over
/// compilation profiles.
fn generate_cargo_toml(
    cell_id: &str,
    user_cargo_toml: &str,
    ironpad_cell_path: &Path,
    shared_cargo_toml: Option<&str>,
) -> String {
    let merged_deps = merge_dependencies(shared_cargo_toml, user_cargo_toml);
    let extra_sections = extract_extra_sections(shared_cargo_toml, user_cargo_toml);

    // Escape backslashes for Windows path compatibility in TOML strings.
    let cell_path_str = ironpad_cell_path.display().to_string().replace('\\', "/");

    let mut toml = format!(
        r#"[package]
name = "cell-{cell_id}"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[workspace]

[dependencies]
ironpad-cell = {{ path = "{cell_path_str}" }}
wasm-bindgen = "0.2"
"#
    );

    if !merged_deps.is_empty() {
        toml.push_str(&merged_deps);
        if !merged_deps.ends_with('\n') {
            toml.push('\n');
        }
    }

    if !extra_sections.is_empty() {
        toml.push('\n');
        toml.push_str(&extra_sections);
        if !extra_sections.ends_with('\n') {
            toml.push('\n');
        }
    }

    toml
}

/// Extract dependency lines from the user's `Cargo.toml` content.
///
/// Finds the `[dependencies]` section and collects all lines until the next
/// section header (`[...]`), filtering out any existing `ironpad-cell` entry
/// (we always inject our own).
fn extract_user_dependencies(cargo_toml: &str) -> String {
    let mut in_deps = false;
    let mut deps = Vec::new();

    for line in cargo_toml.lines() {
        let trimmed = line.trim();

        // Detect section headers.
        if trimmed.starts_with('[') {
            in_deps = trimmed == "[dependencies]";
            continue;
        }

        if in_deps && !trimmed.is_empty() && !trimmed.starts_with('#') {
            // Skip any user-specified ironpad-cell (we inject our own).
            if trimmed.starts_with("ironpad-cell") || trimmed.starts_with("ironpad_cell") {
                continue;
            }
            deps.push(line);
        }
    }

    deps.join("\n")
}

/// Merge shared (notebook-level) and cell-level dependencies.
///
/// Cell deps take precedence: if both shared and cell declare the same crate
/// name, the cell's line wins. The merge is at the dependency-line level.
fn merge_dependencies(shared_cargo_toml: Option<&str>, cell_cargo_toml: &str) -> String {
    let shared_deps = shared_cargo_toml.map_or_else(String::new, extract_user_dependencies);
    let cell_deps = extract_user_dependencies(cell_cargo_toml);

    if shared_deps.is_empty() {
        return cell_deps;
    }
    if cell_deps.is_empty() {
        return shared_deps;
    }

    // Build a map of crate_name → dep_line, shared first, then cell overrides.
    let mut dep_map: Vec<(String, String)> = Vec::new();

    for line in shared_deps.lines() {
        if let Some(name) = crate_name_from_dep_line(line) {
            dep_map.push((name, line.to_string()));
        }
    }

    for line in cell_deps.lines() {
        if let Some(name) = crate_name_from_dep_line(line) {
            if let Some(entry) = dep_map.iter_mut().find(|(n, _)| *n == name) {
                entry.1 = line.to_string();
            } else {
                dep_map.push((name, line.to_string()));
            }
        }
    }

    dep_map
        .into_iter()
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract non-`[dependencies]` sections from the shared and cell Cargo.toml
/// content (e.g. `[profile.release]`, `[features]`).
///
/// The shared sections are emitted first; if the cell Cargo.toml contains a
/// section with the same header, the cell's version replaces the shared one.
fn extract_extra_sections(shared_cargo_toml: Option<&str>, cell_cargo_toml: &str) -> String {
    let shared = shared_cargo_toml.map_or_else(Vec::new, collect_extra_sections);
    let cell = collect_extra_sections(cell_cargo_toml);

    if shared.is_empty() && cell.is_empty() {
        return String::new();
    }

    // Merge: shared first, cell overrides by section header.
    let mut sections: Vec<(String, String)> = shared;

    for (header, body) in cell {
        if let Some(entry) = sections.iter_mut().find(|(h, _)| *h == header) {
            entry.1 = body;
        } else {
            sections.push((header, body));
        }
    }

    sections
        .into_iter()
        .map(|(header, body)| format!("{header}\n{body}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Collect all non-`[dependencies]` sections from a Cargo.toml string.
///
/// Returns a list of `(header, body)` pairs where `header` is the full section
/// line (e.g. `[profile.release]`) and `body` is the content below it.
fn collect_extra_sections(cargo_toml: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_header: Option<String> = None;
    let mut current_body = Vec::new();

    let ignored_sections = ["[dependencies]", "[package]", "[lib]", "[workspace]"];

    for line in cargo_toml.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') {
            // Flush previous section if it was an extra.
            if let Some(header) = current_header.take() {
                let body = current_body.join("\n");
                if !body.trim().is_empty() {
                    sections.push((header, body));
                }
            }
            current_body.clear();

            let is_ignored = ignored_sections.contains(&trimmed);
            if !is_ignored {
                current_header = Some(trimmed.to_string());
            }
            continue;
        }

        if current_header.is_some() {
            current_body.push(line.to_string());
        }
    }

    // Flush final section.
    if let Some(header) = current_header {
        let body = current_body.join("\n");
        if !body.trim().is_empty() {
            sections.push((header, body));
        }
    }

    sections
}

/// Extract the crate name from a TOML dependency line.
///
/// Handles both `crate = "version"` and `crate = { ... }` forms.
/// Normalizes hyphens to underscores for comparison.
fn crate_name_from_dep_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let name = trimmed.split('=').next()?.trim();
    if name.is_empty() {
        return None;
    }
    Some(name.replace('-', "_"))
}

// ── lib.rs Generation ────────────────────────────────────────────────────────

/// Simple heuristic: if user source contains `.await` it needs an async wrapper.
fn needs_async(source: &str) -> bool {
    source.contains(".await")
}

/// Wrap user source code in the `cell_main` wasm-bindgen entry point.
///
/// Produces a `lib.rs` that:
/// 1. Imports the `ironpad_cell` prelude (which re-exports `wasm_bindgen`).
/// 2. Defines the `#[wasm_bindgen]` `cell_main` entry point.
/// 3. Optionally deserializes previous cell outputs into typed local variables.
/// 4. Assigns the user's code (as a block expression) to a `CellOutput` binding
///    via `.into()`, so any `From<T> for CellOutput` type works.
/// 5. Converts the `CellOutput` into a heap-allocated `CellResult` and returns
///    a raw pointer as `u32` for wasm-bindgen compatibility.
///
/// If the source contains `.await`, generates an async wrapper that wraps the
/// user code in an `async` block.
///
/// Returns `(generated_code, preamble_lines, is_async)` where `preamble_lines`
/// is the number of lines before user code begins (for diagnostic line mapping)
/// and `is_async` indicates whether the cell wrapper is async.
pub fn generate_lib_rs(
    source: &str,
    previous_cell_types: &[Option<String>],
) -> (String, u32, bool) {
    let is_async = needs_async(source);
    let has_any_prev = !previous_cell_types.is_empty();
    let typed_cells: Vec<(usize, &str)> = previous_cell_types
        .iter()
        .enumerate()
        .filter_map(|(i, opt)| opt.as_deref().map(|tag| (i, tag)))
        .collect();
    let typed_count = typed_cells.len() as u32;

    let async_kw = if is_async { "async " } else { "" };
    let (ptr_param, len_param) = if has_any_prev {
        ("input_ptr", "input_len")
    } else {
        ("_input_ptr", "_input_len")
    };

    let mut code = format!(
        "\
use ironpad_cell::prelude::*;

#[wasm_bindgen]
pub {async_kw}fn cell_main({ptr_param}: u32, {len_param}: u32) -> u32 {{\n",
    );

    if has_any_prev {
        code.push_str("let input_ptr = input_ptr as *const u8;\n");
        code.push_str("let input_len = input_len as usize;\n");
        code.push_str("let __ironpad_inputs__ = CellInputs::from_raw(unsafe { std::slice::from_raw_parts(input_ptr, input_len) });\n");

        for &(i, tag) in &typed_cells {
            code.push_str(&format!(
                "let cell{i}: {tag} = __ironpad_inputs__.get({i}).deserialize().expect(\"failed to deserialize cell{i}\");\n"
            ));
        }

        // `last` references the last cell with a type tag.
        if let Some(&(last_idx, _)) = typed_cells.last() {
            code.push_str(&format!("let last = &cell{last_idx};\n"));
        }
    }

    if is_async {
        code.push_str(&format!(
            "\
let __ironpad_output__: CellOutput = (async {{
{source}
}}).await.into();
let result: CellResult = __ironpad_output__.into();
Box::into_raw(Box::new(result)) as u32
}}
"
        ));
    } else {
        code.push_str(&format!(
            "\
let __ironpad_output__: CellOutput = ({{
{source}
}}).into();
let result: CellResult = __ironpad_output__.into();
Box::into_raw(Box::new(result)) as u32
}}
"
        ));
    }

    // Preamble: 5 base + optional (2 ptr reconstruction + 1 inputs) + cell decls + last.
    let preamble_lines =
        5 + if has_any_prev { 3 } else { 0 } + typed_count + if typed_count > 0 { 1 } else { 0 };

    (code, preamble_lines, is_async)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── extract_user_dependencies ────────────────────────────────────────

    #[test]
    fn extracts_deps_from_full_cargo_toml() {
        let user_toml = r#"
[package]
name = "my-cell"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
rand = "0.8"

[dev-dependencies]
criterion = "0.5"
"#;

        let deps = extract_user_dependencies(user_toml);
        assert!(deps.contains("serde"));
        assert!(deps.contains("rand"));
        // dev-dependencies should not be included.
        assert!(!deps.contains("criterion"));
    }

    #[test]
    fn filters_out_ironpad_cell_dep() {
        let user_toml = r#"
[dependencies]
ironpad-cell = { path = "../ironpad-cell" }
serde = "1"
"#;
        let deps = extract_user_dependencies(user_toml);
        assert!(!deps.contains("ironpad-cell"));
        assert!(deps.contains("serde"));
    }

    #[test]
    fn filters_out_ironpad_cell_underscore_dep() {
        let user_toml = r#"
[dependencies]
ironpad_cell = { path = "../ironpad-cell" }
serde = "1"
"#;
        let deps = extract_user_dependencies(user_toml);
        assert!(!deps.contains("ironpad_cell"));
        assert!(deps.contains("serde"));
    }

    #[test]
    fn handles_empty_cargo_toml() {
        let deps = extract_user_dependencies("");
        assert!(deps.is_empty());
    }

    #[test]
    fn handles_deps_only_cargo_toml() {
        let user_toml = r#"
[dependencies]
serde = "1"
"#;
        let deps = extract_user_dependencies(user_toml);
        assert!(deps.contains("serde"));
    }

    #[test]
    fn skips_comments_in_deps() {
        let user_toml = r#"
[dependencies]
# This is a comment
serde = "1"
"#;
        let deps = extract_user_dependencies(user_toml);
        assert!(!deps.contains("comment"));
        assert!(deps.contains("serde"));
    }

    // ── generate_cargo_toml ─────────────────────────────────────────────

    #[test]
    fn generates_valid_cargo_toml() {
        let cell_path = PathBuf::from("/opt/ironpad/crates/ironpad-cell");
        let user_toml = r#"
[dependencies]
serde = { version = "1", features = ["derive"] }
"#;

        let result = generate_cargo_toml("abc123", user_toml, &cell_path, None);

        assert!(result.contains(r#"name = "cell-abc123""#));
        assert!(result.contains(r#"crate-type = ["cdylib"]"#));
        assert!(result.contains("ironpad-cell = { path ="));
        assert!(result.contains("serde"));
    }

    #[test]
    fn cargo_toml_with_no_user_deps() {
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let result = generate_cargo_toml("cell0", "", &cell_path, None);

        assert!(result.contains(r#"name = "cell-cell0""#));
        assert!(result.contains(r#"crate-type = ["cdylib"]"#));
        assert!(result.contains("ironpad-cell"));
    }

    // ── generate_lib_rs ─────────────────────────────────────────────────

    #[test]
    fn wraps_user_code_in_cell_main() {
        let source = r#"    CellOutput::text("hello")"#;

        let (lib_rs, _, is_async) = generate_lib_rs(source, &[]);

        assert!(!is_async);
        assert!(lib_rs.contains("use ironpad_cell::prelude::*;"));
        assert!(lib_rs.contains("#[wasm_bindgen]"));
        assert!(lib_rs.contains("pub fn cell_main("));
        assert!(lib_rs.contains("-> u32 {"));
        assert!(lib_rs.contains("CellOutput::text(\"hello\")"));
        assert!(lib_rs.contains("let __ironpad_output__: CellOutput = ({"));
        assert!(lib_rs.contains("}).into();"));
        assert!(lib_rs.contains("let result: CellResult = __ironpad_output__.into();"));
        assert!(lib_rs.contains("Box::into_raw(Box::new(result)) as u32"));
    }

    #[test]
    fn generate_lib_rs_no_previous_cells() {
        let (lib_rs, preamble, is_async) = generate_lib_rs("    // user code here", &[]);

        assert_eq!(preamble, 5);
        assert!(!is_async);
        assert!(!lib_rs.contains("__ironpad_inputs__"));
        assert!(!lib_rs.contains("let cell"));
        assert!(!lib_rs.contains("let last"));
        assert!(lib_rs.contains("_input_ptr: u32"));
        assert!(lib_rs.contains("_input_len: u32"));

        // Verify user code starts at expected line.
        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize].trim(), "// user code here");
    }

    #[test]
    fn generate_lib_rs_with_typed_cells() {
        let types: Vec<Option<String>> = vec![Some("u32".into()), Some("String".into())];
        let (lib_rs, preamble, _) = generate_lib_rs("    // user code here", &types);

        // 5 base + 3 (ptr reconstruction + inputs) + 2 typed + 1 last = 11
        assert_eq!(preamble, 11);
        assert!(lib_rs.contains("let input_ptr = input_ptr as *const u8;"));
        assert!(lib_rs.contains("let input_len = input_len as usize;"));
        assert!(lib_rs.contains("let __ironpad_inputs__ = CellInputs::from_raw("));
        assert!(lib_rs.contains("let cell0: u32 = __ironpad_inputs__.get(0).deserialize()"));
        assert!(lib_rs.contains("let cell1: String = __ironpad_inputs__.get(1).deserialize()"));
        assert!(lib_rs.contains("let last = &cell1;"));

        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize].trim(), "// user code here");
    }

    #[test]
    fn generate_lib_rs_with_mixed_types() {
        let types: Vec<Option<String>> = vec![Some("u32".into()), None, Some("bool".into())];
        let (lib_rs, preamble, _) = generate_lib_rs("    // user code here", &types);

        // 5 base + 3 (ptr reconstruction + inputs) + 2 typed + 1 last = 11
        assert_eq!(preamble, 11);
        assert!(lib_rs.contains("let cell0: u32 = __ironpad_inputs__.get(0).deserialize()"));
        assert!(!lib_rs.contains("let cell1:"));
        assert!(lib_rs.contains("let cell2: bool = __ironpad_inputs__.get(2).deserialize()"));
        assert!(lib_rs.contains("let last = &cell2;"));

        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize].trim(), "// user code here");
    }

    #[test]
    fn generate_lib_rs_all_none_types() {
        let types: Vec<Option<String>> = vec![None, None];
        let (lib_rs, preamble, _) = generate_lib_rs("    // user code here", &types);

        // 5 base + 3 (ptr reconstruction + inputs) + 0 typed + 0 last = 8
        assert_eq!(preamble, 8);
        assert!(lib_rs.contains("let __ironpad_inputs__ = CellInputs::from_raw("));
        assert!(!lib_rs.contains("let cell0"));
        assert!(!lib_rs.contains("let cell1"));
        assert!(!lib_rs.contains("let last"));

        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize].trim(), "// user code here");
    }

    // ── scaffold_micro_crate (integration) ──────────────────────────────

    #[test]
    fn scaffolds_complete_micro_crate() {
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let user_source = "    CellOutput::empty()";
        let user_cargo = r#"
[dependencies]
serde = "1"
"#;

        let (crate_dir, preamble_lines, is_async) = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "session-1",
            "cell-0",
            user_source,
            user_cargo,
            &[],
            None,
        )
        .expect("scaffold should succeed");

        assert_eq!(preamble_lines, 5);
        assert!(!is_async);

        // Verify directory structure.
        assert!(crate_dir.join("Cargo.toml").is_file());
        assert!(crate_dir.join("src/lib.rs").is_file());

        // Verify Cargo.toml content.
        let cargo_content = std::fs::read_to_string(crate_dir.join("Cargo.toml")).unwrap();
        assert!(cargo_content.contains(r#"name = "cell-cell-0""#));
        assert!(cargo_content.contains(r#"crate-type = ["cdylib"]"#));
        assert!(cargo_content.contains("ironpad-cell"));
        assert!(cargo_content.contains("serde"));

        // Verify lib.rs content.
        let lib_content = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        assert!(lib_content.contains("use ironpad_cell::prelude::*;"));
        assert!(lib_content.contains("cell_main"));
        assert!(lib_content.contains("CellOutput::empty()"));
    }

    #[test]
    fn overwrites_existing_scaffold() {
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");

        // First scaffold.
        scaffold_micro_crate(&tmp, &cell_path, "s1", "c1", "    // v1", "", &[], None)
            .expect("first scaffold");

        // Second scaffold with different source.
        scaffold_micro_crate(&tmp, &cell_path, "s1", "c1", "    // v2", "", &[], None)
            .expect("second scaffold");

        let lib_content = std::fs::read_to_string(tmp.join("workspaces/s1/c1/src/lib.rs")).unwrap();
        assert!(lib_content.contains("// v2"));
        assert!(!lib_content.contains("// v1"));
    }

    // ── Test Helpers ────────────────────────────────────────────────────

    /// Create a temporary directory that is cleaned up on drop.
    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ironpad-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ── merge_dependencies / shared Cargo.toml ──────────────────────────

    #[test]
    fn merge_no_shared_returns_cell_deps() {
        let cell_toml = "[dependencies]\nrand = \"0.8\"";
        let result = merge_dependencies(None, cell_toml);
        assert!(result.contains("rand"));
    }

    #[test]
    fn merge_shared_only() {
        let shared = "[dependencies]\nserde = \"1\"";
        let result = merge_dependencies(Some(shared), "");
        assert!(result.contains("serde"));
    }

    #[test]
    fn merge_cell_overrides_shared() {
        let shared = "[dependencies]\nserde = \"1.0.0\"";
        let cell = "[dependencies]\nserde = { version = \"1.0.200\", features = [\"derive\"] }";
        let result = merge_dependencies(Some(shared), cell);
        // Cell version should win.
        assert!(result.contains("1.0.200"));
        assert!(!result.contains("1.0.0"));
    }

    #[test]
    fn merge_shared_and_cell_different_crates() {
        let shared = "[dependencies]\nserde = \"1\"";
        let cell = "[dependencies]\nrand = \"0.8\"";
        let result = merge_dependencies(Some(shared), cell);
        assert!(result.contains("serde"));
        assert!(result.contains("rand"));
    }

    #[test]
    fn merge_normalizes_hyphens_for_override() {
        let shared = "[dependencies]\nmy-crate = \"1.0\"";
        let cell = "[dependencies]\nmy_crate = \"2.0\"";
        let result = merge_dependencies(Some(shared), cell);
        // Cell's version should override shared (same crate, different spelling).
        assert!(result.contains("2.0"));
        // Only one entry for the crate.
        let count = result.lines().filter(|l| l.contains("my")).count();
        assert_eq!(count, 1);
    }

    #[test]
    fn merge_both_empty() {
        let result = merge_dependencies(Some(""), "");
        assert!(result.is_empty());
    }

    #[test]
    fn scaffold_with_shared_cargo_toml() {
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let shared = "[dependencies]\nserde = \"1\"";
        let cell = "[dependencies]\nrand = \"0.8\"";

        let (crate_dir, ..) = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "s1",
            "c1",
            "    CellOutput::empty()",
            cell,
            &[],
            Some(shared),
        )
        .unwrap();

        let cargo = std::fs::read_to_string(crate_dir.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("serde"), "shared dep should be present");
        assert!(cargo.contains("rand"), "cell dep should be present");
    }

    #[test]
    fn crate_name_extraction() {
        assert_eq!(
            crate_name_from_dep_line("serde = \"1\""),
            Some("serde".into())
        );
        assert_eq!(
            crate_name_from_dep_line("my-crate = { version = \"1\" }"),
            Some("my_crate".into())
        );
        assert_eq!(crate_name_from_dep_line(""), None);
    }

    // ── needs_async ─────────────────────────────────────────────────────

    #[test]
    fn needs_async_detects_await() {
        assert!(needs_async("    let x = foo().await;"));
        assert!(needs_async("    foo.await.bar()"));
    }

    #[test]
    fn needs_async_false_for_sync_code() {
        assert!(!needs_async("    let x = 42;"));
        assert!(!needs_async("    CellOutput::empty()"));
    }

    // ── async wrapper generation ────────────────────────────────────────

    #[test]
    fn generate_lib_rs_async_no_previous_cells() {
        let (lib_rs, preamble, is_async) = generate_lib_rs("    something.await", &[]);

        assert!(is_async);
        assert_eq!(preamble, 5);
        assert!(lib_rs.contains("#[wasm_bindgen]"));
        assert!(lib_rs.contains("pub async fn cell_main("));
        assert!(lib_rs.contains("(async {"));
        assert!(lib_rs.contains("}).await.into();"));
        assert!(lib_rs.contains("Box::into_raw(Box::new(result)) as u32"));

        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize].trim(), "something.await");
    }

    #[test]
    fn generate_lib_rs_async_with_typed_cells() {
        let types: Vec<Option<String>> = vec![Some("u32".into())];
        let (lib_rs, preamble, is_async) = generate_lib_rs("    cell0.await", &types);

        assert!(is_async);
        // 5 base + 3 (ptr reconstruction + inputs) + 1 typed + 1 last = 10
        assert_eq!(preamble, 10);
        assert!(lib_rs.contains("pub async fn cell_main(input_ptr: u32, input_len: u32)"));
        assert!(lib_rs.contains("let cell0: u32"));
        assert!(lib_rs.contains("(async {"));

        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize].trim(), "cell0.await");
    }

    #[test]
    fn scaffold_micro_crate_returns_is_async() {
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");

        let (_, _, is_async) = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "s1",
            "c1",
            "    CellOutput::empty()",
            "",
            &[],
            None,
        )
        .unwrap();
        assert!(!is_async);

        let (_, _, is_async) = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "s1",
            "c2",
            "    foo().await",
            "",
            &[],
            None,
        )
        .unwrap();
        assert!(is_async);
    }

    // ── extract_extra_sections ──────────────────────────────────────────

    #[test]
    fn extra_sections_from_shared_profile() {
        let shared = "\
[dependencies]
serde = \"1\"

[profile.release]
opt-level = 1
lto = false
";
        let result = extract_extra_sections(Some(shared), "");
        assert!(result.contains("[profile.release]"));
        assert!(result.contains("opt-level = 1"));
        assert!(result.contains("lto = false"));
        // Dependencies should NOT appear in extra sections.
        assert!(!result.contains("serde"));
    }

    #[test]
    fn extra_sections_cell_overrides_shared() {
        let shared = "\
[profile.release]
opt-level = 1
";
        let cell = "\
[profile.release]
opt-level = 3
";
        let result = extract_extra_sections(Some(shared), cell);
        assert!(result.contains("opt-level = 3"));
        assert!(!result.contains("opt-level = 1"));
    }

    #[test]
    fn extra_sections_ignores_known_headers() {
        let shared = "\
[package]
name = \"should-be-ignored\"

[dependencies]
serde = \"1\"

[lib]
crate-type = [\"cdylib\"]

[workspace]
";
        let result = extract_extra_sections(Some(shared), "");
        assert!(result.is_empty());
    }

    #[test]
    fn extra_sections_multiple_sections() {
        let shared = "\
[profile.release]
opt-level = 1

[features]
my-feature = []
";
        let result = extract_extra_sections(Some(shared), "");
        assert!(result.contains("[profile.release]"));
        assert!(result.contains("opt-level = 1"));
        assert!(result.contains("[features]"));
        assert!(result.contains("my-feature"));
    }

    #[test]
    fn extra_sections_empty_when_none() {
        let result = extract_extra_sections(None, "");
        assert!(result.is_empty());
    }

    #[test]
    fn generate_cargo_toml_includes_profile() {
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let shared = "\
[dependencies]
serde = \"1\"

[profile.release]
opt-level = 1
lto = false
codegen-units = 16
";
        let result = generate_cargo_toml("abc", "", &cell_path, Some(shared));
        assert!(result.contains("[profile.release]"));
        assert!(result.contains("opt-level = 1"));
        assert!(result.contains("serde"));
        assert!(result.contains("ironpad-cell"));
    }

    // ── T-005: Additional edge-case tests ───────────────────────────────

    #[test]
    fn generate_lib_rs_with_unicode_source() {
        let source = "    let msg = \"こんにちは世界 🦀\";\n    CellOutput::text(msg)";
        let (lib_rs, preamble, _) = generate_lib_rs(source, &[]);

        assert_eq!(preamble, 5);
        assert!(lib_rs.contains("こんにちは世界 🦀"));
        // Verify user code appears at the correct preamble offset.
        let lines: Vec<&str> = lib_rs.lines().collect();
        assert!(lines[preamble as usize].contains("こんにちは世界"));
    }

    #[test]
    fn merge_shared_ironpad_cell_dep_is_filtered() {
        // Shared Cargo.toml that redundantly declares ironpad-cell should have
        // that dep stripped (the scaffold always injects its own).
        let shared = "[dependencies]\nironpad-cell = \"0.1\"\nserde = \"1\"";
        let cell = "[dependencies]\nrand = \"0.8\"";
        let result = merge_dependencies(Some(shared), cell);
        assert!(
            !result.contains("ironpad-cell"),
            "ironpad-cell should be filtered from shared"
        );
        assert!(result.contains("serde"));
        assert!(result.contains("rand"));
    }

    #[test]
    fn extra_sections_patch_section_forwarded() {
        let shared = "\
[dependencies]
serde = \"1\"

[patch.crates-io]
serde = { git = \"https://github.com/serde-rs/serde\" }
";
        let result = extract_extra_sections(Some(shared), "");
        assert!(result.contains("[patch.crates-io]"));
        assert!(result.contains("serde = { git ="));
    }

    #[test]
    fn collect_extra_sections_skips_empty_body() {
        // A section header followed immediately by another header should
        // produce no extra section (empty body is filtered).
        let toml = "\
[profile.release]

[dependencies]
serde = \"1\"
";
        let sections = collect_extra_sections(toml);
        assert!(
            sections.is_empty(),
            "section with whitespace-only body should be skipped"
        );
    }

    #[test]
    fn crate_name_from_dep_line_whitespace_only() {
        assert_eq!(crate_name_from_dep_line("   "), None);
    }

    #[test]
    fn generate_lib_rs_empty_source() {
        let (lib_rs, preamble, is_async) = generate_lib_rs("", &[]);

        assert_eq!(preamble, 5);
        assert!(!is_async);
        // The empty source should still produce a compilable wrapper.
        assert!(lib_rs.contains("let __ironpad_output__: CellOutput = ({"));
        assert!(lib_rs.contains("}).into();"));
    }
}
