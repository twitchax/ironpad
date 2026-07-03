//! Server-side compilation pipeline.

pub mod build;
pub mod cache;
pub mod diagnostics;
pub mod optimize;
pub mod scaffold;
pub mod toolchain;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Serializes concurrent compiles that share a cell id.
///
/// Each cell scaffolds into `{cache}/workspaces/default/{cell_id}`, a path keyed
/// only by `cell_id`. Two requests with the same id (the same notebook in two
/// tabs, or an attacker-chosen id colliding with a victim's) would otherwise
/// race: one overwrites the other's scaffolded source, and the loser builds the
/// wrong source and caches it under its *own* content hash — cache poisoning.
/// Holding the per-cell lock across scaffold → build → cache makes those
/// requests run one at a time. Distinct cell ids use distinct dirs and never
/// contend. Cloneable; all clones share one lock table.
#[derive(Clone, Default)]
pub struct CompileLocks {
    locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

impl CompileLocks {
    /// Acquire the lock for `cell_id`, awaiting any in-flight compile of the
    /// same id. The returned guard must be held for the whole compile.
    pub async fn acquire(&self, cell_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let cell_lock = {
            let mut table = self.locks.lock().expect("compile-lock table poisoned");
            table.entry(cell_id.to_string()).or_default().clone()
        };
        cell_lock.lock_owned().await
    }
}

#[cfg(test)]
mod compile_locks_tests {
    use super::CompileLocks;
    use std::time::Duration;

    #[tokio::test]
    async fn same_cell_serializes() {
        let locks = CompileLocks::default();
        let guard = locks.acquire("cell-1").await;

        // A second acquire of the same cell cannot complete while the first is held.
        let blocked =
            tokio::time::timeout(Duration::from_millis(50), locks.acquire("cell-1")).await;
        assert!(blocked.is_err(), "same-cell acquire must block while held");

        drop(guard);
        let after = tokio::time::timeout(Duration::from_millis(500), locks.acquire("cell-1")).await;
        assert!(after.is_ok(), "acquire should succeed after release");
    }

    #[tokio::test]
    async fn different_cells_do_not_block() {
        let locks = CompileLocks::default();
        let _guard = locks.acquire("cell-1").await;

        // A different cell must not contend on the first cell's lock.
        let other = tokio::time::timeout(Duration::from_millis(500), locks.acquire("cell-2")).await;
        assert!(other.is_ok(), "distinct cells should not block each other");
    }
}

// ── Cross-module pipeline integration tests ─────────────────────────────────

#[cfg(test)]
mod pipeline_tests {
    use std::path::PathBuf;

    use super::cache::content_hash;
    use super::diagnostics::parse_diagnostics;
    use super::scaffold::{generate_lib_rs, scaffold_micro_crate};

    // ── Scaffolding → Content Verification ──────────────────────────────

    #[test]
    fn scaffolded_crate_has_valid_cargo_toml_and_lib_rs() {
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad/crates/ironpad-cell");
        let source = "    CellOutput::text(\"hello\")";
        let cargo_toml = "[dependencies]\nserde = \"1\"";

        let (crate_dir, _preamble_lines, _is_async, _, _) = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "sess-1",
            "cell-0",
            source,
            cargo_toml,
            &[],
            None,
            None,
        )
        .expect("scaffold should succeed");

        // Cargo.toml must contain package metadata and the ironpad-cell dependency.
        let cargo = std::fs::read_to_string(crate_dir.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("[package]"), "missing [package] section");
        assert!(
            cargo.contains("crate-type = [\"cdylib\"]"),
            "missing cdylib crate type"
        );
        assert!(
            cargo.contains("ironpad-cell"),
            "missing ironpad-cell dependency"
        );
        assert!(cargo.contains("serde"), "missing user dependency");

