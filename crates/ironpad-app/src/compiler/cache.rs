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
const CACHE_EPOCH: u32 = 2;

// ── Public API ───────────────────────────────────────────────────────────────

/// Compute a deterministic blake3 content hash from cell source, Cargo.toml,
/// and predecessor cell type tags.
///
/// The hash includes the fixed target triple so any future target change
/// naturally invalidates the cache.  Each previous type tag is followed by a
/// NUL separator so that `["u32", ""]` and `["", "u32"]` produce distinct
/// hashes. Also folds in the process-cached toolchain fingerprint (rustc
/// version + host wasm-bindgen CLI version, see [`toolchain_fingerprint`]) so
/// a toolchain upgrade invalidates stale cached blobs.
pub fn content_hash(
    source: &str,
    cargo_toml: &str,
    previous_types: &[String],
    shared_cargo_toml: Option<&str>,
    shared_source: Option<&str>,
    needs_atomics: bool,
) -> String {
    content_hash_inner(
        source,
        cargo_toml,
        previous_types,
        shared_cargo_toml,
        shared_source,
        needs_atomics,
        toolchain_fingerprint(),
    )
}

/// Core hashing logic, parameterized on an explicit toolchain fingerprint so
/// it's unit-testable without depending on the process-global cache. See
/// [`content_hash`] for the public entry point.
#[allow(clippy::too_many_arguments)]
fn content_hash_inner(
    source: &str,
    cargo_toml: &str,
    previous_types: &[String],
    shared_cargo_toml: Option<&str>,
    shared_source: Option<&str>,
    needs_atomics: bool,
    toolchain: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(source.as_bytes());
    hasher.update(cargo_toml.as_bytes());
    hasher.update(TARGET_TRIPLE.as_bytes());
    for t in previous_types {
        hasher.update(t.as_bytes());
        hasher.update(b"\x00");
    }
    if let Some(shared) = shared_cargo_toml {
        hasher.update(b"\x01");
        hasher.update(shared.as_bytes());
    }
    if let Some(shared) = shared_source {
        hasher.update(b"\x02");
        hasher.update(shared.as_bytes());
    }
    hasher.update(b"\x03");
    hasher.update(if needs_atomics {
        b"atomics=1"
    } else {
        b"atomics=0"
    });
    hasher.update(b"\x04");
    hasher.update(&CACHE_EPOCH.to_le_bytes());
    hasher.update(b"\x05");
    hasher.update(toolchain.as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// Path where a cached WASM blob lives (or would live) for the given hash.
pub fn cache_blob_path(cache_dir: &Path, hash: &str) -> PathBuf {
    cache_dir.join("blobs").join(format!("{hash}.wasm"))
}

/// Path where cached JS glue lives (or would live) for the given hash.
pub fn cache_js_glue_path(cache_dir: &Path, hash: &str) -> PathBuf {
    cache_dir.join("blobs").join(format!("{hash}.js"))
}

/// Cached compilation result: WASM blob and optional JS glue.
pub struct CacheHit {
    pub wasm_bytes: Vec<u8>,
    pub js_glue: Option<String>,
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

    tracing::info!(
        hash,
        wasm_bytes = wasm_bytes.len(),
        has_js_glue = js_glue.is_some(),
        "cache hit"
    );

    Some(CacheHit {
        wasm_bytes,
        js_glue,
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

/// Store a compiled WASM blob (and optional JS glue) in the cache.
///
/// Creates the `blobs/` directory if it doesn't already exist.
pub fn store_blob(
    cache_dir: &Path,
    hash: &str,
    wasm_bytes: &[u8],
    js_glue: Option<&str>,
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
        let a = content_hash("fn main() {}", "[dependencies]", &[], None, None, false);
        let b = content_hash("fn main() {}", "[dependencies]", &[], None, None, false);
        assert_eq!(a, b);
    }

    #[test]
    fn hash_changes_when_source_changes() {
        let a = content_hash("fn main() { 1 }", "[dependencies]", &[], None, None, false);
        let b = content_hash("fn main() { 2 }", "[dependencies]", &[], None, None, false);
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
        );
        let b = content_hash(
            source,
            r#"[dependencies]\nrand = "0.8""#,
            &[],
            None,
            None,
            false,
        );
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_64_hex_chars() {
        let h = content_hash("x", "y", &[], None, None, false);
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    // Single-char names are idiomatic for hash-comparison test fixtures.
    #[allow(clippy::many_single_char_names)]
    fn hash_changes_when_previous_types_change() {
        let s = "let x = 1;";
        let c = "[dependencies]";
        let a = content_hash(s, c, &[], None, None, false);
        let b = content_hash(s, c, &["u32".into()], None, None, false);
        let d = content_hash(s, c, &["String".into()], None, None, false);
        assert_ne!(a, b);
        assert_ne!(b, d);
    }

    #[test]
    fn hash_distinguishes_type_positions() {
        let s = "x";
        let c = "y";
        let a = content_hash(s, c, &["u32".into(), String::new()], None, None, false);
        let b = content_hash(s, c, &[String::new(), "u32".into()], None, None, false);
        assert_ne!(a, b);
    }

    #[test]
    // Single-char names are idiomatic for hash-comparison test fixtures.
    #[allow(clippy::many_single_char_names)]
    fn hash_changes_when_shared_cargo_toml_changes() {
        let s = "let x = 1;";
        let c = "[dependencies]";
        let a = content_hash(s, c, &[], None, None, false);
        let b = content_hash(
            s,
            c,
            &[],
            Some("[dependencies]\nserde = \"1\""),
            None,
            false,
        );
        let d = content_hash(
            s,
            c,
            &[],
            Some("[dependencies]\nrand = \"0.8\""),
            None,
            false,
        );
        assert_ne!(a, b);
        assert_ne!(b, d);
    }

    #[test]
    fn hash_with_none_shared_differs_from_empty_shared() {
        let s = "x";
        let c = "y";
        let a = content_hash(s, c, &[], None, None, false);
        let b = content_hash(s, c, &[], Some(""), None, false);
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

        store_blob(dir.path(), hash, blob, None).unwrap();

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

        store_blob(dir.path(), hash, blob, Some(glue)).unwrap();

        let hit = try_cache_hit(dir.path(), hash).unwrap();
        assert_eq!(hit.wasm_bytes, blob);
        assert_eq!(hit.js_glue.as_deref(), Some(glue));
    }

    #[test]
    fn store_creates_blobs_dir() {
        let dir = tempfile::tempdir().unwrap();
        let blobs_dir = dir.path().join("blobs");
        assert!(!blobs_dir.exists());

        store_blob(dir.path(), "aabbccdd", b"wasm", None).unwrap();

        assert!(blobs_dir.exists());
    }

    #[test]
    fn round_trip_with_real_hash() {
        let dir = tempfile::tempdir().unwrap();
        let source = "let x = 42;";
        let cargo = "[dependencies]";
        let hash = content_hash(source, cargo, &[], None, None, false);
        let blob = vec![0u8; 256];
        let glue = "// js glue content";

        store_blob(dir.path(), &hash, &blob, Some(glue)).unwrap();

        let hit = try_cache_hit(dir.path(), &hash).unwrap();
        assert_eq!(hit.wasm_bytes, blob);
        assert_eq!(hit.js_glue.as_deref(), Some(glue));
    }

    // ── T-005: Additional edge-case tests ───────────────────────────────

    #[test]
    fn hash_empty_source_is_valid() {
        let h = content_hash("", "", &[], None, None, false);
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_same_shared_cargo_toml_is_deterministic() {
        let shared = "[dependencies]\nserde = \"1\"";
        let a = content_hash("x", "y", &[], Some(shared), None, false);
        let b = content_hash("x", "y", &[], Some(shared), None, false);
        assert_eq!(a, b);
    }

    #[test]
    fn store_overwrites_existing_blob() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "aabbccdd";
        let blob_v1 = b"version-1";
        let blob_v2 = b"version-2-longer";

        store_blob(dir.path(), hash, blob_v1, None).unwrap();
        let hit1 = try_cache_hit(dir.path(), hash).unwrap();
        assert_eq!(hit1.wasm_bytes, blob_v1);

        store_blob(dir.path(), hash, blob_v2, None).unwrap();
        let hit2 = try_cache_hit(dir.path(), hash).unwrap();
        assert_eq!(hit2.wasm_bytes, blob_v2);
    }

    #[test]
    fn store_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        store_blob(dir.path(), "deadbeef", b"wasm", Some("glue()")).unwrap();

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
        store_blob(dir.path(), hash, b"wasm", None).unwrap();
        let hit = try_cache_hit(dir.path(), hash).unwrap();
        assert!(hit.js_glue.is_none());

        // Store again with JS glue.
        store_blob(dir.path(), hash, b"wasm", Some("glue()")).unwrap();
        let hit = try_cache_hit(dir.path(), hash).unwrap();
        assert_eq!(hit.js_glue.as_deref(), Some("glue()"));
    }

    // ── T-002: needs_atomics flag ────────────────────────────────────────

    #[test]
    fn hash_changes_with_needs_atomics() {
        let s = "x";
        let c = "y";
        let a = content_hash(s, c, &[], None, None, false);
        let b = content_hash(s, c, &[], None, None, true);
        assert_ne!(a, b);
    }

    // ── T-003: toolchain fingerprint folded into the cache key ───────────

    #[test]
    fn inner_hash_changes_when_toolchain_fingerprint_changes() {
        let a = content_hash_inner("x", "y", &[], None, None, false, "toolchain-a");
        let b = content_hash_inner("x", "y", &[], None, None, false, "toolchain-b");
        assert_ne!(a, b);
    }

    #[test]
    fn inner_hash_is_deterministic_for_same_toolchain() {
        let a = content_hash_inner("x", "y", &[], None, None, false, "toolchain-a");
        let b = content_hash_inner("x", "y", &[], None, None, false, "toolchain-a");
        assert_eq!(a, b);
    }
}
