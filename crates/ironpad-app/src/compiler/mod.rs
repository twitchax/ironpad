//! Server-side compilation pipeline.

pub mod admission;
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
/// requests run one at a time. Cloneable; all clones share one lock table.
///
/// The lock keys on the `-`→`_`-NORMALIZED id, not the raw id, because cargo
/// derives one lib target name (`cell_a_b`) for both `cell-a-b` and
/// `cell-a_b`, so two ids differing only by `-`/`_` uplift to the SAME shared
/// artifact (`targets/default/.../cell_a_b.wasm`). Keying on the raw id let
/// those two run concurrently and one build's WASM be read + cached under the
/// other's content hash. Normalizing to the artifact identity makes them
/// contend, which is exactly what the shared output requires.
#[derive(Clone, Default)]
pub struct CompileLocks {
    locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

/// The lock identity for `cell_id`: the same `-`→`_` normalization cargo
/// applies to derive the uplifted lib artifact name (`expected_wasm_path`).
fn lock_key(cell_id: &str) -> String {
    cell_id.replace('-', "_")
}

impl CompileLocks {
    /// Acquire the lock for `cell_id`, awaiting any in-flight compile that
    /// shares its artifact identity. The returned guard must be held for the
    /// whole compile.
    pub async fn acquire(&self, cell_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let key = lock_key(cell_id);
        let cell_lock = {
            let mut table = self.locks.lock().expect("compile-lock table poisoned");
            // Prune entries no longer in use so the table can't grow without
            // bound across many distinct cell ids. An entry with
            // `strong_count == 1` is held only by the table (no in-flight
            // compile holds a guard clone), so it's safe to drop; keep the id
            // we're about to (re)acquire regardless. This runs under the table
            // lock, so a concurrent acquirer for a pruned id simply recreates a
            // fresh lock — mutual exclusion is preserved because a count of 1
            // means no compile is currently inside the critical section.
            table.retain(|id, lock| *id == key || Arc::strong_count(lock) > 1);
            table.entry(key).or_default().clone()
        };
        cell_lock.lock_owned().await
    }

    /// Acquire the lock for `cell_id` only if no compile/check currently
    /// holds it. Returns `None` when busy — live checks use this to SKIP
    /// rather than queue behind a long build (PRD-0045).
    pub fn try_acquire(&self, cell_id: &str) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        let key = lock_key(cell_id);
        let cell_lock = {
            let mut table = self.locks.lock().expect("compile-lock table poisoned");
            table.retain(|id, lock| *id == key || Arc::strong_count(lock) > 1);
            table.entry(key).or_default().clone()
        };
        cell_lock.try_lock_owned().ok()
    }

    /// Number of entries currently in the lock table (test-only introspection).
    #[cfg(test)]
    fn table_len(&self) -> usize {
        self.locks
            .lock()
            .expect("compile-lock table poisoned")
            .len()
    }
}

#[cfg(test)]
mod compile_locks_tests {
    use super::CompileLocks;
    use std::time::Duration;

    #[tokio::test]
    async fn try_acquire_skips_when_busy_and_succeeds_when_free() {
        // Live checks must SKIP a busy cell (never queue behind a build),
        // and must themselves exclude a concurrent build once held.
        let locks = CompileLocks::default();

        let build_guard = locks.acquire("cell-1").await;
        assert!(
            locks.try_acquire("cell-1").is_none(),
            "check must skip while a build holds the lock"
        );
        drop(build_guard);

        let check_guard = locks.try_acquire("cell-1");
        assert!(check_guard.is_some(), "free lock must be acquirable");
        let blocked =
            tokio::time::timeout(Duration::from_millis(50), locks.acquire("cell-1")).await;
        assert!(
            blocked.is_err(),
            "build must wait while a check holds the lock"
        );
        drop(check_guard);
    }

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

    #[tokio::test]
    async fn dash_and_underscore_variants_share_one_lock() {
        // `a-b` and `a_b` uplift to the SAME cargo lib artifact, so they must
        // contend — otherwise one build's WASM lands under the other's hash.
        let locks = CompileLocks::default();
        let guard = locks.acquire("cell-a-b").await;
        assert!(
            locks.try_acquire("cell-a_b").is_none(),
            "the `_` variant must block on the `-` variant's lock"
        );
        let blocked =
            tokio::time::timeout(Duration::from_millis(50), locks.acquire("cell-a_b")).await;
        assert!(blocked.is_err(), "same-artifact ids must serialize");
        drop(guard);
        assert_eq!(locks.table_len(), 1, "both variants map to one entry");
    }