        // lib.rs must wrap user code in cell_main with the ironpad_cell prelude.
        let lib = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        assert!(
            lib.contains("use ironpad_cell::prelude::*;"),
            "missing prelude import"
        );
        assert!(
            lib.contains("pub fn cell_main("),
            "missing cell_main wasm-bindgen entry"
        );
        assert!(
            lib.contains("CellOutput::text(\"hello\")"),
            "user code not embedded"
        );
        assert!(
            lib.contains("let __ironpad_output__: CellOutput = ({"),
            "missing CellOutput wrapper"
        );
    }

    // ── Hashing → Scaffold → Consistency ────────────────────────────────

    #[test]
    fn identical_inputs_produce_same_hash_and_scaffold_content() {
        let source = "    let x = 42;\n    CellOutput::text(&x.to_string())";
        let cargo_toml = "[dependencies]\nrand = \"0.8\"";

        // Hashes must match.
        let hash_a = content_hash(source, cargo_toml, &[], None, None, false);
        let hash_b = content_hash(source, cargo_toml, &[], None, None, false);
        assert_eq!(hash_a, hash_b, "same inputs must produce identical hashes");

        // Scaffolded content must be identical for the same inputs.
        let tmp_a = tempdir();
        let tmp_b = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");

        let (dir_a, ..) = scaffold_micro_crate(
            &tmp_a,
            &cell_path,
            "s",
            "c",
            source,
            cargo_toml,
            &[],
            None,
            None,
        )
        .unwrap();
        let (dir_b, ..) = scaffold_micro_crate(
            &tmp_b,
            &cell_path,
            "s",
            "c",
            source,
            cargo_toml,
            &[],
            None,
            None,
        )
        .unwrap();

        let cargo_a = std::fs::read_to_string(dir_a.join("Cargo.toml")).unwrap();
        let cargo_b = std::fs::read_to_string(dir_b.join("Cargo.toml")).unwrap();
        assert_eq!(cargo_a, cargo_b, "Cargo.toml should be deterministic");

        let lib_a = std::fs::read_to_string(dir_a.join("src/lib.rs")).unwrap();
        let lib_b = std::fs::read_to_string(dir_b.join("src/lib.rs")).unwrap();
        assert_eq!(lib_a, lib_b, "lib.rs should be deterministic");
    }

    #[test]
    fn changed_source_invalidates_hash() {
        let cargo = "[dependencies]";
        let hash_v1 = content_hash("    let x = 1;", cargo, &[], None, None, false);
        let hash_v2 = content_hash("    let x = 2;", cargo, &[], None, None, false);
        assert_ne!(
            hash_v1, hash_v2,
            "different source must produce different hashes"
        );
    }

    #[test]
    fn changed_cargo_toml_invalidates_hash() {
        let source = "    CellOutput::empty()";
        let hash_a = content_hash(
            source,
            "[dependencies]\nserde = \"1\"",
            &[],
            None,
            None,
            false,
        );
        let hash_b = content_hash(
            source,
            "[dependencies]\nrand = \"0.8\"",
            &[],
            None,
            None,
            false,
        );
        assert_ne!(
            hash_a, hash_b,
            "different Cargo.toml must produce different hashes"
        );
    }

    // ── Wrapper Offset → Diagnostic Span Consistency ────────────────────

    #[test]
    fn wrapper_offset_matches_generated_lib_rs_layout() {
        let user_code = "    let x = 42;";
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");

        let (dir, preamble_lines, ..) =
            scaffold_micro_crate(&tmp, &cell_path, "s", "c", user_code, "", &[], None, None)
                .unwrap();
        let lib = std::fs::read_to_string(dir.join("src/lib.rs")).unwrap();

        // User code must start at exactly line preamble_lines + 1 (1-indexed).
        let lines: Vec<&str> = lib.lines().collect();
        assert_eq!(
            lines[preamble_lines as usize].trim(),
            "let x = 42;",
            "user code should appear at line index {} (1-indexed line {})",
            preamble_lines,
            preamble_lines + 1,
        );
    }

