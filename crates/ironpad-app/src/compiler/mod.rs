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
    use super::scaffold::{scaffold_micro_crate, WRAPPER_PREAMBLE_LINES};

    // ── Scaffolding → Content Verification ──────────────────────────────

    #[test]
    fn scaffolded_crate_has_valid_cargo_toml_and_lib_rs() {
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad/crates/ironpad-cell");
        let source = "    CellOutput::text(\"hello\").into()";
        let cargo_toml = "[dependencies]\nserde = \"1\"";

        let crate_dir =
            scaffold_micro_crate(&tmp, &cell_path, "sess-1", "cell-0", source, cargo_toml)
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
            lib.contains("pub extern \"C\" fn cell_main("),
            "missing cell_main FFI entry"
        );
        assert!(
            lib.contains("CellOutput::text(\"hello\")"),
            "user code not embedded"
        );
    }

    // ── Hashing → Scaffold → Consistency ────────────────────────────────

    #[test]
    fn identical_inputs_produce_same_hash_and_scaffold_content() {
        let source = "    let x = 42;\n    CellOutput::text(&x.to_string()).into()";
        let cargo_toml = "[dependencies]\nrand = \"0.8\"";

        // Hashes must match.
        let hash_a = content_hash(source, cargo_toml);
        let hash_b = content_hash(source, cargo_toml);
        assert_eq!(hash_a, hash_b, "same inputs must produce identical hashes");

        // Scaffolded content must be identical for the same inputs.
        let tmp_a = tempdir();
        let tmp_b = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");

        let dir_a = scaffold_micro_crate(&tmp_a, &cell_path, "s", "c", source, cargo_toml).unwrap();
        let dir_b = scaffold_micro_crate(&tmp_b, &cell_path, "s", "c", source, cargo_toml).unwrap();

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
        let hash_v1 = content_hash("    let x = 1;", cargo);
        let hash_v2 = content_hash("    let x = 2;", cargo);
        assert_ne!(
            hash_v1, hash_v2,
            "different source must produce different hashes"
        );
    }

    #[test]
    fn changed_cargo_toml_invalidates_hash() {
        let source = "    CellOutput::empty().into()";
        let hash_a = content_hash(source, "[dependencies]\nserde = \"1\"");
        let hash_b = content_hash(source, "[dependencies]\nrand = \"0.8\"");
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

        let dir = scaffold_micro_crate(&tmp, &cell_path, "s", "c", user_code, "").unwrap();
        let lib = std::fs::read_to_string(dir.join("src/lib.rs")).unwrap();

        // User code must start at exactly line WRAPPER_PREAMBLE_LINES + 1 (1-indexed).
        let lines: Vec<&str> = lib.lines().collect();
        assert_eq!(
            lines[WRAPPER_PREAMBLE_LINES as usize].trim(),
            "let x = 42;",
            "user code should appear at line index {} (1-indexed line {})",
            WRAPPER_PREAMBLE_LINES,
            WRAPPER_PREAMBLE_LINES + 1,
        );
    }

    #[test]
    fn diagnostic_spans_correctly_map_to_user_code_lines() {
        // Simulate a type error on wrapper line 6 (user line 2 given preamble of 4).
        let json = r#"{"reason":"compiler-message","package_id":"cell-test 0.1.0 (path+file:///tmp/cell)","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-test","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error[E0308]: mismatched types\n","children":[],"code":{"code":"E0308","explanation":"Expected type did not match the received type."},"level":"error","message":"mismatched types","spans":[{"byte_end":200,"byte_start":190,"column_end":15,"column_start":5,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"expected `i32`, found `&str`","line_end":6,"line_start":6,"suggested_replacement":null,"suggestion_applicability":null,"text":[{"highlight_end":15,"highlight_start":5,"text":"    \"hello\""}]}]}}"#;

        let diags = parse_diagnostics(json);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].spans.len(), 1);

        let span = &diags[0].spans[0];
        // Wrapper line 6 - WRAPPER_PREAMBLE_LINES(4) = user line 2.
        assert_eq!(span.line_start, 6 - WRAPPER_PREAMBLE_LINES);
        assert_eq!(span.line_end, 6 - WRAPPER_PREAMBLE_LINES);
    }

    // ── Full pipeline flow (minus build) ────────────────────────────────

    #[test]
    fn pipeline_hash_scaffold_diagnostics_round_trip() {
        let source = "    let x: i32 = \"oops\";\n    CellOutput::empty().into()";
        let cargo_toml = "[dependencies]";

        // Step 1: Hash the input.
        let hash = content_hash(source, cargo_toml);
        assert_eq!(hash.len(), 64, "blake3 hash should be 64 hex chars");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash should be hex"
        );

        // Step 2: Scaffold the micro-crate.
        let tmp = tempdir();
        let cell_path = PathBuf::from("/opt/ironpad-cell");
        let crate_dir =
            scaffold_micro_crate(&tmp, &cell_path, "session", "cell-0", source, cargo_toml)
                .unwrap();

        assert!(crate_dir.join("Cargo.toml").is_file());
        assert!(crate_dir.join("src/lib.rs").is_file());

        // Step 3: Verify scaffolded lib.rs contains user code at the right offset.
        let lib = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        let lines: Vec<&str> = lib.lines().collect();
        assert!(
            lines[WRAPPER_PREAMBLE_LINES as usize].contains("let x: i32"),
            "first line of user code should be at preamble offset"
        );

        // Step 4: Parse mock build output (simulated cargo JSON diagnostics).
        let mock_cargo_output = r#"{"reason":"compiler-message","package_id":"cell-cell-0 0.1.0 (path+file:///tmp/cell)","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-cell-0","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error[E0308]: mismatched types\n","children":[],"code":{"code":"E0308","explanation":null},"level":"error","message":"mismatched types","spans":[{"byte_end":200,"byte_start":190,"column_end":30,"column_start":22,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"expected `i32`, found `&str`","line_end":5,"line_start":5,"suggested_replacement":null,"suggestion_applicability":null,"text":[]}]}}"#;

        let diagnostics = parse_diagnostics(mock_cargo_output);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, ironpad_common::Severity::Error);
        assert_eq!(diagnostics[0].code.as_deref(), Some("E0308"));

        // The error was on wrapper line 5 → user line 1.
        assert_eq!(diagnostics[0].spans[0].line_start, 1);
    }

    // ── Cache round-trip with content hash ──────────────────────────────

    #[test]
    fn cache_round_trip_with_pipeline_hash() {
        use super::cache::{store_blob, try_cache_hit};

        let source = "    CellOutput::text(\"cached\").into()";
        let cargo = "[dependencies]";
        let hash = content_hash(source, cargo);

        let cache_dir = tempdir();
        let fake_wasm = b"\x00asm\x01\x00\x00\x00fake-wasm-bytes";

        // Cache miss before storing.
        assert!(try_cache_hit(&cache_dir, &hash).is_none());

        // Store and verify cache hit.
        store_blob(&cache_dir, &hash, fake_wasm).unwrap();
        let hit = try_cache_hit(&cache_dir, &hash).expect("should be a cache hit");
        assert_eq!(hit, fake_wasm);

        // A different source must not hit the same cache entry.
        let different_hash = content_hash("    CellOutput::text(\"different\").into()", cargo);
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