    #[tokio::test]
    async fn idle_locks_are_pruned() {
        let locks = CompileLocks::default();

        // A completed compile leaves its entry idle (guard dropped).
        drop(locks.acquire("cell-1").await);
        assert_eq!(locks.table_len(), 1, "cell-1 entry present after use");

        // Acquiring a different cell prunes the now-idle cell-1 entry; only the
        // in-use cell-2 entry remains.
        let _guard = locks.acquire("cell-2").await;
        assert_eq!(
            locks.table_len(),
            1,
            "idle cell-1 should be pruned, leaving only in-use cell-2"
        );
    }

    #[tokio::test]
    async fn in_use_locks_are_not_pruned() {
        let locks = CompileLocks::default();

        // Hold cell-1's guard while acquiring cell-2 — cell-1 is in use and must
        // survive the prune.
        let _held = locks.acquire("cell-1").await;
        let _other = locks.acquire("cell-2").await;
        assert_eq!(
            locks.table_len(),
            2,
            "an in-flight compile's lock must not be pruned"
        );
    }
}

// ── Cross-module pipeline integration tests ─────────────────────────────────

#[cfg(test)]
mod pipeline_tests {
    use std::path::PathBuf;

    use ironpad_common::cache_key::CellTarget;

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

        let (crate_dir, _preamble_lines, _is_async, _) = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "sess-1",
            "cell-0",
            source,
            cargo_toml,
            &[],
            None,
            None,
            CellTarget::Executor,
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
        let hash_a = content_hash(
            source,
            cargo_toml,
            &[],
            None,
            None,
            false,
            false,
            false,
            CellTarget::Executor,
        );
        let hash_b = content_hash(
            source,
            cargo_toml,
            &[],
            None,
            None,
            false,
            false,
            false,
            CellTarget::Executor,
        );
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
            CellTarget::Executor,
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
            CellTarget::Executor,
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
        let hash_v1 = content_hash(
            "    let x = 1;",
            cargo,
            &[],
            None,
            None,
            false,
            false,
            false,
            CellTarget::Executor,
        );
        let hash_v2 = content_hash(
            "    let x = 2;",
            cargo,
            &[],
            None,
            None,
            false,
            false,
            false,
            CellTarget::Executor,
        );
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
            false,
            false,
            CellTarget::Executor,
        );
        let hash_b = content_hash(
            source,
            "[dependencies]\nrand = \"0.8\"",
            &[],
            None,
            None,
            false,
            false,
            false,
            CellTarget::Executor,
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

        let (dir, preamble_lines, ..) = scaffold_micro_crate(
            &tmp,
            &cell_path,
            "s",
            "c",
            user_code,
            "",
            &[],
            None,
            None,
            CellTarget::Executor,
        )
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
        let hash = content_hash(
            source,
            cargo_toml,
            &[],
            None,
            None,
            false,
            false,
            false,
            CellTarget::Executor,
        );
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
            CellTarget::Executor,
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
        let mock_cargo_output = r#"{"reason":"compiler-message","package_id":"cell-cell-0 0.1.0 (path+file:///tmp/cell)","manifest_path":"/tmp/cell/Cargo.toml","target":{"kind":["cdylib"],"crate_types":["cdylib"],"name":"cell-cell-0","src_path":"/tmp/cell/src/lib.rs","edition":"2021","doc":false,"doctest":false,"test":false},"message":{"rendered":"error[E0308]: mismatched types\n","children":[],"code":{"code":"E0308","explanation":null},"level":"error","message":"mismatched types","spans":[{"byte_end":200,"byte_start":190,"column_end":30,"column_start":22,"expansion":null,"file_name":"src/lib.rs","is_primary":true,"label":"expected `i32`, found `&str`","line_end":8,"line_start":8,"suggested_replacement":null,"suggestion_applicability":null,"text":[]}]}}"#;

        let diagnostics = parse_diagnostics(mock_cargo_output, preamble_lines);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, ironpad_common::Severity::Error);
        assert_eq!(diagnostics[0].code.as_deref(), Some("E0308"));

        // The error was on wrapper line 8 → user line 1 (preamble is 7).
        assert_eq!(diagnostics[0].spans[0].line_start, 1);
    }

    // ── Typed Scaffold Generation ──────────────────────────────────────

    #[test]
    fn scaffold_generates_typed_cell_bindings() {
        // Two typed previous cells, only cell0 referenced (PRD-0060): the
        // scaffold binds exactly the referenced slot.
        let types: Vec<String> = vec!["u32".into(), "String".into()];
        let (code, preamble, ..) = generate_lib_rs("    let x = cell0 + 1;", &types, false);

        assert!(code.contains("let cell0: u32"));
        assert!(
            !code.contains("let cell1:"),
            "unreferenced slots stay unbound"
        );
        assert!(
            !code.contains("let last"),
            "the alias binds only when referenced"
        );
        assert!(code.contains("__ironpad_inputs__"));
        assert_eq!(
            preamble, 11,
            "7 base + 3 (ptr + inputs) + 1 referenced cell"
        );

        // With no previous cells.
        let (code_empty, preamble_empty, ..) = generate_lib_rs("    let x = 1;", &[], false);

        assert!(!code_empty.contains("__ironpad_inputs__"));
        assert!(!code_empty.contains("let cell"));
        assert_eq!(preamble_empty, 7);
    }

    // ── Cache round-trip with content hash ──────────────────────────────

    #[test]
    fn cache_round_trip_with_pipeline_hash() {
        use super::cache::{store_blob, try_cache_hit};

        let source = "    CellOutput::text(\"cached\")";
        let cargo = "[dependencies]";
        let hash = content_hash(
            source,
            cargo,
            &[],
            None,
            None,
            false,
            false,
            false,
            CellTarget::Executor,
        );

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
            false,
            false,
            CellTarget::Executor,
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

    use ironpad_common::cache_key::CellTarget;

    use super::build::{build_micro_crate, check_micro_crate, BuildResult, CheckResult};
    use super::cache::{content_hash, store_blob, try_cache_hit};
    use super::diagnostics::parse_diagnostics;
    use super::scaffold::{
        merged_deps_contain_rayon, scaffold_micro_crate, uses_std_autodiff, uses_wasm_simd,
    };

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

    /// The names in a WASM module's export section (section id 7).
    ///
    /// Hand-rolled rather than adding a parser dependency for one assertion:
    /// this is the section table plus one LEB128-length-prefixed vector. A
    /// substring scan of the whole binary would be a weaker claim — the name
    /// section and the string data carry `_start` too.
    fn wasm_export_names(bytes: &[u8]) -> Vec<String> {
        fn leb128(bytes: &[u8], cursor: &mut usize) -> usize {
            let (mut value, mut shift) = (0usize, 0u32);
            loop {
                let byte = bytes[*cursor];
                *cursor += 1;
                value |= usize::from(byte & 0x7f) << shift;
                if byte & 0x80 == 0 {
                    return value;
                }
                shift += 7;
            }
        }

        assert_eq!(&bytes[..4], b"\0asm", "not a wasm module");
        let mut cursor = 8; // magic + version
        while cursor < bytes.len() {
            let section_id = bytes[cursor];
            cursor += 1;
            let section_len = leb128(bytes, &mut cursor);
            let section_end = cursor + section_len;
            if section_id != 7 {
                cursor = section_end;
                continue;
            }
            let count = leb128(bytes, &mut cursor);
            let mut names = Vec::with_capacity(count);
            for _ in 0..count {
                let name_len = leb128(bytes, &mut cursor);
                names.push(String::from_utf8_lossy(&bytes[cursor..cursor + name_len]).into_owned());
                cursor += name_len;
                cursor += 1; // export kind
                leb128(bytes, &mut cursor); // export index
            }
            return names;
        }
        Vec::new()
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
            CellTarget::Executor,
        )
        .expect("scaffold should succeed");

        assert!(crate_dir.join("Cargo.toml").is_file());
        assert!(crate_dir.join("src/lib.rs").is_file());

        // Build to WASM.
        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Executor,
            false,
            false,
            false,
        )
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
                let js_glue = js_glue.expect("an ordinary cell carries wasm-bindgen glue");
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
    /// PRD-0041 regression: a cell using the real `std::autodiff` compiles
    /// through the actual pipeline — nightly toolchain, `-Zautodiff=Enable`,
    /// the scaffold's crate-root feature gate + fat-LTO profile, and Enzyme's
    /// tape allocations resolving against ironpad-cell's wasm allocator shims
    /// (which live in a DEPENDENCY crate here, unlike the feasibility probe).
    /// The differentiated function loops with a data-dependent trip count, so
    /// Enzyme must exercise its tape (malloc/free/realloc).
    #[tokio::test]
    #[ignore = "slow: full cargo build of a micro-crate (nightly + Enzyme)"]
    async fn compile_cell_with_std_autodiff_builds_successfully() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-session";
        let cell_id = "std-autodiff";

        let shared_source = r"use std::autodiff::autodiff_reverse;

