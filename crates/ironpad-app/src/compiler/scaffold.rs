//! Micro-crate scaffolding for cell compilation.
//!
//! Given a cell's source code and Cargo.toml, this module writes a complete
//! micro-crate to disk that can be compiled to WASM with `cargo build`.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use ironpad_common::cache_key::merge_dependencies;

// Re-exported so in-crate callers (`server_fns.rs`, `compiler/mod.rs`) and the
// tests below keep their `compiler::scaffold::*` paths; the definitions moved
// to `ironpad-common/src/cache_key.rs` (PRD-0047) so the browser computes
// cache keys from the same recipe.
pub use ironpad_common::cache_key::{
    crate_name_from_dep_line, extract_user_dependencies, merged_deps_contain_rayon,
    uses_coroutines, uses_std_autodiff, uses_wasm_simd,
};

// ── Public API ───────────────────────────────────────────────────────────────

/// Whether a cell id is safe to use in filesystem paths and the generated
/// `Cargo.toml`. Restricting to `[A-Za-z0-9_-]` (1–64 chars) prevents path
/// traversal (`../` escaping the workspace) and manifest injection (a `"` or
/// newline in `name = "cell-{id}"` would inject arbitrary `[package]` keys).
/// Callers must validate before passing an id to [`scaffold_micro_crate`].
pub fn is_valid_cell_id(cell_id: &str) -> bool {
    (1..=64).contains(&cell_id.len())
        && cell_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

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
/// Returns `(crate_dir, preamble_lines, is_async, is_simulation)`. Feature
/// flags (atomics/autodiff/simd) are re-derived here from the same pure
/// detection functions callers hash with — those functions are the single
/// source of truth, so both sides always agree:
/// - `crate_dir`: path to the micro-crate root directory
/// - `preamble_lines`: number of lines before user code (for diagnostic mapping)
/// - `is_async`: whether the cell wrapper is async (source contains `.await`)
/// - `is_simulation`: whether the cell uses the tick infrastructure (a
///   `Simulation` or `LiveView` trait impl)
#[allow(clippy::too_many_arguments)]
pub fn scaffold_micro_crate(
    cache_dir: &Path,
    ironpad_cell_path: &Path,
    session_id: &str,
    cell_id: &str,
    source: &str,
    cargo_toml: &str,
    previous_cell_types: &[String],
    shared_cargo_toml: Option<&str>,
    shared_source: Option<&str>,
) -> anyhow::Result<(PathBuf, u32, bool, bool)> {
    let crate_dir = cache_dir.join("workspaces").join(session_id).join(cell_id);

    let src_dir = crate_dir.join("src");
    std::fs::create_dir_all(&src_dir)?;

    // Resolve ironpad-cell path to absolute so the micro-crate can find it
    // regardless of the working directory used by `cargo build`.
    let absolute_cell_path = std::fs::canonicalize(ironpad_cell_path).unwrap_or_else(|_| {
        // Fall back to the raw path if canonicalize fails (e.g. path doesn't exist yet).
        ironpad_cell_path.to_path_buf()
    });

    let needs_atomics = merged_deps_contain_rayon(shared_cargo_toml, cargo_toml);
    let needs_autodiff = uses_std_autodiff(source, shared_source);

    let generated_cargo_toml = generate_cargo_toml(
        cell_id,
        cargo_toml,
        &absolute_cell_path,
        shared_cargo_toml,
        needs_atomics,
        needs_autodiff,
    );
    std::fs::write(crate_dir.join("Cargo.toml"), generated_cargo_toml)?;

    let (mut lib_rs, mut preamble_lines, is_async, is_simulation) =
        generate_lib_rs(source, previous_cell_types, shared_source.is_some());
    if needs_autodiff {
        // `#![feature(autodiff)]` must sit at the crate root — the one place a
        // cell author can't reach and the scaffold owns. Adding a line above
        // the wrapper shifts every diagnostic, hence the preamble bump.
        lib_rs.insert_str(0, "#![feature(autodiff)]\n");
        preamble_lines += 1;
    }
    if uses_wasm_simd(source, shared_source) {
        // Crate-root gate for `std::simd` (portable SIMD is nightly-only);
        // harmless when the cell only uses stable `std::arch::wasm32`
        // intrinsics. Same preamble bump rule as the autodiff gate above.
        lib_rs.insert_str(0, "#![feature(portable_simd)]\n");
        preamble_lines += 1;
    }
    if uses_coroutines(source, shared_source) {
        // Crate-root gate for `#[coroutine]`. `stmt_expr_attributes` is
        // required because the attribute attaches to a closure *expression*.
        // Same preamble bump rule as the two gates above.
        lib_rs.insert_str(
            0,
            "#![feature(coroutines, coroutine_trait, stmt_expr_attributes)]\n",
        );
        preamble_lines += 1;
    }
    std::fs::write(src_dir.join("lib.rs"), lib_rs)?;

    if let Some(shared) = shared_source {
        std::fs::write(src_dir.join("shared.rs"), shared)?;
    }

    Ok((crate_dir, preamble_lines, is_async, is_simulation))
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
    needs_atomics: bool,
    needs_autodiff: bool,
) -> String {
    let merged_deps = merge_dependencies(shared_cargo_toml, user_cargo_toml);
    let mut extra_sections = extract_extra_sections(shared_cargo_toml, user_cargo_toml);
    if needs_autodiff {
        // Enzyme runs during (fat) LTO and rustc enforces the profile per
        // crate, so the notebook's usual fast-compile profile must be
        // overridden — not appended, which would be a TOML duplicate-key error.
        extra_sections = enforce_autodiff_profile(&extra_sections);
    }

    // Escape backslashes for Windows path compatibility in TOML strings.
    let cell_path_str = ironpad_cell_path.display().to_string().replace('\\', "/");

    let cell_dep = if needs_atomics {
        format!(r#"ironpad-cell = {{ path = "{cell_path_str}", features = ["rayon"] }}"#)
    } else {
        format!(r#"ironpad-cell = {{ path = "{cell_path_str}" }}"#)
    };

    let wasm_bindgen_dep = wasm_bindgen_dependency_line();

    let mut toml = format!(
        r#"[package]
name = "cell-{cell_id}"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[workspace]

[dependencies]
{cell_dep}
{wasm_bindgen_dep}
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

// (`extract_user_dependencies`, `merge_dependencies`, and the feature
// detectors moved to `ironpad_common::cache_key` — see the re-exports at the
// top of this module.)

/// Rewrite `[profile.release]` in the forwarded extra sections so the Enzyme
/// requirements hold: `lto = "fat"` and `codegen-units = 1` (rustc refuses
/// `#[autodiff_*]` without fat LTO). The user's other profile keys (e.g.
/// `opt-level`) are preserved; if no release profile exists, one is appended.
fn enforce_autodiff_profile(extra_sections: &str) -> String {
    const FORCED: &str = "lto = \"fat\"\ncodegen-units = 1\n";

    let mut out = String::new();
    let mut in_release = false;
    let mut had_release = false;

    for line in extra_sections.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_release = trimmed == "[profile.release]";
            if in_release {
                had_release = true;
                out.push_str(line);
                out.push('\n');
                out.push_str(FORCED);
                continue;
            }
        }
        if in_release {
            let key = trimmed.split('=').next().map_or("", str::trim);
            if key == "lto" || key == "codegen-units" {
                continue; // replaced by the forced values above
            }
        }
        out.push_str(line);
        out.push('\n');
    }

    if !had_release {
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str("[profile.release]\n");
        out.push_str(FORCED);
    }

    out
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

/// Build the `wasm-bindgen` dependency line for the scaffolded Cargo.toml.
///
/// Pins to the exact host `wasm-bindgen` CLI version (`=X.Y.Z`) so the
/// resolved crate matches the CLI that `compiler/build.rs` shells out to for
/// post-processing — a version mismatch trips wasm-bindgen's exact-schema
/// check and breaks every cell. Falls back to the previous floating `"0.2"`
/// requirement (and logs a warning) if the CLI version can't be determined;
/// the build stage surfaces a clear error if the CLI is truly absent.
fn wasm_bindgen_dependency_line() -> String {
    if let Some(version) = super::toolchain::wasm_bindgen_cli_version() {
        return format!(r#"wasm-bindgen = "={version}""#);
    }

    tracing::warn!(
        "could not determine host wasm-bindgen CLI version; falling back to \
         floating \"0.2\" requirement, which may drift from the CLI used for \
         post-processing"
    );
    r#"wasm-bindgen = "0.2""#.to_string()
}

// ── lib.rs Generation ────────────────────────────────────────────────────────

/// Simple heuristic: if user source contains `.await` it needs an async wrapper.
fn needs_async(source: &str) -> bool {
    source.contains(".await")
}

/// Detect whether the source implements a given trait via `impl <Trait> for <Name>`.
///
/// Scans for `impl <trait_name> for <Name>` and returns the struct name if
/// found. Handles whitespace variations between tokens. Ignores commented-out
/// lines. Backs [`is_simulation`] and [`is_live_view`].
fn impl_target_struct(source: &str, trait_name: &str) -> Option<String> {
    for line in source.lines() {
        let trimmed = line.trim();
        // Skip comment lines.
        if trimmed.starts_with("//") || trimmed.starts_with("/*") {
            continue;
        }
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        for window in tokens.windows(4) {
            if window[0] == "impl" && window[1] == trait_name && window[2] == "for" {
                let name: String = window[3]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }
    None
}

/// Detect whether the source implements the `Simulation` trait, returning the
/// implementing struct's name if so.
fn is_simulation(source: &str) -> Option<String> {
    impl_target_struct(source, "Simulation")
}

/// Detect whether the source implements the `LiveView` trait, returning the
/// implementing struct's name if so.
fn is_live_view(source: &str) -> Option<String> {
    impl_target_struct(source, "LiveView")
}

/// The `mod shared;` declaration injected when a notebook has shared source.
///
/// `allow(dead_code)`: the shared module is the notebook's library, but rustc
/// only ever sees ONE cell's crate, so every helper this cell does not happen
/// to call would report "never used" — a false positive, since it IS used, by
/// another cell. Kept on one line so `preamble_lines` (and therefore
/// diagnostic mapping) counts it as exactly one line in every generator.
fn shared_mod_decl(has_shared_source: bool) -> &'static str {
    if has_shared_source {
        "#[allow(dead_code)] mod shared;\n"
    } else {
        ""
    }
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
/// Returns `(generated_code, preamble_lines, is_async, is_simulation)` where
/// `preamble_lines` is the number of lines before user code begins (for diagnostic
/// line mapping), `is_async` indicates whether the cell wrapper is async, and
/// `is_simulation` indicates whether a `Simulation` trait impl was detected.
pub fn generate_lib_rs(
    source: &str,
    previous_cell_types: &[String],
    has_shared_source: bool,
) -> (String, u32, bool, bool) {
    // Simulation cells get a completely different wrapper.
    if let Some(struct_name) = is_simulation(source) {
        return generate_simulation_lib_rs(source, &struct_name, has_shared_source);
    }

    // LiveView cells get a content-returning tick wrapper.
    if let Some(struct_name) = is_live_view(source) {
        return generate_live_view_lib_rs(source, &struct_name, has_shared_source);
    }

    let is_async = needs_async(source);
    let has_any_prev = !previous_cell_types.is_empty();
    let typed_cells: Vec<(usize, &str)> = previous_cell_types
        .iter()
        .enumerate()
        .filter(|(_, tag)| !tag.is_empty())
        .map(|(i, tag)| (i, tag.as_str()))
        .collect();
    // Cell count is always small (< 100), so truncation is not a concern.
    #[allow(clippy::cast_possible_truncation)]
    let typed_count = typed_cells.len() as u32;

    let async_kw = if is_async { "async " } else { "" };
    let (ptr_param, len_param) = if has_any_prev {
        ("input_ptr", "input_len")
    } else {
        ("_input_ptr", "_input_len")
    };

    let shared_mod = shared_mod_decl(has_shared_source);

    let mut code = format!(
        "\
use ironpad_cell::prelude::*;
{shared_mod}
#[wasm_bindgen]
pub {async_kw}fn cell_main({ptr_param}: u32, {len_param}: u32) -> u32 {{
console_error_panic_hook::set_once();\n",
    );

    if has_any_prev {
        code.push_str("let input_ptr = input_ptr as *const u8;\n");
        code.push_str("let input_len = input_len as usize;\n");
        code.push_str("let __ironpad_inputs__ = CellInputs::from_raw(unsafe { std::slice::from_raw_parts(input_ptr, input_len) });\n");

        // `allow(unused_variables)`: every typed upstream output is bound
        // whether or not this cell reads it, so unguarded bindings spray
        // "unused variable: `cellN`" on cells that only use some (or none) of
        // their inputs. The allow rides on each binding's line so
        // `preamble_lines` (and therefore diagnostic mapping) is unaffected,
        // and unused variables in the user's own code still warn.
        for &(i, tag) in &typed_cells {
            let _ = writeln!(
                code,
                "#[allow(unused_variables)] let cell{i}: {tag} = __ironpad_inputs__.get({i}).deserialize().unwrap_or_else(|e| panic!(\"failed to deserialize cell{i} as `{tag}` (input {{}} bytes): {{e}}\", __ironpad_inputs__.get({i}).raw().len()));"
            );
        }

        // `last` references the last cell with a type tag.
        if let Some(&(last_idx, _)) = typed_cells.last() {
            let _ = writeln!(
                code,
                "#[allow(unused_variables)] let last = &cell{last_idx};"
            );
        }
    }

    if is_async {
        let _ = write!(
            code,
            "\
let __ironpad_output__: CellOutput = (async {{
{source}
}}).await.into();
let result: CellResult = __ironpad_output__.into();
Box::into_raw(Box::new(result)) as u32
}}
"
        );
    } else {
        let _ = write!(
            code,
            "\
let __ironpad_output__: CellOutput = ({{
{source}
}}).into();
let result: CellResult = __ironpad_output__.into();
Box::into_raw(Box::new(result)) as u32
}}
"
        );
    }

    // Preamble: 6 base (includes panic hook) + 1 if shared_source + optional (2 ptr reconstruction + 1 inputs) + cell decls + last.
    let shared_lines = u32::from(has_shared_source);
    let preamble_lines = 6
        + shared_lines
        + if has_any_prev { 3 } else { 0 }
        + typed_count
        + u32::from(typed_count > 0);

    (code, preamble_lines, is_async, false)
}