    #[test]
    fn diagnostic_spans_correctly_map_to_user_code_lines() {
        // Simulate a type error on wrapper line 7 (user line 2 given preamble of 5).
        let json = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0 (path+file:///tmp/cell)","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error[E0308]: mismatched types\n","children":[],"code":{"code":"E0308","explanation":"Expected type did not match the received type."},"level":"error","message":"mismatched types","spans":[{"byte_end":200,"byte_start":190,"column_end":15,"column_start":5,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"expected `i32`, found `&str`","line_end":7,"line_start":7,"suggested_replacement":null,"suggestion_applicability":null,"text":[{"highlight_end":15,"highlight_start":5,"text":"    \"hello\""}]}]}}"#;

        let preamble: u32 = 5;
        let diags = parse_diagnostics(json, preamble);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].spans.len(), 1);

        let span = &diags[0].spans[0];
        // Wrapper line 7 - preamble(5) = user line 2.
        assert_eq!(span.line_start, 7 - preamble);
        assert_eq!(span.line_end, 7 - preamble);
    }

    // ── Full pipeline flow (minus build) ────────────────────────────────

    #[test]
    fn pipeline_hash_scaffold_diagnostics_round_trip() {
        let source = "    let x: i32 = \"oops\";\n    CellOutput::empty()";
        let cargo_toml = "[dependencies]";

        // Step 1: Hash the input.
        let hash = content_hash(source, cargo_toml, &[], None, None, false);
        assert_eq!(hash.len(), 64, "blake3 hash should be 64 hex chars");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash should be hex"
        );

        // Step 2: Scaffold the micro-crate.
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let (crate_dir, preamble_lines, ..) = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "session",
            "cell-0",
            source,
            cargo_toml,
            &[],
            None,
            None,
        )
        .unwrap();

        assert!(crate_dir.join("Cargo.toml").is_file());
        assert!(crate_dir.join("src/lib.rs").is_file());

        // Step 3: Verify scaffolded lib.rs contains user code at the right offset.
        let lib = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        let lines: Vec<&str> = lib.lines().collect();
        assert!(
            lines[preamble_lines as usize].contains("let x: i32"),
            "first line of user code should be at preamble offset"
        );

        // Step 4: Parse mock build output (simulated cargo JSON diagnostics).
        let mock_cargo_output = r#"{"reason":"compiler-message","package_id":"cell-cell-0 0.1.0 (path+file:///tmp/cell)","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-cell-0","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error[E0308]: mismatched types\n","children":[],"code":{"code":"E0308","explanation":null},"level":"error","message":"mismatched types","spans":[{"byte_end":200,"byte_start":190,"column_end":30,"column_start":22,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"expected `i32`, found `&str`","line_end":7,"line_start":7,"suggested_replacement":null,"suggestion_applicability":null,"text":[]}]}}"#;

        let diagnostics = parse_diagnostics(mock_cargo_output, preamble_lines);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, ironpad_common::Severity::Error);
        assert_eq!(diagnostics[0].code.as_deref(), Some("E0308"));

        // The error was on wrapper line 7 → user line 1.
        assert_eq!(diagnostics[0].spans[0].line_start, 1);
    }

    // ── Typed Scaffold Generation ──────────────────────────────────────

    #[test]
    fn scaffold_generates_typed_cell_bindings() {
        // With two typed previous cells.
        let types: Vec<String> = vec!["u32".into(), "String".into()];
        let (code, preamble, ..) = generate_lib_rs("    let x = cell0 + 1;", &types, false);

        assert!(code.contains("let cell0: u32"));
        assert!(code.contains("let cell1: String"));
        assert!(code.contains("let last = &cell1"));
        assert!(code.contains("__ironpad_inputs__"));
        assert_eq!(preamble, 12, "6 base + 3 (ptr + inputs) + 2 cells + 1 last");

        // With no previous cells.
        let (code_empty, preamble_empty, ..) = generate_lib_rs("    let x = 1;", &[], false);

        assert!(!code_empty.contains("__ironpad_inputs__"));
        assert!(!code_empty.contains("let cell"));
        assert_eq!(preamble_empty, 6);
    }

    // ── Cache round-trip with content hash ──────────────────────────────

    #[test]
    fn cache_round_trip_with_pipeline_hash() {
        use super::cache::{store_blob, try_cache_hit};

        let source = "    CellOutput::text(\"cached\")";
        let cargo = "[dependencies]";
        let hash = content_hash(source, cargo, &[], None, None, false);

        let cache_dir = tempdir();
        let fake_wasm = b"\x00asm\x01\x00\x00\x00fake-wasm-bytes";
        let fake_js_glue = "export function init() {}";

        // Cache miss before storing.
        assert!(try_cache_hit(&cache_dir, &hash).is_none());

        // Store and verify cache hit.
        store_blob(&cache_dir, &hash, fake_wasm, Some(fake_js_glue), &[]).unwrap();
        let hit = try_cache_hit(&cache_dir, &hash).expect("should be a cache hit");
        assert_eq!(hit.wasm_bytes, fake_wasm);
        assert_eq!(hit.js_glue.as_deref(), Some(fake_js_glue));

        // A different source must not hit the same cache entry.
        let different_hash = content_hash(
            "    CellOutput::text(\"different\")",
            cargo,
            &[],
            None,
            None,
            false,
        );
        assert!(try_cache_hit(&cache_dir, &different_hash).is_none());
    }

    // ── Test Helpers ────────────────────────────────────────────────────

    fn tempdir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ironpad-pipeline-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}

