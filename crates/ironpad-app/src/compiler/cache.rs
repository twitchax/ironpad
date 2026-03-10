//! Blake3 content-hash caching for compiled WASM blobs.
//!
//! Hashes `source || cargo_toml || "wasm32-unknown-unknown" || previous_types`
//! with blake3 and stores/retrieves compiled `.wasm` blobs under
//! `{cache_dir}/blobs/{hash}.wasm`.

use std::path::{Path, PathBuf};

/// The compilation target baked into the cache key so that a future target
/// change automatically invalidates existing entries.
const TARGET_TRIPLE: &str = "wasm32-unknown-unknown";

// ── Public API ───────────────────────────────────────────────────────────────

/// Compute a deterministic blake3 content hash from cell source, Cargo.toml,
/// and predecessor cell type tags.
///
/// The hash includes the fixed target triple so any future target change
/// naturally invalidates the cache.  Each previous type tag is followed by a
/// NUL separator so that `["u32", ""]` and `["", "u32"]` produce distinct
/// hashes.
pub fn content_hash(
    source: &str,
    cargo_toml: &str,
    previous_types: &[Option<String>],
    shared_cargo_toml: Option<&str>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(source.as_bytes());
    hasher.update(cargo_toml.as_bytes());
    hasher.update(TARGET_TRIPLE.as_bytes());
    for t in previous_types {
        hasher.update(t.as_deref().unwrap_or("").as_bytes());
        hasher.update(b"\x00");
    }
    if let Some(shared) = shared_cargo_toml {
        hasher.update(b"\x01");
        hasher.update(shared.as_bytes());
    }
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

    std::fs::write(&path, wasm_bytes)?;

    if let Some(glue) = js_glue {
        let js_path = cache_js_glue_path(cache_dir, hash);
        std::fs::write(&js_path, glue)?;
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
        let a = content_hash("fn main() {}", "[dependencies]", &[], None);
        let b = content_hash("fn main() {}", "[dependencies]", &[], None);
        assert_eq!(a, b);
    }

    #[test]
    fn hash_changes_when_source_changes() {
        let a = content_hash("fn main() { 1 }", "[dependencies]", &[], None);
        let b = content_hash("fn main() { 2 }", "[dependencies]", &[], None);
        assert_ne!(a, b);
    }

    #[test]
    fn hash_changes_when_cargo_toml_changes() {
        let source = "fn main() {}";
        let a = content_hash(source, r#"[dependencies]\nserde = "1""#, &[], None);
        let b = content_hash(source, r#"[dependencies]\nrand = "0.8""#, &[], None);
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_64_hex_chars() {
        let h = content_hash("x", "y", &[], None);
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_changes_when_previous_types_change() {
        let s = "let x = 1;";
        let c = "[dependencies]";
        let a = content_hash(s, c, &[], None);
        let b = content_hash(s, c, &[Some("u32".into())], None);
        let d = content_hash(s, c, &[Some("String".into())], None);
        assert_ne!(a, b);
        assert_ne!(b, d);
    }

    #[test]
    fn hash_distinguishes_type_positions() {
        let s = "x";
        let c = "y";
        let a = content_hash(s, c, &[Some("u32".into()), None], None);
        let b = content_hash(s, c, &[None, Some("u32".into())], None);
        assert_ne!(a, b);
    }

    #[test]
    fn hash_changes_when_shared_cargo_toml_changes() {
        let s = "let x = 1;";
        let c = "[dependencies]";
        let a = content_hash(s, c, &[], None);
        let b = content_hash(s, c, &[], Some("[dependencies]\nserde = \"1\""));
        let d = content_hash(s, c, &[], Some("[dependencies]\nrand = \"0.8\""));
        assert_ne!(a, b);
        assert_ne!(b, d);
    }

    #[test]
    fn hash_with_none_shared_differs_from_empty_shared() {
        let s = "x";
        let c = "y";
        let a = content_hash(s, c, &[], None);
        let b = content_hash(s, c, &[], Some(""));
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
        let hash = content_hash(source, cargo, &[], None);
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
        let h = content_hash("", "", &[], None);
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_same_shared_cargo_toml_is_deterministic() {
        let shared = "[dependencies]\nserde = \"1\"";
        let a = content_hash("x", "y", &[], Some(shared));
        let b = content_hash("x", "y", &[], Some(shared));
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
}