/// Projectile range under quadratic drag: a timestepped simulation with a
/// data-dependent loop, so the reverse-mode gradient needs Enzyme's tape.
#[autodiff_reverse(d_range, Active, Active)]
pub fn range(angle: f64) -> f64 {
    let (mut x, mut y) = (0.0_f64, 0.0_f64);
    let v = 50.0_f64;
    let (mut vx, mut vy) = (v * angle.cos(), v * angle.sin());
    let (dt, drag, g) = (0.01_f64, 0.002_f64, 9.81_f64);
    while y >= 0.0 {
        let speed = (vx * vx + vy * vy).sqrt();
        vx -= drag * speed * vx * dt;
        vy -= (g + drag * speed * vy) * dt;
        x += vx * dt;
        y += vy * dt;
    }
    x
}
";
        let source = "    let (_r, grad) = shared::d_range(0.6, 1.0);\n    CellOutput::from(grad)";
        let cargo_toml = "[dependencies]";

        let (crate_dir, preamble, ..) = scaffold_micro_crate(
            &cache_dir,
            &cell_path,
            session_id,
            cell_id,
            source,
            cargo_toml,
            &[],
            None,
            Some(shared_source),
            CellTarget::Executor,
        )
        .expect("scaffold should succeed");

        // The scaffold must have opted in: feature gate on line 1.
        let lib_rs = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        assert!(lib_rs.starts_with("#![feature(autodiff)]\n"));
        assert!(preamble >= 1);

        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Executor,
            false,
            true,
            false,
        )
        .await
        .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success { wasm_path, .. } => {
                let wasm_bytes = std::fs::read(&wasm_path).unwrap();
                assert_eq!(&wasm_bytes[..4], b"\x00asm");
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!(
                    "std::autodiff cell should build.\nstdout(tail): {}\nstderr(tail): {}",
                    &stdout[stdout.len().saturating_sub(2000)..],
                    &stderr[stderr.len().saturating_sub(1500)..],
                );
            }
        }
    }

    #[tokio::test]
    #[ignore = "slow: invokes cargo build --target wasm32-unknown-unknown"]
    async fn compile_cell_with_blocking_imports_links_successfully() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-session";
        let cell_id = "jspi-blocking";
        // References all four `ironpad_blocking_*` host imports through the
        // public API (sleep + fetch), so the build must resolve them at link
        // time under `wasm_import_module = "env"` (the PRD-0031 lesson,
        // extended to PRD-0043).
        let source = "    blocking::sleep_ms(1.0);\n    let body = blocking::fetch_text(\"/notebooks/welcome.ironpad\");\n    CellOutput::from(body.map_or_else(|e| e, |b| b))";
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
            CellTarget::Executor,
        )
        .expect("scaffold should succeed");

        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Executor,
            false,
            false,
            false,
        )
        .await
        .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success { wasm_path, .. } => {
                let wasm_bytes = std::fs::read(&wasm_path).unwrap();
                assert_eq!(&wasm_bytes[..4], b"\x00asm");
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!(
                    "blocking (JSPI) cell should build and link.\nstdout(tail): {}\nstderr(tail): {}",
                    &stdout[stdout.len().saturating_sub(2000)..],
                    &stderr[stderr.len().saturating_sub(1500)..],
                );
            }
        }
    }

    #[tokio::test]
    #[ignore = "slow: invokes cargo build --target wasm32-unknown-unknown"]
    async fn compile_cell_with_portable_simd_builds_successfully() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-session";
        let cell_id = "portable-simd";

        // Mixes portable SIMD (`std::simd`, needs the injected feature gate)
        // with a raw `std::arch::wasm32` intrinsic. The intrinsic is the
        // stronger probe: calling it from a plain function only compiles when
        // `-C target-feature=+simd128` actually reached rustc, so a build
        // success proves the flag plumbing end to end.
        let source = "    use std::simd::prelude::*;\n    use std::arch::wasm32::{f32x4_extract_lane, f32x4_splat};\n    let v = f32x4::from_array([1.0, 2.0, 3.0, 4.0]) * f32x4::splat(2.0);\n    let intrinsic = f32x4_extract_lane::<0>(f32x4_splat(21.0)) * 2.0;\n    CellOutput::from(f64::from(v.reduce_sum() + intrinsic))";
        let cargo_toml = "[dependencies]";

        let (crate_dir, preamble, ..) = scaffold_micro_crate(
            &cache_dir,
            &cell_path,
            session_id,
            cell_id,
            source,
            cargo_toml,
            &[],
            None,
            None,
            CellTarget::Executor,
        )
        .expect("scaffold should succeed");

        // The scaffold must have opted in: feature gate on line 1.
        let lib_rs = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        assert!(lib_rs.starts_with("#![feature(portable_simd)]\n"));
        assert!(preamble >= 1);

        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Executor,
            false,
            false,
            true,
        )
        .await
        .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success { wasm_path, .. } => {
                let wasm_bytes = std::fs::read(&wasm_path).unwrap();
                assert_eq!(&wasm_bytes[..4], b"\x00asm");
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!(
                    "portable SIMD cell should build.\nstdout(tail): {}\nstderr(tail): {}",
                    &stdout[stdout.len().saturating_sub(2000)..],
                    &stderr[stderr.len().saturating_sub(1500)..],
                );
            }
        }
    }

    #[tokio::test]
    #[ignore = "slow: invokes cargo build --target wasm32-unknown-unknown"]
    async fn compile_cell_with_gen_block_builds_successfully() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-session";
        let cell_id = "gen-block";

        // `gen` blocks are edition-2024 syntax: this test would fail to
        // PARSE on the 2021 micro-crates the scaffold used to generate,
        // which is exactly how the v0.17.0 gen_blocks gate shipped inert —
        // its unit tests covered detection and preamble math, never a real
        // compile. A build success proves edition + feature gate together.
        let source = "    let g = gen { yield 1u32; yield 2; yield 3; };\n    let total: u32 = g.sum();\n    CellOutput::from(total)";
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
            CellTarget::Executor,
        )
        .expect("scaffold should succeed");

        let lib_rs = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
        assert!(lib_rs.starts_with("#![feature(gen_blocks)]\n"));
        let manifest = std::fs::read_to_string(crate_dir.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("edition = \"2024\""));

        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Executor,
            false,
            false,
            false,
        )
        .await
        .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success { wasm_path, .. } => {
                let wasm_bytes = std::fs::read(&wasm_path).unwrap();
                assert_eq!(&wasm_bytes[..4], b"\x00asm");
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!(
                    "gen-block cell should build on edition 2024.\nstdout(tail): {}\nstderr(tail): {}",
                    &stdout[stdout.len().saturating_sub(2000)..],
                    &stderr[stderr.len().saturating_sub(1500)..],
                );
            }
        }
    }

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
            CellTarget::Executor,
        )
        .expect("scaffold should succeed");

        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Executor,
            false,
            false,
            false,
        )
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
            CellTarget::Executor,
        )
        .expect("scaffold should succeed");

        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Executor,
            false,
            false,
            false,
        )
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

    /// The shared module is the notebook's library, but rustc only ever compiles
    /// ONE cell's crate at a time. Every shared helper that *this* cell does not
    /// call therefore looked like `dead_code` ("never used") even though a
    /// sibling cell uses it — a false positive on every cell of a notebook with
    /// shared helpers. The scaffold allows `dead_code` on the shared mod, so a
    /// clean build must carry no such warning.
    #[tokio::test]
    #[ignore = "slow: invokes cargo build --target wasm32-unknown-unknown"]
    async fn shared_helpers_unused_by_this_cell_produce_no_dead_code_warnings() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-session";
        let cell_id = "shared-dead-code";

        // Only `used_here` is called below; the rest stand in for helpers a
        // sibling cell would use.
        let shared_source = r"pub fn used_here() -> i32 { 41 }

