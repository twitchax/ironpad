//! Blake3 content-hash caching for compiled WASM blobs.
//!
//! Hashes `source || cargo_toml || "wasm32-unknown-unknown"` with blake3 and
//! stores/retrieves compiled `.wasm` blobs under `{cache_dir}/blobs/{hash}.wasm`.

use std::path::{Path, PathBuf};

/// The compilation target baked into the cache key so that a future target
/// change automatically invalidates existing entries.
const TARGET_TRIPLE: &str = "wasm32-unknown-unknown";

// ── Public API ───────────────────────────────────────────────────────────────

/// Compute a deterministic blake3 content hash from cell source and Cargo.toml.
///
/// The hash includes the fixed target triple so any future target change
/// naturally invalidates the cache.
pub fn content_hash(source: &str, cargo_toml: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(source.as_bytes());
    hasher.update(cargo_toml.as_bytes());
    hasher.update(TARGET_TRIPLE.as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// Path where a cached WASM blob lives (or would live) for the given hash.
pub fn cache_blob_path(cache_dir: &Path, hash: &str) -> PathBuf {
    cache_dir.join("blobs").join(format!("{hash}.wasm"))
}

/// Attempt to read a cached WASM blob.
///
/// Returns `Some(bytes)` on cache hit, `None` on miss.
/// Filesystem errors (permission denied, corrupt reads) are treated as misses
/// and logged at warn level.
pub fn try_cache_hit(cache_dir: &Path, hash: &str) -> Option<Vec<u8>> {
    let path = cache_blob_path(cache_dir, hash);

    match std::fs::read(&path) {
        Ok(bytes) => {
            tracing::info!(hash, bytes = bytes.len(), "cache hit");
            Some(bytes)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(hash, "cache miss");
            None
        }
        Err(e) => {
            tracing::warn!(hash, error = %e, "cache read error — treating as miss");
            None
        }
    }
}

/// Store a compiled WASM blob in the cache.
///
/// Creates the `blobs/` directory if it doesn't already exist.
pub fn store_blob(cache_dir: &Path, hash: &str, wasm_bytes: &[u8]) -> anyhow::Result<()> {
    let path = cache_blob_path(cache_dir, hash);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(&path, wasm_bytes)?;

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
        let a = content_hash("fn main() {}", "[dependencies]");
        let b = content_hash("fn main() {}", "[dependencies]");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_changes_when_source_changes() {
        let a = content_hash("fn main() { 1 }", "[dependencies]");
        let b = content_hash("fn main() { 2 }", "[dependencies]");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_changes_when_cargo_toml_changes() {
        let source = "fn main() {}";
        let a = content_hash(source, r#"[dependencies]\nserde = "1""#);
        let b = content_hash(source, r#"[dependencies]\nrand = "0.8""#);
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_64_hex_chars() {
        let h = content_hash("x", "y");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── cache_blob_path ─────────────────────────────────────────────────

    #[test]
    fn blob_path_layout() {
        let path = cache_blob_path(Path::new("/cache"), "abc123");
        assert_eq!(path, PathBuf::from("/cache/blobs/abc123.wasm"));
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

        store_blob(dir.path(), hash, blob).unwrap();

        let hit = try_cache_hit(dir.path(), hash);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap(), blob);
    }

    #[test]
    fn store_creates_blobs_dir() {
        let dir = tempfile::tempdir().unwrap();
        let blobs_dir = dir.path().join("blobs");
        assert!(!blobs_dir.exists());

        store_blob(dir.path(), "aabbccdd", b"wasm").unwrap();

        assert!(blobs_dir.exists());
    }

    #[test]
    fn round_trip_with_real_hash() {
        let dir = tempfile::tempdir().unwrap();
        let source = "let x = 42;";
        let cargo = "[dependencies]";
        let hash = content_hash(source, cargo);
        let blob = vec![0u8; 256];

        store_blob(dir.path(), &hash, &blob).unwrap();

        let hit = try_cache_hit(dir.path(), &hash).unwrap();
        assert_eq!(hit, blob);
    }
}