// ── E2E integration tests (require Rust toolchain + wasm32-unknown-unknown) ──

#[cfg(test)]
mod e2e_tests {
    use std::path::PathBuf;

    use super::build::{build_micro_crate, check_micro_crate, BuildResult, CheckResult};
    use super::cache::{content_hash, store_blob, try_cache_hit};
    use super::diagnostics::parse_diagnostics;
    use super::scaffold::scaffold_micro_crate;

    /// Resolve the path to the `ironpad-cell` crate relative to this crate's manifest.
    fn ironpad_cell_path() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest_dir.join("../ironpad-cell")
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ironpad-e2e-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ── Successful compilation ──────────────────────────────────────────

    #[tokio::test]
    #[ignore = "slow: invokes cargo build --target wasm32-unknown-unknown"]
    async fn compile_trivial_cell_produces_valid_wasm_blob() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-session";
        let cell_id = "trivial";
        let source = "    CellOutput::empty()";
        let cargo_toml = "[dependencies]";
        // Scaffold the micro-crate.
        let (crate_dir, ..) = scaffold_micro_crate(
            &cache_dir,
            &cell_path,
            session_id,
            cell_id,
            source,
            cargo_toml,
            &[],
            None,
            None,
        )
        .expect("scaffold should succeed");

        assert!(crate_dir.join("Cargo.toml").is_file());
        assert!(crate_dir.join("src/lib.rs").is_file());