/// Generate the `lib.rs` wrapper for simulation cells.
///
/// Simulation cells produce two WASM exports: `cell_main` (initialization) and
/// `cell_tick` (per-frame update). The user source (struct definition + trait
/// impl) appears at module level rather than inside a function body.
fn generate_simulation_lib_rs(
    source: &str,
    struct_name: &str,
    has_shared_source: bool,
) -> (String, u32, bool, bool) {
    let shared_mod = shared_mod_decl(has_shared_source);

    let code = format!(
        "\
use ironpad_cell::prelude::*;
{shared_mod}
{source}

static mut __IRONPAD_SIM__: Option<{struct_name}> = None;

#[wasm_bindgen]
pub fn cell_main(_input_ptr: u32, _input_len: u32) -> u32 {{
console_error_panic_hook::set_once();
let mut sim = {struct_name}::init();
let first_frame = sim.tick();
let meta = SimulationMeta {{
    width: first_frame.width(),
    height: first_frame.height(),
    fps: {struct_name}::fps(),
    first_frame,
    sliders: {struct_name}::sliders(),
}};
unsafe {{ __IRONPAD_SIM__ = Some(sim); }}
let __ironpad_output__: CellOutput = meta.into();
let result: CellResult = __ironpad_output__.into();
Box::into_raw(Box::new(result)) as u32
}}

#[wasm_bindgen]
pub fn cell_tick() -> u32 {{
// Raw-pointer access: a direct `&mut` to the static warns (static_mut_refs)
// and the diagnostic maps past the end of the user's source. Sound here:
// wasm is single-threaded and ticks never overlap.
let sim = unsafe {{ (*&raw mut __IRONPAD_SIM__).as_mut().unwrap() }};
let frame = sim.tick();
let result: TickResult = frame.into();
Box::into_raw(Box::new(result)) as u32
}}
"
    );

    // Preamble: 2 base (use + blank) + 1 if shared_mod.
    let shared_lines = u32::from(has_shared_source);
    let preamble_lines = 2 + shared_lines;

    // Simulations are always sync.
    (code, preamble_lines, false, true)
}

