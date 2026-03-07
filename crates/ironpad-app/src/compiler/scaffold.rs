//! Micro-crate scaffolding for cell compilation.
//!
//! Given a cell's source code and Cargo.toml, this module writes a complete
//! micro-crate to disk that can be compiled to WASM with `cargo build`.

use std::path::{Path, PathBuf};

/// Number of lines in the auto-generated wrapper *before* user code begins.
///
/// Used by the diagnostic parser (T-011) to map compiler spans back to user
/// source line numbers.
pub const WRAPPER_PREAMBLE_LINES: u32 = 4;

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
/// Returns the path to the micro-crate root directory.
pub fn scaffold_micro_crate(
    cache_dir: &Path,
    ironpad_cell_path: &Path,
    session_id: &str,
    cell_id: &str,
    source: &str,
    cargo_toml: &str,
) -> anyhow::Result<PathBuf> {
    let crate_dir = cache_dir.join("workspaces").join(session_id).join(cell_id);

    let src_dir = crate_dir.join("src");
    std::fs::create_dir_all(&src_dir)?;

    // Resolve ironpad-cell path to absolute so the micro-crate can find it
    // regardless of the working directory used by `cargo build`.
    let absolute_cell_path = std::fs::canonicalize(ironpad_cell_path).unwrap_or_else(|_| {
        // Fall back to the raw path if canonicalize fails (e.g. path doesn't exist yet).
        ironpad_cell_path.to_path_buf()
    });

    let generated_cargo_toml = generate_cargo_toml(cell_id, cargo_toml, &absolute_cell_path);
    std::fs::write(crate_dir.join("Cargo.toml"), generated_cargo_toml)?;

    let lib_rs = generate_lib_rs(source);
    std::fs::write(src_dir.join("lib.rs"), lib_rs)?;

    Ok(crate_dir)
}

// ── Cargo.toml Generation ────────────────────────────────────────────────────

/// Build a complete `Cargo.toml` for the micro-crate.
///
/// Merges the user-provided dependency lines with the required scaffolding
/// (package metadata, `cdylib` crate type, `ironpad-cell` path dependency).
fn generate_cargo_toml(cell_id: &str, user_cargo_toml: &str, ironpad_cell_path: &Path) -> String {
    let user_deps = extract_user_dependencies(user_cargo_toml);

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
"#
    );

    if !user_deps.is_empty() {
        toml.push_str(&user_deps);
        if !user_deps.ends_with('\n') {
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

// ── lib.rs Generation ────────────────────────────────────────────────────────

/// Wrap user source code in the `cell_main` FFI function.
///
/// Produces a `lib.rs` that:
/// 1. Imports the `ironpad_cell` prelude.
/// 2. Defines the `#[no_mangle]` `cell_main` entry point.
/// 3. Embeds the user's code verbatim inside the function body.
fn generate_lib_rs(source: &str) -> String {
    format!(
        "\
use ironpad_cell::prelude::*;

#[no_mangle]
pub extern \"C\" fn cell_main(input_ptr: *const u8, input_len: usize) -> CellResult {{
{source}
}}
"
    )
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

        let result = generate_cargo_toml("abc123", user_toml, &cell_path);

        assert!(result.contains(r#"name = "cell-abc123""#));
        assert!(result.contains(r#"crate-type = ["cdylib"]"#));
        assert!(result.contains("ironpad-cell = { path ="));
        assert!(result.contains("serde"));
    }

    #[test]
    fn cargo_toml_with_no_user_deps() {
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let result = generate_cargo_toml("cell0", "", &cell_path);

        assert!(result.contains(r#"name = "cell-cell0""#));
        assert!(result.contains(r#"crate-type = ["cdylib"]"#));
        assert!(result.contains("ironpad-cell"));
    }

    // ── generate_lib_rs ─────────────────────────────────────────────────

    #[test]
    fn wraps_user_code_in_cell_main() {
        let source = r#"    let input = CellInput::new(unsafe { std::slice::from_raw_parts(input_ptr, input_len) });
    CellOutput::text("hello").into()"#;

        let lib_rs = generate_lib_rs(source);

        assert!(lib_rs.contains("use ironpad_cell::prelude::*;"));
        assert!(lib_rs.contains("pub extern \"C\" fn cell_main("));
        assert!(lib_rs.contains("CellOutput::text(\"hello\")"));
        assert!(lib_rs.contains("-> CellResult {"));
    }

    #[test]
    fn wrapper_preamble_line_count_is_correct() {
        let lib_rs = generate_lib_rs("    // user code here");
        let lines: Vec<&str> = lib_rs.lines().collect();

        // The user code should appear at line index WRAPPER_PREAMBLE_LINES.
        assert_eq!(
            lines[WRAPPER_PREAMBLE_LINES as usize].trim(),
            "// user code here",
            "user code should start at line {}",
            WRAPPER_PREAMBLE_LINES + 1
        );
    }

    // ── scaffold_micro_crate (integration) ──────────────────────────────

    #[test]
    fn scaffolds_complete_micro_crate() {
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let user_source = "    CellOutput::empty().into()";
        let user_cargo = r#"
[dependencies]
serde = "1"
"#;

        let crate_dir = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "session-1",
            "cell-0",
            user_source,
            user_cargo,
        )
        .expect("scaffold should succeed");

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
        assert!(lib_content.contains("CellOutput::empty().into()"));
    }

    #[test]
    fn overwrites_existing_scaffold() {
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");

        // First scaffold.
        scaffold_micro_crate(&tmp, &cell_path, "s1", "c1", "    // v1", "")
            .expect("first scaffold");

        // Second scaffold with different source.
        scaffold_micro_crate(&tmp, &cell_path, "s1", "c1", "    // v2", "")
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
}