        // Build to WASM.
        let result = build_micro_crate(&crate_dir, &cache_dir, session_id, cell_id, None, false)
            .await
            .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success {
                wasm_path, js_glue, ..
            } => {
                assert!(wasm_path.exists(), "WASM blob should exist on disk");

                let wasm_bytes = std::fs::read(&wasm_path).unwrap();
                assert!(
                    wasm_bytes.len() > 8,
                    "WASM blob should not be trivially small"
                );

                // Validate WASM magic number: \0asm
                assert_eq!(
                    &wasm_bytes[..4],
                    b"\x00asm",
                    "WASM blob should start with the WASM magic number",
                );

                // wasm-bindgen should have produced JS glue.
                assert!(!js_glue.is_empty(), "JS glue should not be empty");
                assert!(
                    js_glue.contains("export"),
                    "JS glue should contain ES module exports",
                );
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!(
                    "expected successful compilation but got failure.\nstdout: {stdout}\nstderr: {stderr}"
                );
            }
        }
    }

    // ── Host-import linking (PRD-0031 T-004 regression) ─────────────────

    /// Regression test for PRD-0031 T-004: a cell that calls the
    /// simulation-bus / host-message FFI (`sim::read`, `host_message`) must
    /// still *link* against the `env` wasm import module. Before
    /// `ironpad-cell`'s `extern "C"` host-import blocks were annotated with
    /// `#[link(wasm_import_module = "env")]`, current nightly `rust-lld`
    /// rejected these as undefined symbols (e.g. `undefined symbol:
    /// ironpad_sim_read`) because it no longer defaults to
    /// `--allow-undefined` for `wasm32-unknown-unknown`.
    ///
    /// Note: this guard only *discriminates* on nightlies that dropped the
    /// `--allow-undefined` default for wasm32. Verified experimentally that
    /// the pinned `nightly-2025-12-22` (see `rust-toolchain.toml`) still
    /// allows undefined symbols, so this test passes with or without the
    /// `#[link(...)]` fix while that pin is in place — the fix is correct
    /// and necessary regardless, but the test's teeth return once the pin is
    /// bumped forward to a strict nightly (e.g. 2026-06-01+). Browser-level
    /// module-name correctness is independently covered by the Playwright
    /// uat-003 acceptance test in PRD-0031.
    #[tokio::test]
    #[ignore = "slow: invokes cargo build --target wasm32-unknown-unknown"]
    async fn compile_cell_with_host_imports_links_successfully() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-session";
        let cell_id = "host-imports";
        // References both `ironpad_sim_read` (via `sim::read`) and
        // `ironpad_host_message` (via `host_message`) so the build must
        // resolve both host imports at link time.
        let source = "    let val: Option<f64> = sim::read(\"regression_key\");\n    host_message(\"prd-0031-regression\");\n    CellOutput::from(val.unwrap_or(0.0))";
        let cargo_toml = "[dependencies]";

        let (crate_dir, ..) = scaffold_micro_crate(
            &cache_dir,
            &cell_path,
            session_id,
            cell_id,
            source,
            cargo_toml,
            &[],
            None,
            None,
        )
        .expect("scaffold should succeed");

        let result = build_micro_crate(&crate_dir, &cache_dir, session_id, cell_id, None, false)
            .await
            .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success { wasm_path, .. } => {
                assert!(wasm_path.exists(), "WASM blob should exist on disk");
                let wasm_bytes = std::fs::read(&wasm_path).unwrap();
                assert_eq!(
                    &wasm_bytes[..4],
                    b"\x00asm",
                    "WASM blob should start with the WASM magic number",
                );
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!(
                    "cell using sim::read/host_message should link successfully but got failure.\nstdout: {stdout}\nstderr: {stderr}"
                );
            }
        }
    }

    // ── Compilation failure produces diagnostics ────────────────────────

    #[tokio::test]
    #[ignore = "slow: invokes cargo build --target wasm32-unknown-unknown"]
    async fn compile_bad_code_returns_diagnostics() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-session";
        let cell_id = "badcode";
        // Deliberately broken: assigning a string to i32.
        let source = "    let x: i32 = \"oops\";\n    CellOutput::empty()";
        let cargo_toml = "[dependencies]";

        let (crate_dir, preamble_lines, ..) = scaffold_micro_crate(
            &cache_dir,
            &cell_path,
            session_id,
            cell_id,
            source,
            cargo_toml,
            &[],
            None,
            None,
        )
        .expect("scaffold should succeed");

        let result = build_micro_crate(&crate_dir, &cache_dir, session_id, cell_id, None, false)
            .await
            .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Failure { stdout, .. } => {
                let diagnostics = parse_diagnostics(&stdout, preamble_lines);
                assert!(
                    !diagnostics.is_empty(),
                    "type error should produce at least one diagnostic",
                );

                let has_type_error = diagnostics.iter().any(|d| {
                    d.message.contains("mismatched types") || d.code.as_deref() == Some("E0308")
                });
                assert!(
                    has_type_error,
                    "diagnostics should include type mismatch error, got: {diagnostics:?}",
                );
            }
            BuildResult::Success { .. } => {
                panic!("expected compilation failure for invalid code, but build succeeded");
            }
        }
    }

    // ── Full pipeline: compile → cache → cache hit ─────────────────────

    #[tokio::test]
    #[ignore = "slow: invokes cargo build --target wasm32-unknown-unknown"]
    async fn compile_and_cache_round_trip() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-cache";
        let cell_id = "cached";
        let source = "    CellOutput::text(\"hello from e2e\")";
        let cargo_toml = "[dependencies]";

        // Step 1: Hash the input (should be a cache miss).
        let hash = content_hash(source, cargo_toml, &[], None, None, false);
        assert!(
            try_cache_hit(&cache_dir, &hash).is_none(),
            "should be a cache miss before compilation",
        );

        // Step 2: Scaffold and build.
        let (crate_dir, ..) = scaffold_micro_crate(
            &cache_dir,
            &cell_path,
            session_id,
            cell_id,
            source,
            cargo_toml,
            &[],
            None,
            None,
        )
        .expect("scaffold should succeed");

        let result = build_micro_crate(&crate_dir, &cache_dir, session_id, cell_id, None, false)
            .await
            .expect("build should not return an infra error");

        let (wasm_bytes, js_glue) = match result {
            BuildResult::Success {
                wasm_path, js_glue, ..
            } => {
                let wasm = std::fs::read(&wasm_path).expect("should read WASM blob");
                (wasm, js_glue)
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!("expected success but got failure.\nstdout: {stdout}\nstderr: {stderr}");
            }
        };

        // Step 3: Store in cache (WASM blob + JS glue).
        store_blob(&cache_dir, &hash, &wasm_bytes, Some(&js_glue), &[])
            .expect("store_blob should succeed");

        // Step 4: Verify cache hit returns identical bytes and JS glue.
        let cached = try_cache_hit(&cache_dir, &hash).expect("should be a cache hit after storing");
        assert_eq!(
            cached.wasm_bytes, wasm_bytes,
            "cached blob should match the compiled blob byte-for-byte",
        );
        assert_eq!(
            cached.js_glue.as_deref(),
            Some(js_glue.as_str()),
            "cached JS glue should match the generated glue",
        );

        // Step 5: Different source should miss the cache.
        let different_hash = content_hash(
            "    CellOutput::text(\"different\")",
            cargo_toml,
            &[],
            None,
            None,
            false,
        );
        assert!(
            try_cache_hit(&cache_dir, &different_hash).is_none(),
            "different source should not hit the cache",
        );
    }

    // ── Sim bus compilation ─────────────────────────────────────────────

    #[tokio::test]
    #[ignore = "slow: invokes cargo build --target wasm32-unknown-unknown"]
    async fn pipeline_simulation_with_sim_bus_compiles() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-simbustest";
        let cell_id = "simbus";

        // A minimal simulation that uses sim::emit and sim::read via sliders.
        let source = r#"
