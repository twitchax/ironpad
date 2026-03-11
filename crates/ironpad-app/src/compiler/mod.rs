//! Server-side compilation pipeline.

pub mod build;
pub mod cache;
pub mod diagnostics;
pub mod optimize;
pub mod scaffold;

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

        let (crate_dir, _preamble_lines, _is_async) = scaffold_micro_crate(
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
        let hash_a = content_hash(source, cargo_toml, &[], None, None);
        let hash_b = content_hash(source, cargo_toml, &[], None, None);
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
        let hash_v1 = content_hash("    let x = 1;", cargo, &[], None, None);
        let hash_v2 = content_hash("    let x = 2;", cargo, &[], None, None);
        assert_ne!(
            hash_v1, hash_v2,
            "different source must produce different hashes"
        );
    }

    #[test]
    fn changed_cargo_toml_invalidates_hash() {
        let source = "    CellOutput::empty()";
        let hash_a = content_hash(source, "[dependencies]\nserde = \"1\"", &[], None, None);
        let hash_b = content_hash(source, "[dependencies]\nrand = \"0.8\"", &[], None, None);
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

        let (dir, preamble_lines, _) =
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
        let hash = content_hash(source, cargo_toml, &[], None, None);
        assert_eq!(hash.len(), 64, "blake3 hash should be 64 hex chars");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash should be hex"
        );

        // Step 2: Scaffold the micro-crate.
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let (crate_dir, preamble_lines, _) = scaffold_micro_crate(
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
        let mock_cargo_output = r#"{"reason":"compiler-message","package_id":"cell-cell-0 0.1.0 (path+file:///tmp/cell)","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-cell-0","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error[E0308]: mismatched types\n","children":[],"code":{"code":"E0308","explanation":null},"level":"error","message":"mismatched types","spans":[{"byte_end":200,"byte_start":190,"column_end":30,"column_start":22,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"expected `i32`, found `&str`","line_end":6,"line_start":6,"suggested_replacement":null,"suggestion_applicability":null,"text":[]}]}}"#;

        let diagnostics = parse_diagnostics(mock_cargo_output, preamble_lines);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, ironpad_common::Severity::Error);
        assert_eq!(diagnostics[0].code.as_deref(), Some("E0308"));

        // The error was on wrapper line 6 → user line 1.
        assert_eq!(diagnostics[0].spans[0].line_start, 1);
    }

    // ── Typed Scaffold Generation ──────────────────────────────────────

    #[test]
    fn scaffold_generates_typed_cell_bindings() {
        // With two typed previous cells.
        let types: Vec<Option<String>> = vec![Some("u32".into()), Some("String".into())];
        let (code, preamble, _) = generate_lib_rs("    let x = cell0 + 1;", &types, false);

        assert!(code.contains("let cell0: u32"));
        assert!(code.contains("let cell1: String"));
        assert!(code.contains("let last = &cell1"));
        assert!(code.contains("__ironpad_inputs__"));
        assert_eq!(preamble, 11, "5 base + 3 (ptr + inputs) + 2 cells + 1 last");

        // With no previous cells.
        let (code_empty, preamble_empty, _) = generate_lib_rs("    let x = 1;", &[], false);

        assert!(!code_empty.contains("__ironpad_inputs__"));
        assert!(!code_empty.contains("let cell"));
        assert_eq!(preamble_empty, 5);
    }

    // ── Cache round-trip with content hash ──────────────────────────────

    #[test]
    fn cache_round_trip_with_pipeline_hash() {
        use super::cache::{store_blob, try_cache_hit};

        let source = "    CellOutput::text(\"cached\")";
        let cargo = "[dependencies]";
        let hash = content_hash(source, cargo, &[], None, None);

        let cache_dir = tempdir();
        let fake_wasm = b"\x00asm\x01\x00\x00\x00fake-wasm-bytes";
        let fake_js_glue = "export function init() {}";

        // Cache miss before storing.
        assert!(try_cache_hit(&cache_dir, &hash).is_none());

        // Store and verify cache hit.
        store_blob(&cache_dir, &hash, fake_wasm, Some(fake_js_glue)).unwrap();
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

    use super::build::{build_micro_crate, BuildResult};
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
    #[ignore] // Slow: invokes `cargo build --target wasm32-unknown-unknown`.
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
        let result = build_micro_crate(&crate_dir, &cache_dir, session_id, cell_id)
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

    // ── Compilation failure produces diagnostics ────────────────────────

    #[tokio::test]
    #[ignore] // Slow: invokes `cargo build --target wasm32-unknown-unknown`.
    async fn compile_bad_code_returns_diagnostics() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-session";
        let cell_id = "badcode";
        // Deliberately broken: assigning a string to i32.
        let source = "    let x: i32 = \"oops\";\n    CellOutput::empty()";
        let cargo_toml = "[dependencies]";

        let (crate_dir, preamble_lines, _) = scaffold_micro_crate(
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

        let result = build_micro_crate(&crate_dir, &cache_dir, session_id, cell_id)
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
    #[ignore] // Slow: invokes `cargo build --target wasm32-unknown-unknown`.
    async fn compile_and_cache_round_trip() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-cache";
        let cell_id = "cached";
        let source = "    CellOutput::text(\"hello from e2e\")";
        let cargo_toml = "[dependencies]";

        // Step 1: Hash the input (should be a cache miss).
        let hash = content_hash(source, cargo_toml, &[], None, None);
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

        let result = build_micro_crate(&crate_dir, &cache_dir, session_id, cell_id)
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
        store_blob(&cache_dir, &hash, &wasm_bytes, Some(&js_glue))
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
        );
        assert!(
            try_cache_hit(&cache_dir, &different_hash).is_none(),
            "different source should not hit the cache",
        );
    }
}
