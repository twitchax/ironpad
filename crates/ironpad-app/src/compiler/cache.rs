//! Blake3 content-hash caching for compiled WASM blobs.
//!
//! Hashes `source || cargo_toml || "wasm32-unknown-unknown" || previous_types
//! || toolchain_fingerprint` with blake3 and stores/retrieves compiled
//! `.wasm` blobs under `{cache_dir}/blobs/{hash}.wasm`. The toolchain
//! fingerprint (rustc version + host wasm-bindgen CLI version, see
//! `compiler/toolchain.rs`) ensures a toolchain upgrade invalidates stale
//! cached blobs instead of silently serving output built against a
//! different, possibly incompatible, toolchain.

use std::path::{Path, PathBuf};

use super::toolchain::toolchain_fingerprint;

/// The compilation target baked into the cache key so that a future target
/// change automatically invalidates existing entries.
const TARGET_TRIPLE: &str = "wasm32-unknown-unknown";

/// Monotonic epoch counter baked into the cache key.
///
/// Bump this whenever the compilation pipeline changes in a way that should
/// invalidate all cached blobs (e.g. RUSTFLAGS changes, wasm-bindgen
/// post-processing changes, scaffold template changes, `ironpad-cell` source
/// edits — there is no automatic hash of the injected `ironpad-cell` crate's
/// contents, so bump this manually when it changes).
///
/// Bumped 1 -> 2 for PRD-0031 T-003: folding the toolchain fingerprint into
/// the key (see [`toolchain_fingerprint`]) should invalidate all pre-existing
/// blobs once, since their toolchain provenance is unknown.
///
/// Bumped 2 -> 3 for PRD-0036 T-008: `ironpad-cell`'s `CellInputs::from_raw`
/// gained bounds checking, so cached cells should rebuild against the safer
/// runtime.
///
/// Bumped 3 -> 4 for PRD-0038 T-001: variable-length fields are now
/// length-prefixed (framed) in the hash input, which changes every key, so
/// pre-existing blobs (hashed with the old bare-concatenation scheme) must be
/// invalidated once.
///
/// Bumped 4 -> 5 for PRD-0043 T-001: `ironpad-cell` gained the `blocking`
/// module (JSPI host imports), so cached cells must rebuild against the new
/// runtime. (The `needs_simd` hash byte added in the same release also
/// changes every key; the bump keeps the documented ironpad-cell discipline.)
///
/// Bumped 5 -> 6: `IntoPanels for Canvas` switched from an Html panel (whose
/// data: URI the sanitizer strips — tuple outputs rendered an empty image) to
/// the structured `BlobImage` panel; cached blobs bake the old behavior in.
const CACHE_EPOCH: u32 = 6;

// ── Public API ───────────────────────────────────────────────────────────────

/// Compute a deterministic blake3 content hash from cell source, Cargo.toml,
/// and predecessor cell type tags.
///
/// The hash includes the fixed target triple so any future target change
/// naturally invalidates the cache. Every variable-length field is
/// length-prefixed (framed) so field boundaries are unambiguous — otherwise
/// distinct inputs whose bytes concatenate identically would collide (e.g.
/// `("ab", "c")` and `("a", "bc")`) and serve each other's compiled WASM.
/// Also folds in the process-cached toolchain fingerprint (rustc version +
/// host wasm-bindgen CLI version, see [`toolchain_fingerprint`]) so a toolchain
/// upgrade invalidates stale cached blobs.
#[allow(clippy::fn_params_excessive_bools)]
#[allow(clippy::too_many_arguments)]
pub fn content_hash(
    source: &str,
    cargo_toml: &str,
    previous_types: &[String],
    shared_cargo_toml: Option<&str>,
    shared_source: Option<&str>,
    needs_atomics: bool,
    needs_autodiff: bool,
    needs_simd: bool,
) -> String {
    content_hash_inner(
        source,
        cargo_toml,
        previous_types,
        shared_cargo_toml,
        shared_source,
        needs_atomics,
        needs_autodiff,
        needs_simd,
        toolchain_fingerprint(),
    )
}