struct BusSim {
    frame: u32,
}

impl Simulation for BusSim {
    fn init() -> Self {
        Self { frame: 0 }
    }

    fn tick(&mut self) -> Canvas {
        let val: f64 = sim::read("test_key").unwrap_or(42.0);
        sim::emit("output_key", &val);
        self.frame += 1;
        Canvas::new(10, 10)
    }

    fn sliders() -> Vec<SimSliderMeta> {
        vec![
            ui::sim_slider("test_key", 0.0, 100.0)
                .default_value(42.0)
                .into_meta(),
        ]
    }
}
"#;
        let cargo_toml = "[dependencies]";

        let (crate_dir, ..) = scaffold_micro_crate(
            &cache_dir,
            &cell_path,
            session_id,
            cell_id,
            source,
            cargo_toml,
            &[],
            None,
            None,
        )
        .expect("scaffold should succeed");

        let result = build_micro_crate(&crate_dir, &cache_dir, session_id, cell_id, None, false)
            .await
            .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success { wasm_path, .. } => {
                assert!(wasm_path.exists(), "WASM blob should exist on disk");
                let wasm_bytes = std::fs::read(&wasm_path).unwrap();
                assert_eq!(
                    &wasm_bytes[..4],
                    b"\x00asm",
                    "WASM blob should start with the WASM magic number",
                );
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!(
                    "sim bus cell should compile successfully but got failure.\nstdout: {stdout}\nstderr: {stderr}"
                );
            }
        }
    }

    // ── LiveView compilation ────────────────────────────────────────────

    #[tokio::test]
    #[ignore = "slow: invokes cargo build --target wasm32-unknown-unknown"]
    async fn pipeline_live_view_compiles() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-liveview";
        let cell_id = "liveview";

        // A minimal LiveView that returns HTML content.
        let source = r#"