pub fn used_by_another_cell() -> i32 { 99 }

pub const UNUSED_BY_THIS_CELL: i32 = 7;

pub struct AlsoUnusedHere {
    pub field: i32,
}
";
        let source = "    let _ = shared::used_here();\n    CellOutput::empty()";
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
            Some(shared_source),
            CellTarget::Executor,
        )
        .expect("scaffold should succeed");

        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Executor,
            false,
            false,
            false,
        )
        .await
        .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success { stdout, .. } => {
                let diagnostics = parse_diagnostics(&stdout, preamble_lines);
                let dead_code: Vec<_> = diagnostics
                    .iter()
                    .filter(|d| {
                        d.message.contains("never used")
                            || d.message.contains("never read")
                            || d.message.contains("never constructed")
                    })
                    .collect();
                assert!(
                    dead_code.is_empty(),
                    "shared helpers used by OTHER cells must not warn here, got: {dead_code:?}",
                );
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!(
                    "cell calling a shared helper should build.\nstdout(tail): {}\nstderr(tail): {}",
                    &stdout[stdout.len().saturating_sub(2000)..],
                    &stderr[stderr.len().saturating_sub(1500)..],
                );
            }
        }
    }

    /// Every typed upstream output is bound into the cell as `cellN` (plus
    /// `last`) whether or not the cell reads it, so a cell that uses only some
    /// (or none) of its inputs sprayed "unused variable: `cellN`" warnings.
    /// The scaffold allows `unused_variables` on the injected bindings, so a
    /// clean build must carry no such warning — while unused variables in the
    /// user's own code still warn.
    #[tokio::test]
    #[ignore = "slow: invokes cargo build --target wasm32-unknown-unknown"]
    async fn unused_injected_input_bindings_produce_no_warnings() {
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-session";
        let cell_id = "unused-inputs";

        // Two typed upstream cells; the source references `last`, which
        // binds BOTH cellN slots (the alias target is dynamic) while using
        // neither by name — without the allow, cell0 and cell1 would warn.
        let previous_cell_types: Vec<String> = vec!["u32".into(), "String".into()];
        let source = "    CellOutput::text(format!(\"{last}\"))";
        let cargo_toml = "[dependencies]";

        let (crate_dir, preamble_lines, ..) = scaffold_micro_crate(
            &cache_dir,
            &cell_path,
            session_id,
            cell_id,
            source,
            cargo_toml,
            &previous_cell_types,
            None,
            None,
            CellTarget::Executor,
        )
        .expect("scaffold should succeed");

        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Executor,
            false,
            false,
            false,
        )
        .await
        .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success { stdout, .. } => {
                let diagnostics = parse_diagnostics(&stdout, preamble_lines);
                let unused: Vec<_> = diagnostics
                    .iter()
                    .filter(|d| d.message.contains("unused variable"))
                    .collect();
                assert!(
                    unused.is_empty(),
                    "injected cellN/last bindings must not warn when unread, got: {unused:?}",
                );
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!(
                    "cell ignoring its inputs should build.\nstdout(tail): {}\nstderr(tail): {}",
                    &stdout[stdout.len().saturating_sub(2000)..],
                    &stderr[stderr.len().saturating_sub(1500)..],
                );
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
        let hash = content_hash(
            source,
            cargo_toml,
            &[],
            None,
            None,
            false,
            false,
            false,
            CellTarget::Executor,
        );
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
            CellTarget::Executor,
        )
        .expect("scaffold should succeed");

        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Executor,
            false,
            false,
            false,
        )
        .await
        .expect("build should not return an infra error");

        let (wasm_bytes, js_glue) = match result {
            BuildResult::Success {
                wasm_path, js_glue, ..
            } => {
                let wasm = std::fs::read(&wasm_path).expect("should read WASM blob");
                (
                    wasm,
                    js_glue.expect("an ordinary cell carries wasm-bindgen glue"),
                )
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
            false,
            false,
            CellTarget::Executor,
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
            CellTarget::Executor,
        )
        .expect("scaffold should succeed");

        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Executor,
            false,
            false,
            false,
        )
        .await
        .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success {
                wasm_path, stdout, ..
            } => {
                assert!(wasm_path.exists(), "WASM blob should exist on disk");
                let wasm_bytes = std::fs::read(&wasm_path).unwrap();
                assert_eq!(
                    &wasm_bytes[..4],
                    b"\x00asm",
                    "WASM blob should start with the WASM magic number",
                );
                // The tick wrapper accesses the simulation static through a
                // raw pointer; a direct `&mut` would surface a
                // `static_mut_refs` warning attributed past the user's code.
                assert_no_static_mut_warning(&stdout);
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!(
                    "sim bus cell should compile successfully but got failure.\nstdout: {stdout}\nstderr: {stderr}"
                );
            }
        }
    }

    /// Fails the calling test if the build output carries a `static_mut_refs`
    /// warning. The sim/live-view tick wrappers hold their state in a
    /// `static mut`; taking `&mut` to it directly warns on current
    /// toolchains, and the diagnostic maps to a line PAST the end of the
    /// user's source (the wrapper sits below it) — exactly how it surfaced
    /// in the cannon notebook's "Newton, drawn" cell.
    fn assert_no_static_mut_warning(stdout: &str) {
        let diagnostics = parse_diagnostics(stdout, 0);
        let offenders: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.message.contains("mutable static"))
            .collect();
        assert!(
            offenders.is_empty(),
            "tick wrapper must not warn about mutable-static references, got: {offenders:?}",
        );
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

        let (crate_dir, _preamble, _is_async, is_sim) = scaffold_micro_crate(
            &cache_dir,
            &cell_path,
            session_id,
            cell_id,
            source,
            cargo_toml,
            &[],
            None,
            None,
            CellTarget::Executor,
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

        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Executor,
            false,
            false,
            false,
        )
        .await
        .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success {
                wasm_path, stdout, ..
            } => {
                assert!(wasm_path.exists(), "WASM blob should exist on disk");
                // Same raw-pointer discipline as the simulation wrapper — the
                // cannon "Newton, drawn" cell surfaced this warning live.
                assert_no_static_mut_warning(&stdout);
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

    /// Comment marker a public-notebook cell can carry in its source to declare
    /// itself a *deliberate* compile failure (a teaching cell, e.g. "this is the
    /// dyn-async-trait wall"). The gate below then asserts the cell FAILS to
    /// type-check — regression-locking the pedagogy: if a toolchain or library
    /// change ever makes the cell compile, the gate flags the notebook as stale.
    ///
    /// Usage inside a cell: `// ironpad-test: compile-fail`
    const EXPECTED_COMPILE_FAIL_MARKER: &str = "ironpad-test: compile-fail";

    /// Compiles every Code cell in every public notebook to verify they all
    /// type-check against wasm32-unknown-unknown — except cells carrying
    /// [`EXPECTED_COMPILE_FAIL_MARKER`], which must FAIL to type-check. Uses
    /// `cargo check` (not `cargo build`) with a shared `CARGO_HOME` and target
    /// dir so dependencies are downloaded/compiled only once.
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
            // Match production: cells compile against the notebook-level
            // shared source PLUS every shared cell, assembled by the same
            // ironpad-common helper the browser uses (PRD-0044).
            let effective_shared = notebook.effective_shared_source();
            let shared_source = effective_shared.as_deref();

            // Sort cells by order so previous_cell_types accumulates correctly.
            let mut cells = notebook.cells.clone();
            cells.sort_by_key(|c| c.order);

            let mut previous_cell_types: Vec<String> = Vec::new();

            for cell in &cells {
                if cell.cell_type != CellType::Code || cell.shared {
                    // Markdown never compiles; shared cells compile as part of
                    // every OTHER cell's shared.rs, not as cells. Both hold an
                    // empty piping slot so cellN indices stay positional.
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
                    CellTarget::Executor,
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

                // Match production: rayon cells need the atomics/shared-memory
                // build (the scaffold injects wasm-bindgen-rayon, whose
                // `atomics` target-feature guard fails otherwise). The scaffold
                // detects this the same way via `merged_deps_contain_rayon`.
                let needs_atomics = merged_deps_contain_rayon(shared_cargo_toml, cell_cargo);
                // Match production for autodiff cells too: nightly toolchain,
                // -Zautodiff, and the fat-LTO profile the scaffold enforces.
                let needs_autodiff = uses_std_autodiff(&cell.source, shared_source);
                // And for SIMD cells: the +simd128 target feature (the scaffold
                // injects the portable_simd gate either way).
                let needs_simd = uses_wasm_simd(&cell.source, shared_source);
                let result = check_micro_crate(
                    &crate_dir,
                    &cache_dir,
                    session_id,
                    &unique_id,
                    None,
                    CellTarget::Executor,
                    needs_atomics,
                    needs_autodiff,
                    needs_simd,
                    super::build::build_timeout(),
                )
                .await;

                // Cells carrying the compile-fail marker are deliberate teaching
                // failures (e.g. the dyn-async-trait wall): the gate asserts they
                // FAIL, so a toolchain or library change that makes one start
                // compiling flags the notebook as stale instead of slipping by.
                let expects_failure = cell.source.contains(EXPECTED_COMPILE_FAIL_MARKER);

                match result {
                    Ok(CheckResult::Ok) => {
                        if expects_failure {
                            failures.push(format!(
                                "{filename} / {} ({}): marked `{EXPECTED_COMPILE_FAIL_MARKER}` \
                                 but now COMPILES — the teaching error it demonstrates may have \
                                 been fixed by a toolchain/library change; update the notebook",
                                cell.id, cell.label
                            ));
                        }
                        previous_cell_types.push(String::new());
                    }
                    Ok(CheckResult::Failure { stdout, stderr }) if !expects_failure => {
                        // Show the TAIL of each stream: with --message-format=json
                        // the error `compiler-message` and the final
                        // `build-finished` record are at the end, while the head is
                        // just dependency-locking + successful-artifact noise.
                        let tail = |s: &str, n: usize| -> String {
                            let chars: Vec<char> = s.chars().collect();
                            chars[chars.len().saturating_sub(n)..].iter().collect()
                        };
                        failures.push(format!(
                            "{filename} / {} ({}):\n  stdout(tail): {}\n  stderr(tail): {}",
                            cell.id,
                            cell.label,
                            tail(&stdout, 2000),
                            tail(&stderr, 1500),
                        ));
                        previous_cell_types.push(String::new());
                    }
                    // Marked cell failed to compile: exactly what it promises.
                    Ok(CheckResult::Failure { .. }) => {
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

    // ── Linux cells (PRD-0066) ──────────────────────────────────────────

    #[tokio::test]
    #[ignore = "slow: invokes cargo build --target wasm32-browserpod-linux-musl"]
    async fn compile_linux_cell_produces_a_program_exporting_start() {
        // The whole T-003/T-004 claim in one pass: a cell that is a complete
        // Rust program compiles on the browserpod pin, to the browserpod
        // triple, into an executable a pod can start — with no `cell_main`,
        // no `ironpad-cell`, and no wasm-bindgen stage anywhere in it.
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-session";
        let cell_id = "linux-trivial";
        // Uses std::fs and std::thread: things a wasm32-unknown-unknown cell
        // cannot do, so a build that quietly took the ordinary path fails
        // here rather than passing.
        let source = "use std::fs;\n\nfn main() {\n    fs::write(\"/tmp/ironpad-linux-cell\", b\"ok\").unwrap();\n    let t = std::thread::spawn(|| 40 + 2);\n    println!(\"{}\", t.join().unwrap());\n}";

        let (crate_dir, preamble_lines, is_async, is_sim) = scaffold_micro_crate(
            &cache_dir,
            &cell_path,
            session_id,
            cell_id,
            source,
            "[dependencies]",
            &[],
            None,
            None,
            CellTarget::Linux,
        )
        .expect("scaffold should succeed");
        assert_eq!(preamble_lines, 0);
        assert!(!is_async && !is_sim);

        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Linux,
            false,
            false,
            false,
        )
        .await
        .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success {
                wasm_path, js_glue, ..
            } => {
                assert!(
                    js_glue.is_none(),
                    "a Linux cell has no JS boundary and so no wasm-bindgen glue"
                );
                assert!(
                    wasm_path
                        .to_string_lossy()
                        .contains("wasm32-browserpod-linux-musl"),
                    "artifact should live under its own target triple: {}",
                    wasm_path.display(),
                );

                let bytes = std::fs::read(&wasm_path).expect("artifact should exist on disk");
                let exports = wasm_export_names(&bytes);
                assert!(
                    exports.iter().any(|e| e == "_start"),
                    "a pod starts the program at `_start`; exports were {exports:?}",
                );
                // `__startThread` is how their kernel enters a spawned thread
                // in a fresh Worker, and memory is IMPORTED (shared), not
                // exported — both are properties of the target spec that this
                // build must not have lost.
                assert!(
                    exports.iter().any(|e| e == "__startThread"),
                    "exports were {exports:?}",
                );
                assert!(
                    !exports.iter().any(|e| e == "memory"),
                    "memory is imported from the kernel, not exported: {exports:?}",
                );
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!("Linux cell failed to compile.\nstdout: {stdout}\nstderr: {stderr}");
            }
        }
    }

    #[tokio::test]
    #[ignore = "slow: invokes cargo build --target wasm32-browserpod-linux-musl"]
    async fn linux_cell_compiles_against_the_notebook_shared_module() {
        // The unit tests can see that `mod shared;` was written; only a real
        // build can see that cargo accepts the generated manifest (bin target
        // plus forwarded `[profile.release]`) and that rustc resolves the
        // module from a whole program rather than from the `cell_main`
        // wrapper it does not have.
        let cache_dir = tempdir();
        let cell_path = ironpad_cell_path();
        let session_id = "e2e-session";
        let cell_id = "linux-shared";
        let source = "fn main() {\n    println!(\"{}\", shared::doubled(21));\n}";
        let shared = "pub fn doubled(n: u32) -> u32 { n * 2 }";

        let (crate_dir, preamble_lines, ..) = scaffold_micro_crate(
            &cache_dir,
            &cell_path,
            session_id,
            cell_id,
            source,
            "[profile.release]\nopt-level = 1\n",
            &[],
            None,
            Some(shared),
            CellTarget::Linux,
        )
        .expect("scaffold should succeed");
        assert_eq!(preamble_lines, 1, "one injected line: `mod shared;`");

        let result = build_micro_crate(
            &crate_dir,
            &cache_dir,
            session_id,
            cell_id,
            None,
            CellTarget::Linux,
            false,
            false,
            false,
        )
        .await
        .expect("build_micro_crate should not return an infra error");

        match result {
            BuildResult::Success { wasm_path, .. } => {
                let bytes = std::fs::read(&wasm_path).expect("artifact should exist");
                assert!(wasm_export_names(&bytes).iter().any(|e| e == "_start"));
            }
            BuildResult::Failure { stdout, stderr } => {
                panic!("shared-source Linux cell failed.\nstdout: {stdout}\nstderr: {stderr}");
            }
        }
    }
}