/// Core hashing logic, parameterized on an explicit toolchain fingerprint so
/// it's unit-testable without depending on the process-global cache. See
/// [`content_hash`] for the public entry point.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::fn_params_excessive_bools)]
fn content_hash_inner(
    source: &str,
    cargo_toml: &str,
    previous_types: &[String],
    shared_cargo_toml: Option<&str>,
    shared_source: Option<&str>,
    needs_atomics: bool,
    needs_autodiff: bool,
    needs_simd: bool,
    toolchain: &str,
) -> String {
    // Every variable-length field is length-prefixed so its boundary with the
    // next field is unambiguous. A bare concatenation (`source || cargo_toml ||
    // …`) lets distinct inputs whose bytes line up collide and serve each
    // other's cached WASM — e.g. `("ab", "c")` vs `("a", "bc")`, or a
    // `cargo_toml` ending in the target triple bytes vs a shorter one.
    let mut hasher = blake3::Hasher::new();
    update_framed(&mut hasher, source.as_bytes());
    update_framed(&mut hasher, cargo_toml.as_bytes());
    update_framed(&mut hasher, TARGET_TRIPLE.as_bytes());
    update_framed(&mut hasher, &(previous_types.len() as u64).to_le_bytes());
    for t in previous_types {
        update_framed(&mut hasher, t.as_bytes());
    }
    update_framed_opt(&mut hasher, shared_cargo_toml.map(str::as_bytes));
    update_framed_opt(&mut hasher, shared_source.map(str::as_bytes));
    hasher.update(&[u8::from(needs_atomics)]);
    hasher.update(&[u8::from(needs_autodiff)]);
    hasher.update(&[u8::from(needs_simd)]);
    hasher.update(&CACHE_EPOCH.to_le_bytes());
    update_framed(&mut hasher, toolchain.as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// Feed `bytes` into the hasher framed by an 8-byte little-endian length
/// prefix, so the boundary between this field and the next is unambiguous and
/// no two distinct field splits can produce the same byte stream.
fn update_framed(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Frame an optional field: a presence byte (`1`/`0`) followed by the framed
/// content when present. Keeps `None` distinct from `Some("")`.
fn update_framed_opt(hasher: &mut blake3::Hasher, bytes: Option<&[u8]>) {
    match bytes {
        Some(b) => {
            hasher.update(&[1]);
            update_framed(hasher, b);
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

/// Path where a cached WASM blob lives (or would live) for the given hash.
pub fn cache_blob_path(cache_dir: &Path, hash: &str) -> PathBuf {
    cache_dir.join("blobs").join(format!("{hash}.wasm"))
}

/// Path where cached JS glue lives (or would live) for the given hash.
pub fn cache_js_glue_path(cache_dir: &Path, hash: &str) -> PathBuf {
    cache_dir.join("blobs").join(format!("{hash}.js"))
}

/// Path where cached diagnostics live (or would live) for the given hash.
pub fn cache_diag_path(cache_dir: &Path, hash: &str) -> PathBuf {
    cache_dir.join("blobs").join(format!("{hash}.diag.json"))
}

/// Cached compilation result: WASM blob, optional JS glue, and any diagnostics
/// (warnings) from the original compile so they survive a cache hit.
pub struct CacheHit {
    pub wasm_bytes: Vec<u8>,
    pub js_glue: Option<String>,
    pub diagnostics: Vec<ironpad_common::Diagnostic>,
}

/// Attempt to read a cached WASM blob (and JS glue if present).
///
/// Returns `Some(CacheHit)` on cache hit, `None` on miss.
/// Filesystem errors (permission denied, corrupt reads) are treated as misses
/// and logged at warn level.
pub fn try_cache_hit(cache_dir: &Path, hash: &str) -> Option<CacheHit> {
    let path = cache_blob_path(cache_dir, hash);

    let wasm_bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(hash, "cache miss");
            return None;
        }
        Err(e) => {
            tracing::warn!(hash, error = %e, "cache read error — treating as miss");
            return None;
        }
    };

    // JS glue is optional — older cache entries may not have it.
    let js_glue_path = cache_js_glue_path(cache_dir, hash);
    let js_glue = std::fs::read_to_string(&js_glue_path).ok();

    // Diagnostics are optional too (older entries, or a clean compile). A
    // malformed file is treated as "no diagnostics" rather than a cache miss.
    let diagnostics = std::fs::read(cache_diag_path(cache_dir, hash))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();

    tracing::info!(
        hash,
        wasm_bytes = wasm_bytes.len(),
        has_js_glue = js_glue.is_some(),
        "cache hit"
    );

    Some(CacheHit {
        wasm_bytes,
        js_glue,
        diagnostics,
    })
}

/// Write `contents` to `path` atomically: write a uniquely-named temp sibling,
/// then rename it into place. `rename` is atomic on the same filesystem, so a
/// concurrent [`try_cache_hit`] reader sees either the old file or the fully
/// written new one — never the truncated partial write `std::fs::write` exposes.
fn atomic_write(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp.{}", uuid::Uuid::new_v4()));
    let tmp = PathBuf::from(tmp);

    std::fs::write(&tmp, contents)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp); // best-effort cleanup on failure
        return Err(e.into());
    }
    Ok(())
}

/// Store a compiled WASM blob (plus optional JS glue and any diagnostics) in
/// the cache.
///
/// Creates the `blobs/` directory if it doesn't already exist. Diagnostics
/// (warnings from a successful compile) are cached so they survive a cache hit.
pub fn store_blob(
    cache_dir: &Path,
    hash: &str,
    wasm_bytes: &[u8],
    js_glue: Option<&str>,
    diagnostics: &[ironpad_common::Diagnostic],
) -> anyhow::Result<()> {
    let path = cache_blob_path(cache_dir, hash);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    atomic_write(&path, wasm_bytes)?;

    if let Some(glue) = js_glue {
        let js_path = cache_js_glue_path(cache_dir, hash);
        atomic_write(&js_path, glue.as_bytes())?;
        tracing::info!(hash, js_bytes = glue.len(), "cached JS glue");
    }

    // Only write a diagnostics file when there's something to remember.
    if !diagnostics.is_empty() {
        let json = serde_json::to_vec(diagnostics)?;
        atomic_write(&cache_diag_path(cache_dir, hash), &json)?;
        tracing::info!(
            hash,
            diagnostic_count = diagnostics.len(),
            "cached diagnostics"
        );
    }

    tracing::info!(
        hash,
        bytes = wasm_bytes.len(),
        path = %path.display(),
        "cached WASM blob",
    );

    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── content_hash ────────────────────────────────────────────────────

    #[test]
    fn hash_is_deterministic() {
        let a = content_hash(
            "fn main() {}",
            "[dependencies]",
            &[],
            None,
            None,
            false,
            false,
            false,
        );
        let b = content_hash(
            "fn main() {}",
            "[dependencies]",
            &[],
            None,
            None,
            false,
            false,
            false,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn hash_changes_when_source_changes() {
        let a = content_hash(
            "fn main() { 1 }",
            "[dependencies]",
            &[],
            None,
            None,
            false,
            false,
            false,
        );
        let b = content_hash(
            "fn main() { 2 }",
            "[dependencies]",
            &[],
            None,
            None,
            false,
            false,
            false,
        );
        assert_ne!(a, b);
    }

    #[test]
    fn hash_changes_when_cargo_toml_changes() {
        let source = "fn main() {}";
        let a = content_hash(
            source,
            r#"[dependencies]\nserde = "1""#,
            &[],
            None,
            None,
            false,
            false,
            false,
        );
        let b = content_hash(
            source,
            r#"[dependencies]\nrand = "0.8""#,
            &[],
            None,
            None,
            false,
            false,
            false,
        );
        assert_ne!(a, b);
    }

    #[test]
    // Single-char names are idiomatic for hash-comparison test fixtures.
    #[allow(clippy::many_single_char_names)]
    fn hash_disambiguates_field_boundaries() {
        // Regression (PRD-0038 T-001): variable-length fields must be framed, or
        // two distinct inputs whose bytes concatenate identically collide and
        // serve each other's compiled WASM from cache.

        // source/cargo_toml boundary: "ab"+"c" and "a"+"bc" both concatenate to
        // "abc" under the old bare-concatenation scheme.
        let a = content_hash("ab", "c", &[], None, None, false, false, false);
        let b = content_hash("a", "bc", &[], None, None, false, false, false);
        assert_ne!(a, b, "source/cargo_toml boundary must be unambiguous");

        // cargo_toml/target boundary: the fixed target triple is appended right
        // after cargo_toml, so a cargo_toml ending in those bytes could shift
        // the split. ("", "x" + TARGET) vs ("x", TARGET) collided before framing.
        let target = "wasm32-unknown-unknown";
        let c = content_hash(
            "",
            &format!("x{target}"),
            &[],
            None,
            None,
            false,
            false,
            false,
        );
        let d = content_hash("x", target, &[], None, None, false, false, false);
        assert_ne!(c, d, "cargo_toml/target boundary must be unambiguous");
    }

    #[test]
    fn hash_is_64_hex_chars() {
        let h = content_hash("x", "y", &[], None, None, false, false, false);
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    // Single-char names are idiomatic for hash-comparison test fixtures.
    #[allow(clippy::many_single_char_names)]
    fn hash_changes_when_previous_types_change() {
        let s = "let x = 1;";
        let c = "[dependencies]";
        let a = content_hash(s, c, &[], None, None, false, false, false);
        let b = content_hash(s, c, &["u32".into()], None, None, false, false, false);
        let d = content_hash(s, c, &["String".into()], None, None, false, false, false);
        assert_ne!(a, b);
        assert_ne!(b, d);
    }

    #[test]
    fn hash_distinguishes_type_positions() {
        let s = "x";
        let c = "y";
        let a = content_hash(
            s,
            c,
            &["u32".into(), String::new()],
            None,
            None,
            false,
            false,
            false,
        );
        let b = content_hash(
            s,
            c,
            &[String::new(), "u32".into()],
            None,
            None,
            false,
            false,
            false,
        );
        assert_ne!(a, b);
    }

    #[test]
    // Single-char names are idiomatic for hash-comparison test fixtures.
    #[allow(clippy::many_single_char_names)]
    fn hash_changes_when_shared_cargo_toml_changes() {
        let s = "let x = 1;";
        let c = "[dependencies]";
        let a = content_hash(s, c, &[], None, None, false, false, false);
        let b = content_hash(
            s,
            c,
            &[],
            Some("[dependencies]\nserde = \"1\""),
            None,
            false,
            false,
            false,
        );
        let d = content_hash(
            s,
            c,
            &[],
            Some("[dependencies]\nrand = \"0.8\""),
            None,
            false,
            false,
            false,
        );
        assert_ne!(a, b);
        assert_ne!(b, d);
    }

    #[test]
    fn hash_with_none_shared_differs_from_empty_shared() {
        let s = "x";
        let c = "y";
        let a = content_hash(s, c, &[], None, None, false, false, false);
        let b = content_hash(s, c, &[], Some(""), None, false, false, false);
        assert_ne!(a, b);
    }

    // ── cache_blob_path ─────────────────────────────────────────────────

    #[test]
    fn blob_path_layout() {
        let path = cache_blob_path(Path::new("/cache"), "abc123");
        assert_eq!(path, PathBuf::from("/cache/blobs/abc123.wasm"));
    }

    #[test]
    fn js_glue_path_layout() {
        let path = cache_js_glue_path(Path::new("/cache"), "abc123");
        assert_eq!(path, PathBuf::from("/cache/blobs/abc123.js"));
    }

    // ── try_cache_hit / store_blob (integration) ────────────────────────

    #[test]
    fn miss_on_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(try_cache_hit(dir.path(), "nonexistent").is_none());
    }

    #[test]
    fn store_and_hit() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "deadbeef01234567deadbeef01234567deadbeef01234567deadbeef01234567";
        let blob = b"\x00asm\x01\x00\x00\x00";

        store_blob(dir.path(), hash, blob, None, &[]).unwrap();

        let hit = try_cache_hit(dir.path(), hash);
        assert!(hit.is_some());
        let hit = hit.unwrap();
        assert_eq!(hit.wasm_bytes, blob);
        assert!(hit.js_glue.is_none());
    }

    #[test]
    fn store_and_hit_with_js_glue() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "deadbeef01234567deadbeef01234567deadbeef01234567deadbeef01234567";
        let blob = b"\x00asm\x01\x00\x00\x00";
        let glue = "export function init() {}";

        store_blob(dir.path(), hash, blob, Some(glue), &[]).unwrap();

        let hit = try_cache_hit(dir.path(), hash).unwrap();
        assert_eq!(hit.wasm_bytes, blob);
        assert_eq!(hit.js_glue.as_deref(), Some(glue));
    }

    #[test]
    fn store_creates_blobs_dir() {
        let dir = tempfile::tempdir().unwrap();
        let blobs_dir = dir.path().join("blobs");
        assert!(!blobs_dir.exists());

        store_blob(dir.path(), "aabbccdd", b"wasm", None, &[]).unwrap();

        assert!(blobs_dir.exists());
    }

    #[test]
    fn round_trip_with_real_hash() {
        let dir = tempfile::tempdir().unwrap();
        let source = "let x = 42;";
        let cargo = "[dependencies]";
        let hash = content_hash(source, cargo, &[], None, None, false, false, false);
        let blob = vec![0u8; 256];
        let glue = "// js glue content";

        store_blob(dir.path(), &hash, &blob, Some(glue), &[]).unwrap();

        let hit = try_cache_hit(dir.path(), &hash).unwrap();
        assert_eq!(hit.wasm_bytes, blob);
        assert_eq!(hit.js_glue.as_deref(), Some(glue));
    }

    // ── T-005: Additional edge-case tests ───────────────────────────────

    #[test]
    fn hash_empty_source_is_valid() {
        let h = content_hash("", "", &[], None, None, false, false, false);
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_same_shared_cargo_toml_is_deterministic() {
        let shared = "[dependencies]\nserde = \"1\"";
        let a = content_hash("x", "y", &[], Some(shared), None, false, false, false);
        let b = content_hash("x", "y", &[], Some(shared), None, false, false, false);
        assert_eq!(a, b);
    }

    #[test]
    fn store_overwrites_existing_blob() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "aabbccdd";
        let blob_v1 = b"version-1";
        let blob_v2 = b"version-2-longer";

        store_blob(dir.path(), hash, blob_v1, None, &[]).unwrap();
        let hit1 = try_cache_hit(dir.path(), hash).unwrap();
        assert_eq!(hit1.wasm_bytes, blob_v1);

        store_blob(dir.path(), hash, blob_v2, None, &[]).unwrap();
        let hit2 = try_cache_hit(dir.path(), hash).unwrap();
        assert_eq!(hit2.wasm_bytes, blob_v2);
    }

    #[test]
    fn store_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        store_blob(dir.path(), "deadbeef", b"wasm", Some("glue()"), &[]).unwrap();

        // The atomic write renames the temp into place — no `.tmp.*` sidecar
        // should survive to be mistaken for (or read as) a cache entry.
        let blobs = dir.path().join("blobs");
        let leftovers: Vec<_> = std::fs::read_dir(&blobs)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn cache_hit_without_js_glue_then_with() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "test1234";

        // Store without JS glue.
        store_blob(dir.path(), hash, b"wasm", None, &[]).unwrap();
        let hit = try_cache_hit(dir.path(), hash).unwrap();
        assert!(hit.js_glue.is_none());

        // Store again with JS glue.
        store_blob(dir.path(), hash, b"wasm", Some("glue()"), &[]).unwrap();
        let hit = try_cache_hit(dir.path(), hash).unwrap();
        assert_eq!(hit.js_glue.as_deref(), Some("glue()"));
    }

    #[test]
    fn diagnostics_survive_cache_round_trip() {
        use ironpad_common::{Diagnostic, Severity};
        let dir = tempfile::tempdir().unwrap();
        let hash = "cafef00d";
        let diags = vec![Diagnostic {
            message: "unused variable: `x`".into(),
            severity: Severity::Warning,
            spans: vec![],
            code: Some("unused_variables".into()),
        }];

        store_blob(dir.path(), hash, b"wasm", Some("glue()"), &diags).unwrap();
        let hit = try_cache_hit(dir.path(), hash).unwrap();

        assert_eq!(hit.diagnostics.len(), 1, "warnings survive a cache hit");
        assert_eq!(hit.diagnostics[0].message, "unused variable: `x`");
        assert_eq!(hit.diagnostics[0].severity, Severity::Warning);
    }

    #[test]
    fn empty_diagnostics_writes_no_file_and_reads_empty() {
        let dir = tempfile::tempdir().unwrap();
        store_blob(dir.path(), "beefbeef", b"wasm", None, &[]).unwrap();
        assert!(
            !cache_diag_path(dir.path(), "beefbeef").exists(),
            "no diag file when there are no diagnostics"
        );
        let hit = try_cache_hit(dir.path(), "beefbeef").unwrap();
        assert!(hit.diagnostics.is_empty());
    }

    // ── T-002: needs_atomics flag ────────────────────────────────────────

    #[test]
    fn hash_changes_with_needs_atomics() {
        let s = "x";
        let c = "y";
        let a = content_hash(s, c, &[], None, None, false, false, false);
        let b = content_hash(s, c, &[], None, None, true, false, false);
        assert_ne!(a, b);
    }

    #[test]
    fn hash_changes_with_needs_autodiff() {
        let a = content_hash("x", "y", &[], None, None, false, false, false);
        let b = content_hash("x", "y", &[], None, None, false, true, false);
        assert_ne!(a, b, "autodiff changes codegen, so it must change the key");
    }

    #[test]
    fn hash_changes_with_needs_simd() {
        let a = content_hash("x", "y", &[], None, None, false, false, false);
        let b = content_hash("x", "y", &[], None, None, false, false, true);
        assert_ne!(a, b, "simd128 changes codegen, so it must change the key");
    }

    // ── T-003: toolchain fingerprint folded into the cache key ───────────

    #[test]
    fn inner_hash_changes_when_toolchain_fingerprint_changes() {
        let a = content_hash_inner(
            "x",
            "y",
            &[],
            None,
            None,
            false,
            false,
            false,
            "toolchain-a",
        );
        let b = content_hash_inner(
            "x",
            "y",
            &[],
            None,
            None,
            false,
            false,
            false,
            "toolchain-b",
        );
        assert_ne!(a, b);
    }

    #[test]
    fn inner_hash_is_deterministic_for_same_toolchain() {
        let a = content_hash_inner(
            "x",
            "y",
            &[],
            None,
            None,
            false,
            false,
            false,
            "toolchain-a",
        );
        let b = content_hash_inner(
            "x",
            "y",
            &[],
            None,
            None,
            false,
            false,
            false,
            "toolchain-a",
        );
        assert_eq!(a, b);
    }
}