/// Generate the `lib.rs` wrapper for `LiveView` cells.
///
/// `LiveView` cells produce two WASM exports: `cell_main` (initialization) and
/// `cell_tick` (per-tick update returning content). The user source (struct
/// definition + trait impl) appears at module level.
fn generate_live_view_lib_rs(
    source: &str,
    struct_name: &str,
    has_shared_source: bool,
) -> (String, u32, bool, bool) {
    let shared_mod = shared_mod_decl(has_shared_source);

    let code = format!(
        "\
use ironpad_cell::prelude::*;
{shared_mod}
{source}

static mut __IRONPAD_LIVE_VIEW__: Option<{struct_name}> = None;

#[wasm_bindgen]
pub fn cell_main(_input_ptr: u32, _input_len: u32) -> u32 {{
console_error_panic_hook::set_once();
let mut view = {struct_name}::init();
let first_content = view.tick();
let meta = LiveViewMeta {{
    fps: {struct_name}::fps(),
    initial_content: first_content,
}};
unsafe {{ __IRONPAD_LIVE_VIEW__ = Some(view); }}
let __ironpad_output__: CellOutput = meta.into();
let result: CellResult = __ironpad_output__.into();
Box::into_raw(Box::new(result)) as u32
}}

#[wasm_bindgen]
pub fn cell_tick() -> u32 {{
// Raw-pointer access: a direct `&mut` to the static warns (static_mut_refs)
// and the diagnostic maps past the end of the user's source. Sound here:
// wasm is single-threaded and ticks never overlap.
let view = unsafe {{ (*&raw mut __IRONPAD_LIVE_VIEW__).as_mut().unwrap() }};
let content = view.tick();
let result: LiveTickResult = content.into();
Box::into_raw(Box::new(result)) as u32
}}
"
    );

    // Preamble: 2 base (use + blank) + 1 if shared_mod.
    let shared_lines = u32::from(has_shared_source);
    let preamble_lines = 2 + shared_lines;

    // LiveView cells reuse the tick infrastructure (is_simulation=true tells
    // the executor to export cell_tick).
    (code, preamble_lines, false, true)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── is_valid_cell_id ─────────────────────────────────────────────────

    #[test]
    fn accepts_typical_cell_ids() {
        assert!(is_valid_cell_id("c1"));
        assert!(is_valid_cell_id(
            "550e8400-e29b-41d4-a716-446655440000" // a UUID
        ));
        assert!(is_valid_cell_id("Cell_42-abc"));
        assert!(is_valid_cell_id(&"a".repeat(64)));
    }

    #[test]
    fn rejects_path_traversal_and_injection() {
        assert!(!is_valid_cell_id(""), "empty");
        assert!(!is_valid_cell_id("../etc/passwd"), "path traversal");
        assert!(!is_valid_cell_id("a/b"), "slash");
        assert!(!is_valid_cell_id("a.b"), "dot");
        assert!(!is_valid_cell_id("a\"\n[package]"), "manifest injection");
        assert!(!is_valid_cell_id("has space"), "space");
        assert!(!is_valid_cell_id(&"a".repeat(65)), "too long");
    }

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

        let result = generate_cargo_toml("abc123", user_toml, &cell_path, None, false, false);

        assert!(result.contains(r#"name = "cell-abc123""#));
        assert!(result.contains(r#"crate-type = ["cdylib"]"#));
        assert!(result.contains("ironpad-cell = { path ="));
        assert!(result.contains("serde"));
    }

    #[test]
    fn cargo_toml_with_no_user_deps() {
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let result = generate_cargo_toml("cell0", "", &cell_path, None, false, false);

        assert!(result.contains(r#"name = "cell-cell0""#));
        assert!(result.contains(r#"crate-type = ["cdylib"]"#));
        assert!(result.contains("ironpad-cell"));
        assert!(!result.contains(r#"features = ["rayon"]"#));
    }

    #[test]
    fn cargo_toml_with_rayon_enables_feature() {
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let result = generate_cargo_toml("cell0", "rayon = \"1\"", &cell_path, None, true, false);

        assert!(result.contains(r#"features = ["rayon"]"#));
    }

    #[test]
    fn cargo_toml_without_rayon_omits_feature() {
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let result = generate_cargo_toml("cell0", "serde = \"1\"", &cell_path, None, false, false);

        assert!(!result.contains(r#"features = ["rayon"]"#));
    }

    // ── T-002: wasm-bindgen version pin ─────────────────────────────────

    #[test]
    fn cargo_toml_pins_wasm_bindgen_to_exact_cli_version() {
        // Derive the expectation from the toolchain module under test rather
        // than hardcoding a version, so this doesn't break on a wasm-bindgen
        // CLI upgrade or a differently-pinned CI image.
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let result = generate_cargo_toml("cell0", "", &cell_path, None, false, false);

        match super::super::toolchain::wasm_bindgen_cli_version() {
            Some(v) => assert!(
                result.contains(&format!(r#"wasm-bindgen = "={v}""#)),
                "expected exact wasm-bindgen pin matching host CLI {v}, got:\n{result}"
            ),
            None => assert!(
                result.contains(r#"wasm-bindgen = "0.2""#),
                "expected floating wasm-bindgen fallback, got:\n{result}"
            ),
        }
    }

    // ── generate_lib_rs ─────────────────────────────────────────────────

    #[test]
    fn wraps_user_code_in_cell_main() {
        let source = r#"    CellOutput::text("hello")"#;

        let (lib_rs, _, is_async, is_sim) = generate_lib_rs(source, &[], false);

        assert!(!is_async);
        assert!(!is_sim);
        assert!(lib_rs.contains("use ironpad_cell::prelude::*;"));
        assert!(lib_rs.contains("#[wasm_bindgen]"));
        assert!(lib_rs.contains("pub fn cell_main("));
        assert!(lib_rs.contains("console_error_panic_hook::set_once();"));
        assert!(lib_rs.contains("-> u32 {"));
        assert!(lib_rs.contains("CellOutput::text(\"hello\")"));
        assert!(lib_rs.contains("let __ironpad_output__: CellOutput = ({"));
        assert!(lib_rs.contains("}).into();"));
        assert!(lib_rs.contains("let result: CellResult = __ironpad_output__.into();"));
        assert!(lib_rs.contains("Box::into_raw(Box::new(result)) as u32"));
    }

    #[test]
    fn generate_lib_rs_no_previous_cells() {
        let (lib_rs, preamble, is_async, _) = generate_lib_rs("    // user code here", &[], false);

        assert_eq!(preamble, 6);
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
        let types: Vec<String> = vec!["u32".into(), "String".into()];
        let (lib_rs, preamble, ..) = generate_lib_rs("    // user code here", &types, false);

        // 6 base + 3 (ptr reconstruction + inputs) + 2 typed + 1 last = 12
        assert_eq!(preamble, 12);
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
        let types: Vec<String> = vec!["u32".into(), String::new(), "bool".into()];
        let (lib_rs, preamble, ..) = generate_lib_rs("    // user code here", &types, false);

        // 6 base + 3 (ptr reconstruction + inputs) + 2 typed + 1 last = 12
        assert_eq!(preamble, 12);
        assert!(lib_rs.contains("let cell0: u32 = __ironpad_inputs__.get(0).deserialize()"));
        assert!(!lib_rs.contains("let cell1:"));
        assert!(lib_rs.contains("let cell2: bool = __ironpad_inputs__.get(2).deserialize()"));
        assert!(lib_rs.contains("let last = &cell2;"));

        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize].trim(), "// user code here");
    }

    #[test]
    fn generate_lib_rs_all_none_types() {
        let types: Vec<String> = vec![String::new(), String::new()];
        let (lib_rs, preamble, ..) = generate_lib_rs("    // user code here", &types, false);

        // 6 base + 3 (ptr reconstruction + inputs) + 0 typed + 0 last = 9
        assert_eq!(preamble, 9);
        assert!(lib_rs.contains("let __ironpad_inputs__ = CellInputs::from_raw("));
        assert!(!lib_rs.contains("let cell0"));
        assert!(!lib_rs.contains("let cell1"));
        assert!(!lib_rs.contains("let last"));

        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize].trim(), "// user code here");
    }

    #[test]
    fn generate_lib_rs_with_markdown_gaps() {
        // Simulates absolute-index semantics: Markdown cells at indices 0 and 2
        // produce empty type strings; Code cells at indices 1 and 3 have types.
        let types: Vec<String> = vec![String::new(), "u32".into(), String::new(), "String".into()];
        let (lib_rs, preamble, ..) = generate_lib_rs("    // user code here", &types, false);

        // 6 base + 3 (ptr reconstruction + inputs) + 2 typed + 1 last = 12
        assert_eq!(preamble, 12);
        assert!(!lib_rs.contains("let cell0:"));
        assert!(lib_rs.contains("let cell1: u32 = __ironpad_inputs__.get(1).deserialize()"));
        assert!(!lib_rs.contains("let cell2:"));
        assert!(lib_rs.contains("let cell3: String = __ironpad_inputs__.get(3).deserialize()"));
        assert!(lib_rs.contains("let last = &cell3;"));

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

        let (crate_dir, preamble_lines, is_async, _) = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "session-1",
            "cell-0",
            user_source,
            user_cargo,
            &[],
            None,
            None,
        )
        .expect("scaffold should succeed");

        assert_eq!(preamble_lines, 6);
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
        scaffold_micro_crate(
            &tmp,
            &cell_path,
            "s1",
            "c1",
            "    // v1",
            "",
            &[],
            None,
            None,
        )
        .expect("first scaffold");

        // Second scaffold with different source.
        scaffold_micro_crate(
            &tmp,
            &cell_path,
            "s1",
            "c1",
            "    // v2",
            "",
            &[],
            None,
            None,
        )
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
            None,
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
        let (lib_rs, preamble, is_async, _) = generate_lib_rs("    something.await", &[], false);

        assert!(is_async);
        assert_eq!(preamble, 6);
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
        let types: Vec<String> = vec!["u32".into()];
        let (lib_rs, preamble, is_async, _) = generate_lib_rs("    cell0.await", &types, false);

        assert!(is_async);
        // 6 base + 3 (ptr reconstruction + inputs) + 1 typed + 1 last = 11
        assert_eq!(preamble, 11);
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

        let (_, _, is_async, _) = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "s1",
            "c1",
            "    CellOutput::empty()",
            "",
            &[],
            None,
            None,
        )
        .unwrap();
        assert!(!is_async);

        let (_, _, is_async, _) = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "s1",
            "c2",
            "    foo().await",
            "",
            &[],
            None,
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
        let result = generate_cargo_toml("abc", "", &cell_path, Some(shared), false, false);
        assert!(result.contains("[profile.release]"));
        assert!(result.contains("opt-level = 1"));
        assert!(result.contains("serde"));
        assert!(result.contains("ironpad-cell"));
    }

    // ── T-005: Additional edge-case tests ───────────────────────────────

    #[test]
    fn generate_lib_rs_with_unicode_source() {
        let source = "    let msg = \"こんにちは世界 🦀\";\n    CellOutput::text(msg)";
        let (lib_rs, preamble, ..) = generate_lib_rs(source, &[], false);

        assert_eq!(preamble, 6);
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
        let (lib_rs, preamble, is_async, _) = generate_lib_rs("", &[], false);

        assert_eq!(preamble, 6);
        assert!(!is_async);
        // The empty source should still produce a compilable wrapper.
        assert!(lib_rs.contains("let __ironpad_output__: CellOutput = ({"));
        assert!(lib_rs.contains("}).into();"));
    }

    // ── T-013: shared source tests ──────────────────────────────────────

    #[test]
    fn generate_lib_rs_with_shared_source() {
        let (lib_rs, preamble, ..) = generate_lib_rs("    // user code here", &[], true);

        assert!(
            lib_rs.contains("mod shared;"),
            "should contain mod shared declaration"
        );
        // 6 base + 1 shared = 7
        assert_eq!(preamble, 7);

        // Verify user code starts at expected line.
        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize].trim(), "// user code here");
    }

    /// rustc only ever sees one cell's crate, so every shared helper THIS cell
    /// does not call would report `dead_code` even though another cell uses it.
    /// The shared mod carries `allow(dead_code)` to kill that false positive,
    /// and it must stay on a single line so `preamble_lines` (and diagnostic
    /// mapping) are unaffected.
    #[test]
    fn shared_mod_allows_dead_code_without_shifting_preamble() {
        for (label, (lib_rs, preamble)) in [
            ("plain", {
                let (l, p, ..) = generate_lib_rs("    // user code here", &[], true);
                (l, p)
            }),
            ("simulation", {
                let (l, p, ..) = generate_simulation_lib_rs("// user", "Sim", true);
                (l, p)
            }),
            ("live_view", {
                let (l, p, ..) = generate_live_view_lib_rs("// user", "View", true);
                (l, p)
            }),
        ] {
            assert!(
                lib_rs.contains("#[allow(dead_code)] mod shared;"),
                "{label}: shared mod must allow dead_code (helpers used by OTHER cells)"
            );
            // The allow rides on the same line as the mod, so the declaration
            // still costs exactly one preamble line.
            let shared_lines = lib_rs.lines().filter(|l| l.contains("mod shared;")).count();
            assert_eq!(shared_lines, 1, "{label}: shared mod must be one line");
            assert!(preamble > 0, "{label}: preamble should be counted");
        }
    }

    /// Every typed upstream output is bound whether or not this cell reads
    /// it, so the injected `cellN` / `last` bindings carry
    /// `allow(unused_variables)` — a cell that uses only some (or none) of
    /// its inputs must not warn. The allows ride on each binding's line so
    /// `preamble_lines` (and diagnostic mapping) are unaffected.
    #[test]
    fn injected_bindings_allow_unused_without_shifting_preamble() {
        let types: Vec<String> = vec!["u32".into(), "String".into()];
        let (lib_rs, preamble, ..) = generate_lib_rs("    // user code here", &types, false);

        for needle in [
            "#[allow(unused_variables)] let cell0: u32 =",
            "#[allow(unused_variables)] let cell1: String =",
            "#[allow(unused_variables)] let last = &cell1;",
        ] {
            assert!(
                lib_rs.contains(needle),
                "injected binding must allow unused_variables: {needle}"
            );
        }

        // Each allow rides on its binding's line, so the preamble count is
        // exactly what it was without the attributes: 6 base + 3 inputs +
        // 2 typed + 1 last = 12, with user code starting right after.
        assert_eq!(preamble, 12);
        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize].trim(), "// user code here");
    }

    #[test]
    fn generate_lib_rs_without_shared_source() {
        let (lib_rs, preamble, ..) = generate_lib_rs("    // user code here", &[], false);

        assert!(
            !lib_rs.contains("mod shared;"),
            "should not contain mod shared declaration"
        );
        assert_eq!(preamble, 6);

        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize].trim(), "// user code here");
    }

    #[test]
    fn scaffold_micro_crate_writes_shared_rs() {
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let shared_src = "pub fn helper() -> u32 { 42 }";

        let (crate_dir, preamble, ..) = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "s1",
            "c1",
            "    CellOutput::empty()",
            "",
            &[],
            None,
            Some(shared_src),
        )
        .unwrap();

        let shared_path = crate_dir.join("src/shared.rs");
        assert!(shared_path.is_file(), "shared.rs should be written");
        let content = std::fs::read_to_string(&shared_path).unwrap();
        assert_eq!(content, shared_src);

        // lib.rs should contain mod shared
        let lib = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        assert!(lib.contains("mod shared;"));

        // preamble should be 7 (6 base + 1 shared)
        assert_eq!(preamble, 7);
    }

    #[test]
    fn scaffold_micro_crate_no_shared_rs_when_none() {
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");

        let (crate_dir, preamble, ..) = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "s1",
            "c1",
            "    CellOutput::empty()",
            "",
            &[],
            None,
            None,
        )
        .unwrap();

        let shared_path = crate_dir.join("src/shared.rs");
        assert!(!shared_path.exists(), "shared.rs should not exist");

        let lib = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        assert!(!lib.contains("mod shared;"));

        assert_eq!(preamble, 6);
    }

    // ── T-006: Simulation detection and scaffold ────────────────────────

    #[test]
    fn is_simulation_detects_impl() {
        assert_eq!(
            is_simulation("impl Simulation for Pendulum"),
            Some("Pendulum".into())
        );
    }

    #[test]
    fn is_simulation_handles_whitespace_variations() {
        assert_eq!(
            is_simulation("impl  Simulation  for  MyStruct"),
            Some("MyStruct".into())
        );
        assert_eq!(
            is_simulation("  impl Simulation for Foo {"),
            Some("Foo".into())
        );
    }

    #[test]
    fn is_simulation_returns_none_for_normal_code() {
        assert_eq!(is_simulation("let x = 42;"), None);
        assert_eq!(is_simulation("impl Display for Foo"), None);
        assert_eq!(is_simulation("// impl Simulation for Foo"), None);
    }

    #[test]
    fn is_simulation_multiline_source() {
        let source = "\
struct Wave { t: f64 }

impl Simulation for Wave {
    fn init() -> Self { Wave { t: 0.0 } }
    fn tick(&mut self) -> Canvas { todo!() }
}";
        assert_eq!(is_simulation(source), Some("Wave".into()));
    }

    #[test]
    fn generate_lib_rs_simulation_mode() {
        let source = "\
struct Pendulum { angle: f64 }

impl Simulation for Pendulum {
    fn init() -> Self { Pendulum { angle: 1.0 } }
    fn tick(&mut self) -> Canvas { todo!() }
}";
        let (lib_rs, preamble, is_async, is_sim) = generate_lib_rs(source, &[], false);

        assert!(!is_async);
        assert!(is_sim);
        assert_eq!(preamble, 2);

        // Both exports present.
        assert!(lib_rs.contains("pub fn cell_main("));
        assert!(lib_rs.contains("pub fn cell_tick()"));

        // Simulation-specific code.
        assert!(lib_rs.contains("static mut __IRONPAD_SIM__: Option<Pendulum>"));
        assert!(lib_rs.contains("Pendulum::init()"));
        assert!(lib_rs.contains("Pendulum::fps()"));
        assert!(lib_rs.contains("SimulationMeta"));
        assert!(lib_rs.contains("TickResult"));

        // The tick wrapper must reach the static through a raw pointer: a
        // direct `&mut` warns (static_mut_refs) with a span mapped past the
        // end of the user's source.
        assert!(lib_rs.contains("(*&raw mut __IRONPAD_SIM__).as_mut()"));
        assert!(!lib_rs.contains(" __IRONPAD_SIM__.as_mut()"));

        // User source present in generated code.
        assert!(lib_rs.contains("struct Pendulum"));
        assert!(lib_rs.contains("impl Simulation for Pendulum"));

        // Should NOT contain normal expression wrapper.
        assert!(!lib_rs.contains("let __ironpad_output__: CellOutput = ({"));
    }

    #[test]
    fn generate_lib_rs_simulation_preamble_count() {
        let source = "struct S {}\nimpl Simulation for S {}";
        let (lib_rs, preamble, ..) = generate_lib_rs(source, &[], false);

        // Preamble: 2 (use + blank).
        assert_eq!(preamble, 2);

        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize], "struct S {}");
    }

    #[test]
    fn generate_lib_rs_simulation_preamble_with_shared() {
        let source = "struct S {}\nimpl Simulation for S {}";
        let (lib_rs, preamble, ..) = generate_lib_rs(source, &[], true);

        // Preamble: 2 base + 1 shared = 3.
        assert_eq!(preamble, 3);

        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize], "struct S {}");
    }

    #[test]
    fn generate_lib_rs_non_simulation_regression() {
        // Normal code should still produce the standard wrapper.
        let source = "    CellOutput::text(\"hello\")";
        let (lib_rs, preamble, is_async, is_sim) = generate_lib_rs(source, &[], false);

        assert!(!is_async);
        assert!(!is_sim);
        assert_eq!(preamble, 6);
        assert!(lib_rs.contains("let __ironpad_output__: CellOutput = ({"));
        assert!(!lib_rs.contains("cell_tick"));
        assert!(!lib_rs.contains("__IRONPAD_SIM__"));
    }

    // ── T-003/T-008: LiveView detection and scaffold ────────────────────

    #[test]
    fn is_live_view_detects_impl() {
        assert_eq!(
            is_live_view("impl LiveView for Dashboard"),
            Some("Dashboard".into())
        );
    }

    #[test]
    fn is_live_view_handles_whitespace_variations() {
        assert_eq!(
            is_live_view("impl  LiveView  for  MyView"),
            Some("MyView".into())
        );
        assert_eq!(
            is_live_view("  impl LiveView for Foo {"),
            Some("Foo".into())
        );
    }

    #[test]
    fn is_live_view_returns_none_for_non_live_view() {
        assert_eq!(is_live_view("let x = 42;"), None);
        assert_eq!(is_live_view("impl Simulation for Foo"), None);
        assert_eq!(is_live_view("impl Display for Foo"), None);
        assert_eq!(is_live_view("// impl LiveView for Foo"), None);
    }

    #[test]
    fn is_live_view_multiline_source() {
        let source = "\
struct Dashboard { count: u32 }

impl LiveView for Dashboard {
    fn init() -> Self { Dashboard { count: 0 } }
    fn tick(&mut self) -> LiveContent { LiveContent::Text(\"hi\".into()) }
}";
        assert_eq!(is_live_view(source), Some("Dashboard".into()));
    }

    #[test]
    fn generate_lib_rs_live_view_mode() {
        let source = "\
struct Dashboard { count: u32 }

impl LiveView for Dashboard {
    fn init() -> Self { Dashboard { count: 0 } }
    fn tick(&mut self) -> LiveContent { LiveContent::Text(\"hi\".into()) }
}";
        let (lib_rs, preamble, is_async, is_sim) = generate_lib_rs(source, &[], false);

        assert!(!is_async);
        assert!(is_sim); // reuses tick infrastructure
        assert_eq!(preamble, 2);

        // Both exports present.
        assert!(lib_rs.contains("pub fn cell_main("));
        assert!(lib_rs.contains("pub fn cell_tick()"));

        // LiveView-specific code.
        assert!(lib_rs.contains("static mut __IRONPAD_LIVE_VIEW__: Option<Dashboard>"));
        assert!(lib_rs.contains("Dashboard::init()"));
        assert!(lib_rs.contains("Dashboard::fps()"));
        assert!(lib_rs.contains("LiveViewMeta"));
        assert!(lib_rs.contains("LiveTickResult"));

        // The tick wrapper must reach the static through a raw pointer: a
        // direct `&mut` warns (static_mut_refs) with a span mapped past the
        // end of the user's source — how this surfaced in cannon's
        // "Newton, drawn" cell.
        assert!(lib_rs.contains("(*&raw mut __IRONPAD_LIVE_VIEW__).as_mut()"));
        assert!(!lib_rs.contains(" __IRONPAD_LIVE_VIEW__.as_mut()"));

        // User source present in generated code.
        assert!(lib_rs.contains("struct Dashboard"));
        assert!(lib_rs.contains("impl LiveView for Dashboard"));

        // Should NOT contain normal expression wrapper or Simulation code.
        assert!(!lib_rs.contains("let __ironpad_output__: CellOutput = ({"));
        assert!(!lib_rs.contains("__IRONPAD_SIM__"));
        assert!(!lib_rs.contains(": TickResult"));
    }

    #[test]
    fn generate_lib_rs_live_view_preamble_count() {
        let source = "struct V {}\nimpl LiveView for V {}";
        let (lib_rs, preamble, ..) = generate_lib_rs(source, &[], false);

        // Preamble: 2 (use + blank).
        assert_eq!(preamble, 2);

        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize], "struct V {}");
    }

    #[test]
    fn generate_lib_rs_live_view_preamble_with_shared() {
        let source = "struct V {}\nimpl LiveView for V {}";
        let (lib_rs, preamble, ..) = generate_lib_rs(source, &[], true);

        // Preamble: 2 base + 1 shared = 3.
        assert_eq!(preamble, 3);

        let lines: Vec<&str> = lib_rs.lines().collect();
        assert_eq!(lines[preamble as usize], "struct V {}");
    }

    #[test]
    fn generate_lib_rs_live_view_does_not_match_simulation() {
        // A Simulation impl should NOT be caught by the LiveView detector.
        let source = "struct S {}\nimpl Simulation for S {\n    fn init() -> Self { S {} }\n    fn tick(&mut self) -> Canvas { todo!() }\n}";
        let (lib_rs, _, _, _) = generate_lib_rs(source, &[], false);

        // Should use simulation path, not live view.
        assert!(lib_rs.contains("__IRONPAD_SIM__"));
        assert!(!lib_rs.contains("__IRONPAD_LIVE_VIEW__"));
    }

    // ── merged_deps_contain_rayon ────────────────────────────────────────

    #[test]
    fn merged_deps_contain_rayon_detects_simple() {
        assert!(merged_deps_contain_rayon(
            None,
            "[dependencies]\nrayon = \"1\""
        ));
    }

    #[test]
    fn merged_deps_contain_rayon_detects_in_shared() {
        assert!(merged_deps_contain_rayon(
            Some("[dependencies]\nrayon = \"1\""),
            "[dependencies]"
        ));
    }

    #[test]
    fn merged_deps_contain_rayon_false_when_absent() {
        assert!(!merged_deps_contain_rayon(
            None,
            "[dependencies]\nserde = \"1\""
        ));
    }

    // ── WASM SIMD opt-in (PRD-0042) ─────────────────────────────────────

    #[test]
    fn uses_wasm_simd_detects_in_cell_source() {
        assert!(uses_wasm_simd("use std::simd::prelude::*;", None));
        assert!(uses_wasm_simd(
            "let v = core::simd::f32x4::splat(0.0);",
            None
        ));
        assert!(uses_wasm_simd("use std::arch::wasm32::f32x4_add;", None));
        assert!(!uses_wasm_simd("let x = 42;", None));
    }

    #[test]
    fn uses_wasm_simd_detects_in_shared_source() {
        let shared = "use std::simd::prelude::*;\npub fn dot(a: &[f32]) -> f32 { a[0] }";
        assert!(uses_wasm_simd("shared::dot(&[1.0])", Some(shared)));
        assert!(!uses_wasm_simd("let x = 1;", Some("pub fn f() {}")));
    }

    #[test]
    fn simd_scaffold_adds_feature_gate_and_bumps_preamble() {
        let dir = tempfile::tempdir().unwrap();

        // Baseline without simd for the preamble delta.
        let (_, base_preamble, ..) = scaffold_micro_crate(
            dir.path(),
            Path::new("crates/ironpad-cell"),
            "s",
            "no-simd",
            "    1",
            "[dependencies]",
            &[],
            None,
            None,
        )
        .unwrap();

        let (crate_dir, preamble, ..) = scaffold_micro_crate(
            dir.path(),
            Path::new("crates/ironpad-cell"),
            "s",
            "with-simd",
            "    use std::simd::prelude::*;\n    f32x4::splat(1.0).reduce_sum()",
            "[dependencies]",
            &[],
            None,
            None,
        )
        .unwrap();

        let lib_rs = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        assert!(
            lib_rs.starts_with("#![feature(portable_simd)]\n"),
            "feature gate must be the first crate-root line"
        );
        assert_eq!(
            preamble,
            base_preamble + 1,
            "the injected feature line shifts diagnostics by exactly one"
        );
    }

    #[test]
    fn coroutine_gate_is_injected_and_bumps_the_preamble() {
        let dir = tempfile::tempdir().unwrap();

        let (_, base_preamble, ..) = scaffold_micro_crate(
            dir.path(),
            Path::new("crates/ironpad-cell"),
            "s",
            "no-coroutine",
            "    1u32",
            "[dependencies]",
            &[],
            None,
            None,
        )
        .unwrap();

        let (crate_dir, preamble, ..) = scaffold_micro_crate(
            dir.path(),
            Path::new("crates/ironpad-cell"),
            "s",
            "with-coroutine",
            "    let mut c = #[coroutine] |_: ()| { yield 1u32; 2u32 };\n    0u32",
            "[dependencies]",
            &[],
            None,
            None,
        )
        .unwrap();

        let lib_rs = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        assert!(
            lib_rs.starts_with("#![feature(coroutines, coroutine_trait, stmt_expr_attributes)]\n"),
            "feature gate must be the first crate-root line"
        );
        assert_eq!(
            preamble,
            base_preamble + 1,
            "the injected feature line shifts diagnostics by exactly one"
        );
    }

    #[test]
    fn simd_and_autodiff_gates_compose_with_two_preamble_bumps() {
        let dir = tempfile::tempdir().unwrap();
        let cell_src =
            "    use std::simd::prelude::*;\n    let (_v, g) = shared::d_f(2.0, 1.0);\n    g";
        let shared = "#[autodiff_reverse(d_f, Active, Active)]\npub fn f(x: f64) -> f64 { x * x }";

        let (_, base_preamble, ..) = scaffold_micro_crate(
            dir.path(),
            Path::new("crates/ironpad-cell"),
            "s",
            "plain",
            "    1",
            "[dependencies]",
            &[],
            None,
            Some("pub fn f() {}"),
        )
        .unwrap();

        let (crate_dir, preamble, ..) = scaffold_micro_crate(
            dir.path(),
            Path::new("crates/ironpad-cell"),
            "s",
            "simd-ad",
            cell_src,
            "[dependencies]",
            &[],
            None,
            Some(shared),
        )
        .unwrap();

        let lib_rs = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        let lines: Vec<&str> = lib_rs.lines().take(2).collect();
        assert_eq!(lines[0], "#![feature(portable_simd)]");
        assert_eq!(lines[1], "#![feature(autodiff)]");
        assert_eq!(
            preamble,
            base_preamble + 2,
            "each injected gate shifts diagnostics by one line"
        );
    }

    // ── std::autodiff opt-in (PRD-0041) ─────────────────────────────────

    #[test]
    fn uses_std_autodiff_detects_in_cell_source() {
        assert!(uses_std_autodiff(
            "let g = d_f(1.0, 1.0); // autodiff_reverse",
            None
        ));
        assert!(uses_std_autodiff(
            "use std::autodiff::autodiff_forward;",
            None
        ));
        assert!(!uses_std_autodiff("let x = 42;", None));
    }

    #[test]
    fn uses_std_autodiff_detects_in_shared_source() {
        let shared = "#[autodiff_reverse(d_f, Active, Active)]\npub fn f(x: f64) -> f64 { x * x }";
        assert!(uses_std_autodiff("d_f(2.0, 1.0)", Some(shared)));
        assert!(!uses_std_autodiff("let x = 1;", Some("pub fn f() {}")));
    }

    #[test]
    fn autodiff_scaffold_adds_feature_gate_and_bumps_preamble() {
        let dir = tempfile::tempdir().unwrap();
        let cell_src = "    let (_v, g) = shared::d_f(2.0, 1.0);\n    g";
        let shared = "#[autodiff_reverse(d_f, Active, Active)]\npub fn f(x: f64) -> f64 { x * x }";

        // Baseline without autodiff for the preamble delta.
        let (_, base_preamble, ..) = scaffold_micro_crate(
            dir.path(),
            Path::new("crates/ironpad-cell"),
            "s",
            "no-ad",
            "    1",
            "[dependencies]",
            &[],
            None,
            Some("pub fn f() {}"),
        )
        .unwrap();

        let (crate_dir, preamble, ..) = scaffold_micro_crate(
            dir.path(),
            Path::new("crates/ironpad-cell"),
            "s",
            "with-ad",
            cell_src,
            "[dependencies]",
            &[],
            None,
            Some(shared),
        )
        .unwrap();

        let lib_rs = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        assert!(
            lib_rs.starts_with("#![feature(autodiff)]\n"),
            "feature gate must be the first crate-root line"
        );
        assert_eq!(
            preamble,
            base_preamble + 1,
            "the injected feature line shifts diagnostics by exactly one"
        );
    }

    #[test]
    fn autodiff_cargo_toml_enforces_fat_lto_over_user_profile() {
        let shared =
            "[dependencies]\n\n[profile.release]\nopt-level = 1\nlto = false\ncodegen-units = 16";
        let toml = generate_cargo_toml(
            "c1",
            "[dependencies]",
            Path::new("/cell"),
            Some(shared),
            false,
            true,
        );
        assert!(
            toml.contains("lto = \"fat\""),
            "lto must be forced to fat: {toml}"
        );
        assert!(
            toml.contains("codegen-units = 1"),
            "cgu must be forced to 1: {toml}"
        );
        assert!(
            !toml.contains("lto = false"),
            "user lto must be replaced: {toml}"
        );
        assert!(
            !toml.contains("codegen-units = 16"),
            "user cgu must be replaced: {toml}"
        );
        assert!(
            toml.contains("opt-level = 1"),
            "other user keys survive: {toml}"
        );
    }

    #[test]
    fn autodiff_cargo_toml_appends_profile_when_absent() {
        let toml = generate_cargo_toml(
            "c2",
            "[dependencies]",
            Path::new("/cell"),
            None,
            false,
            true,
        );
        assert!(toml.contains("[profile.release]"));
        assert!(toml.contains("lto = \"fat\""));
        assert!(toml.contains("codegen-units = 1"));
    }

    #[test]
    fn merged_deps_contain_rayon_detects_table_form() {
        assert!(merged_deps_contain_rayon(
            None,
            "[dependencies]\nrayon = { version = \"1\", features = [\"x\"] }"
        ));
    }
}