struct Counter {
    n: u32,
}

impl LiveView for Counter {
    fn init() -> Self {
        Self { n: 0 }
    }

    fn tick(&mut self) -> LiveContent {
        self.n += 1;
        LiveContent::Html(format!("<p>Tick {}</p>", self.n))
    }

    fn fps() -> u32 {
        5
    }
}
"#;
        let cargo_toml = "[dependencies]";

        let (crate_dir, _preamble, _is_async, is_sim, _) = scaffold_micro_crate(
            &cache_dir,
            &cell_path,
            session_id,
            cell_id,
            source,
            cargo_toml,
            &[],
            None,
            None,
        )
        .expect("scaffold should succeed");

        // LiveView cells report is_simulation=true so the executor knows to
        // export cell_tick.
        assert!(is_sim, "LiveView cells should report is_simulation=true");

        // Verify the generated lib.rs contains LiveView-specific code.
        let lib_rs = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        assert!(
            lib_rs.contains("LiveViewMeta"),
            "generated lib.rs should contain LiveViewMeta"
        );
        assert!(
            lib_rs.contains("LiveTickResult"),
            "generated lib.rs should contain LiveTickResult"
        );
        assert!(
            lib_rs.contains("__IRONPAD_LIVE_VIEW__"),
            "generated lib.rs should use __IRONPAD_LIVE_VIEW__ static"
        );

        let result = build_micro_crate(&crate_dir, &cache_dir, session_id, cell_id, None, false)
            .await
            .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success { wasm_path, .. } => {
                assert!(wasm_path.exists(), "WASM blob should exist on disk");
                let wasm_bytes = std::fs::read(&wasm_path).unwrap();
                assert_eq!(
                    &wasm_bytes[..4],
                    b"\x00asm",
                    "WASM blob should start with the WASM magic number",
                );
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!(
                    "LiveView cell should compile successfully but got failure.\nstdout: {stdout}\nstderr: {stderr}"
                );
            }
        }
    }

    // ── Public notebook compilation ──────────────────────────────────────

    /// Compiles every Code cell in every public notebook to verify they all
    /// type-check against wasm32-unknown-unknown.  Uses `cargo check` (not
    /// `cargo build`) with a shared `CARGO_HOME` and target dir so dependencies
    /// are downloaded/compiled only once.
    #[tokio::test]
    #[ignore = "slow: type-checks every public notebook cell for wasm32-unknown-unknown"]
    async fn all_public_notebook_cells_compile() {
        use ironpad_common::{CellType, IronpadNotebook};

        let notebooks_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../public/notebooks")
            .canonicalize()
            .expect("public/notebooks directory should exist");

        let cell_path = ironpad_cell_path();

        let mut notebook_files: Vec<_> = std::fs::read_dir(&notebooks_dir)
            .expect("should read notebooks directory")
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "ironpad"))
            .map(|e| e.path())
            .collect();
        notebook_files.sort();

        assert!(
            !notebook_files.is_empty(),
            "should find at least one .ironpad file"
        );

        // Shared cache_dir: all cells share one CARGO_HOME (download crates
        // once) and one CARGO_TARGET_DIR (compile deps once).
        let cache_dir = tempdir();
        let session_id = "e2e-notebooks";

        let mut failures: Vec<String> = Vec::new();
        let mut total_cells = 0u32;

        for nb_path in &notebook_files {
            let filename = nb_path.file_name().unwrap().to_string_lossy();
            let nb_stem = nb_path.file_stem().unwrap().to_string_lossy();
            let json = std::fs::read_to_string(nb_path)
                .unwrap_or_else(|e| panic!("failed to read {filename}: {e}"));
            let notebook: IronpadNotebook = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("failed to parse {filename}: {e}"));

            let shared_cargo_toml = notebook.shared_cargo_toml.as_deref();
            let shared_source = notebook.shared_source.as_deref();

            // Sort cells by order so previous_cell_types accumulates correctly.
            let mut cells = notebook.cells.clone();
            cells.sort_by_key(|c| c.order);

            let mut previous_cell_types: Vec<String> = Vec::new();

            for cell in &cells {
                if cell.cell_type != CellType::Code {
                    previous_cell_types.push(String::new());
                    continue;
                }

                // Skip cells that reference prior cell outputs (cell0, cell1,
                // etc.) or the scaffold-injected `last` binding — they can't
                // compile in isolation since we don't know the concrete
                // output types of predecessor cells.
                let uses_cell_ref = (0..10).any(|i| {
                    let pat = format!("cell{i}");
                    cell.source.contains(&pat)
                });
                // Bare `last` (not `.last()`) indicates the scaffold binding.
                let uses_last_binding = cell.source.split_whitespace().any(|tok| {
                    tok == "last" || tok.starts_with("last,") || tok.starts_with("last)")
                });
                if uses_cell_ref || uses_last_binding {
                    previous_cell_types.push(String::new());
                    continue;
                }

                total_cells += 1;
                let cell_cargo = cell.cargo_toml.as_deref().unwrap_or("[dependencies]");

                // Prefix cell_id with notebook stem to avoid collisions
                // (many notebooks use cell_0, cell_1, etc.).
                let unique_id = format!("{nb_stem}__{}", cell.id);

                let scaffold_result = scaffold_micro_crate(
                    &cache_dir,
                    &cell_path,
                    session_id,
                    &unique_id,
                    &cell.source,
                    cell_cargo,
                    &previous_cell_types,
                    shared_cargo_toml,
                    shared_source,
                );

                let (crate_dir, ..) = match scaffold_result {
                    Ok(r) => r,
                    Err(e) => {
                        failures.push(format!(
                            "{filename} / {} ({}): scaffold error: {e}",
                            cell.id, cell.label
                        ));
                        previous_cell_types.push(String::new());
                        continue;
                    }
                };

                let result =
                    check_micro_crate(&crate_dir, &cache_dir, session_id, &unique_id, None, false)
                        .await;

                match result {
                    Ok(CheckResult::Ok) => {
                        previous_cell_types.push(String::new());
                    }
                    Ok(CheckResult::Failure { stdout, stderr }) => {
                        failures.push(format!(
                            "{filename} / {} ({}):\n  stdout: {}\n  stderr: {}",
                            cell.id,
                            cell.label,
                            stdout.chars().take(500).collect::<String>(),
                            stderr.chars().take(500).collect::<String>(),
                        ));
                        previous_cell_types.push(String::new());
                    }
                    Err(e) => {
                        failures.push(format!(
                            "{filename} / {} ({}): check infra error: {e}",
                            cell.id, cell.label
                        ));
                        previous_cell_types.push(String::new());
                    }
                }
            }
        }

        assert!(
            failures.is_empty(),
            "\n{} of {total_cells} notebook cells failed to compile:\n\n{}\n",
            failures.len(),
            failures.join("\n\n"),
        );

        // Sanity: we should have compiled a meaningful number of cells.
        assert!(
            total_cells >= 20,
            "expected at least 20 standalone code cells across all notebooks, got {total_cells}"
        );
    }
}
