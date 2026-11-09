//! Blake3 content-hash caching for compiled WASM blobs.
//!
//! Stores and retrieves compiled blobs (plus JS glue and diagnostics) under
//! `{cache_dir}/blobs/{hash}.*`; the key recipe is
//! `ironpad_common::cache_key` (see [`content_hash`]).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::build::MAX_CELL_BLOB_BYTES;
use super::toolchain::toolchain_fingerprint;

// ── Public API ───────────────────────────────────────────────────────────────

/// Compute a deterministic blake3 content hash from cell source, Cargo.toml,
/// and predecessor cell type tags.
///
/// Thin wrapper over the shared recipe in [`ironpad_common::cache_key`]
/// (which the browser also uses for its local blob store, PRD-0047), bound to
/// the process-cached toolchain fingerprint (rustc version + host wasm-bindgen
/// CLI version, see [`toolchain_fingerprint`]) so a toolchain upgrade
/// invalidates stale cached blobs. `CACHE_EPOCH` lives with the recipe —
/// bump it in `ironpad-common/src/cache_key.rs`.
///
/// `target` must be the target the cell will actually be built for: it is part
/// of the key precisely so a Code cell and a Linux cell with identical source
/// cannot serve each other's blobs. The feature flags are detected by the
/// recipe itself, from these same inputs.
pub fn content_hash(
    source: &str,
    cargo_toml: &str,
    previous_types: &[String],
    shared_cargo_toml: Option<&str>,
    shared_source: Option<&str>,
    target: ironpad_common::cache_key::CellTarget,
) -> String {
    ironpad_common::cache_key::content_hash_with_fingerprint(
        source,
        cargo_toml,
        previous_types,
        shared_cargo_toml,
        shared_source,
        target,
        toolchain_fingerprint(),
    )
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

/// Upper bound on `{cache_dir}/blobs/` (review bug-server-4).
///
/// The boot valve deliberately never clears this tier, and `compile_cell` is
/// anonymous, so without a write-time bound it only ever grew: each
/// `CACHE_EPOCH` or toolchain bump strands a generation of unreachable
/// entries, and a client can buy new entries at the build rate limit. Share
/// snapshots are separate copies under the data dir, so evicting here only
/// ever costs a recompile.
const MAX_BLOB_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// What an eviction trims the tier down to: a low-water mark, so a cache
/// sitting at the cap does not rescan the directory on every store.
const fn evict_blobs_to(max: u64) -> u64 {
    max / 4 * 3
}

/// A hit on an entry whose mtime is older than this refreshes it, so the
/// eviction order (oldest mtime first) means least recently USED rather than
/// least recently built. A day keeps the hot path at most one metadata write
/// per entry per day.
const REFRESH_MTIME_AFTER: Duration = Duration::from_hours(24);

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
///
/// Async fs: this is the compile hot path (a repeat Run of an unchanged cell),
/// and a whole blob read must not park a runtime worker.
#[tracing::instrument(name = "cache_lookup", level = "info", skip_all, fields(hash = %hash, hit = tracing::field::Empty))]
pub async fn try_cache_hit(cache_dir: &Path, hash: &str) -> Option<CacheHit> {
    let path = cache_blob_path(cache_dir, hash);

    // Stat before reading (review bug-security-13): a hit is served without
    // a build token and without the per-cell lock, so an oversized entry
    // (one cached before the build-side cap existed) would be an unmetered
    // allocation per request. Refusing it costs one recompile, which the
    // build-side cap then fails cleanly.
    let modified = match tokio::fs::metadata(&path).await {
        Ok(meta) if meta.len() > MAX_CELL_BLOB_BYTES => {
            tracing::Span::current().record("hit", false);
            tracing::warn!(
                hash,
                bytes = meta.len(),
                "cached blob is over the size cap — treating as miss"
            );
            return None;
        }
        Ok(meta) => meta.modified().ok(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::Span::current().record("hit", false);
            tracing::info!(hash, "cache miss");
            return None;
        }
        Err(e) => {
            tracing::Span::current().record("hit", false);
            tracing::warn!(hash, error = %e, "cache stat error — treating as miss");
            return None;
        }
    };

    let wasm_bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::Span::current().record("hit", false);
            tracing::info!(hash, "cache miss");
            return None;
        }
        Err(e) => {
            tracing::Span::current().record("hit", false);
            tracing::warn!(hash, error = %e, "cache read error — treating as miss");
            return None;
        }
    };
    tracing::Span::current().record("hit", true);

    // JS glue is optional — older cache entries may not have it.
    let js_glue_path = cache_js_glue_path(cache_dir, hash);
    let js_glue = tokio::fs::read_to_string(&js_glue_path).await.ok();

    // Diagnostics are optional too (older entries, or a clean compile). A
    // malformed file is treated as "no diagnostics" rather than a cache miss.
    let diagnostics = tokio::fs::read(cache_diag_path(cache_dir, hash))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();

    tracing::info!(
        hash,
        wasm_bytes = wasm_bytes.len(),
        has_js_glue = js_glue.is_some(),
        "cache hit"
    );

    refresh_stale_mtime(&path, modified).await;

    Some(CacheHit {
        wasm_bytes,
        js_glue,
        diagnostics,
    })
}

/// Bump a hit entry's mtime to now when it is older than
/// [`REFRESH_MTIME_AFTER`], so eviction ages entries by last use. Best
/// effort: a failure only means the entry ages from its build time.
async fn refresh_stale_mtime(path: &Path, modified: Option<SystemTime>) {
    let stale = modified
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .is_some_and(|age| age > REFRESH_MTIME_AFTER);
    if !stale {
        return;
    }
    let path = path.to_path_buf();
    let touched = tokio::task::spawn_blocking(move || {
        std::fs::File::options()
            .write(true)
            .open(&path)?
            .set_modified(SystemTime::now())
    })
    .await;
    if let Ok(Err(e)) = touched {
        tracing::debug!(error = %e, "could not refresh a cache entry's mtime");
    }
}

/// A uniquely-named temp sibling of `path`, for the write-then-rename
/// discipline below.
fn temp_sibling(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp.{}", uuid::Uuid::new_v4()));
    PathBuf::from(tmp)
}

/// Move a fully written `tmp` into place at `path`, removing `tmp` if the
/// rename fails so no `.tmp.` sidecar survives to be mistaken for an entry.
async fn rename_into_place(tmp: &Path, path: &Path) -> anyhow::Result<()> {
    if let Err(e) = tokio::fs::rename(tmp, path).await {
        let _ = tokio::fs::remove_file(tmp).await; // best-effort cleanup
        return Err(anyhow::anyhow!(
            "failed to rename into {}: {e}",
            path.display()
        ));
    }
    Ok(())
}

/// Write `contents` to `path` atomically: write a uniquely-named temp sibling,
/// then rename it into place. `rename` is atomic on the same filesystem, so a
/// concurrent reader ([`try_cache_hit`], the `/share-blobs/` route, another
/// in-flight share) sees either the old file or the fully written new one —
/// never the truncated partial write a plain write exposes.
///
/// The ONE implementation of that discipline: the compile cache and the
/// share store both write through it.
pub(crate) async fn atomic_write(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let tmp = temp_sibling(path);
    tokio::fs::write(&tmp, contents)
        .await
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", tmp.display()))?;
    rename_into_place(&tmp, path).await
}

/// Copy `src` to `dst` with the same temp-sibling-and-rename discipline as
/// [`atomic_write`], without buffering the file in memory.
pub(crate) async fn atomic_copy(src: &Path, dst: &Path) -> anyhow::Result<()> {
    let tmp = temp_sibling(dst);
    if let Err(e) = tokio::fs::copy(src, &tmp).await {
        let _ = tokio::fs::remove_file(&tmp).await; // a partial copy may exist
        return Err(anyhow::anyhow!(
            "failed to copy {} to {}: {e}",
            src.display(),
            tmp.display()
        ));
    }
    rename_into_place(&tmp, dst).await
}

/// Store a compiled WASM blob (plus optional JS glue and any diagnostics) in
/// the cache.
///
/// Creates the `blobs/` directory if it doesn't already exist. Diagnostics
/// (warnings from a successful compile) are cached so they survive a cache hit.
/// The tier is bounded at [`MAX_BLOB_CACHE_BYTES`]; see [`store_blob_capped`].
pub async fn store_blob(
    cache_dir: &Path,
    hash: &str,
    wasm_bytes: &[u8],
    js_glue: Option<&str>,
    diagnostics: &[ironpad_common::Diagnostic],
) -> anyhow::Result<()> {
    store_blob_capped(
        cache_dir,
        hash,
        wasm_bytes,
        js_glue,
        diagnostics,
        MAX_BLOB_CACHE_BYTES,
    )
    .await
}

/// [`store_blob`] with an explicit cap on the `blobs/` tier, so tests can
/// exercise eviction without writing gigabytes. Evicts BEFORE writing, and
/// only here: a store runs only on the miss path, after a build, so the scan
/// never lands on the cache-hit hot path.
#[tracing::instrument(name = "cache_store", level = "info", skip_all, fields(hash = %hash, bytes = wasm_bytes.len()))]
async fn store_blob_capped(
    cache_dir: &Path,
    hash: &str,
    wasm_bytes: &[u8],
    js_glue: Option<&str>,
    diagnostics: &[ironpad_common::Diagnostic],
    max_bytes: u64,
) -> anyhow::Result<()> {
    let path = cache_blob_path(cache_dir, hash);

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
        let incoming = wasm_bytes.len() + js_glue.map_or(0, str::len);
        if let Err(e) = evict_blobs_if_needed(parent, incoming as u64, max_bytes).await {
            tracing::warn!(error = %e, "compile cache eviction failed; storing anyway");
        }
    }

    atomic_write(&path, wasm_bytes).await?;

    if let Some(glue) = js_glue {
        let js_path = cache_js_glue_path(cache_dir, hash);
        atomic_write(&js_path, glue.as_bytes()).await?;
        tracing::info!(hash, js_bytes = glue.len(), "cached JS glue");
    }

    // Only write a diagnostics file when there's something to remember.
    if !diagnostics.is_empty() {
        let json = serde_json::to_vec(diagnostics)?;
        atomic_write(&cache_diag_path(cache_dir, hash), &json).await?;
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

/// One cache entry's files, grouped by hash so they are evicted together.
#[derive(Default)]
struct BlobEntry {
    bytes: u64,
    /// The `.wasm`'s mtime, which ages the whole entry. `None` for an orphan
    /// (glue or diagnostics with no blob), which nothing can hit, so it
    /// sorts first.
    modified: Option<SystemTime>,
    wasm: Option<PathBuf>,
    /// Glue and diagnostics: removed only after the `.wasm`.
    siblings: Vec<PathBuf>,
}

/// Split a `blobs/` file name into its hash and whether it is the `.wasm`.
/// `None` for anything that is not a cache entry file.
fn split_blob_name(name: &str) -> Option<(&str, bool)> {
    if let Some(hash) = name.strip_suffix(".wasm") {
        return Some((hash, true));
    }
    name.strip_suffix(".diag.json")
        .or_else(|| name.strip_suffix(".js"))
        .map(|hash| (hash, false))
}

/// Delete whole cache entries, oldest `.wasm` mtime first, until the tier
/// plus `incoming` fits under [`evict_blobs_to`]`(max_bytes)`. A no-op while
/// it fits under `max_bytes`.
///
/// Modeled on the OG card cache's eviction, with two differences that
/// matter here. Entries are deleted WHOLE and the `.wasm` goes FIRST:
/// [`try_cache_hit`] reads a missing `.js` as an older glue-less entry, so a
/// `.wasm` that outlived its glue would serve a blob with `js_glue: None`.
/// And names containing `.tmp.` are skipped, because they are in-flight
/// [`atomic_write`]s that are about to be renamed into place.
async fn evict_blobs_if_needed(
    blobs_dir: &Path,
    incoming: u64,
    max_bytes: u64,
) -> anyhow::Result<()> {
    let mut read_dir = match tokio::fs::read_dir(blobs_dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(anyhow::anyhow!(
                "failed to read {}: {e}",
                blobs_dir.display()
            ))
        }
    };

    let mut entries: std::collections::HashMap<String, BlobEntry> =
        std::collections::HashMap::new();
    let mut total: u64 = 0;
    while let Some(dirent) = read_dir.next_entry().await? {
        let name = dirent.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.contains(".tmp.") {
            continue;
        }
        let Some((hash, is_wasm)) = split_blob_name(name) else {
            continue;
        };
        let Ok(meta) = dirent.metadata().await else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        total = total.saturating_add(meta.len());
        let entry = entries.entry(hash.to_owned()).or_default();
        entry.bytes = entry.bytes.saturating_add(meta.len());
        if is_wasm {
            entry.modified = meta.modified().ok();
            entry.wasm = Some(dirent.path());
        } else {
            entry.siblings.push(dirent.path());
        }
    }

    if total.saturating_add(incoming) <= max_bytes {
        return Ok(());
    }

    let low_water = evict_blobs_to(max_bytes);
    let mut oldest_first: Vec<BlobEntry> = entries.into_values().collect();
    oldest_first.sort_by_key(|e| e.modified);

    let mut removed = 0_usize;
    for entry in oldest_first {
        if total.saturating_add(incoming) <= low_water {
            break;
        }
        for file in entry.wasm.iter().chain(&entry.siblings) {
            match tokio::fs::remove_file(file).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(file = %file.display(), error = %e, "cache eviction could not remove a file");
                }
            }
        }
        total = total.saturating_sub(entry.bytes);
        removed += 1;
    }

    tracing::info!(
        removed,
        remaining_bytes = total,
        "evicted compile cache entries to stay under the cache cap"
    );
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ironpad_common::cache_key::CellTarget;

    // ── content_hash ────────────────────────────────────────────────────

    #[test]
    fn hash_is_deterministic() {
        let a = content_hash(
            "fn main() {}",
            "[dependencies]",
            &[],
            None,
            None,
            CellTarget::Executor,
        );
        let b = content_hash(
            "fn main() {}",
            "[dependencies]",
            &[],
            None,
            None,
            CellTarget::Executor,
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
            CellTarget::Executor,
        );
        let b = content_hash(
            "fn main() { 2 }",
            "[dependencies]",
            &[],
            None,
            None,
            CellTarget::Executor,
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
            CellTarget::Executor,
        );
        let b = content_hash(
            source,
            r#"[dependencies]\nrand = "0.8""#,
            &[],
            None,
            None,
            CellTarget::Executor,
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
        let a = content_hash("ab", "c", &[], None, None, CellTarget::Executor);
        let b = content_hash("a", "bc", &[], None, None, CellTarget::Executor);
        assert_ne!(a, b, "source/cargo_toml boundary must be unambiguous");

        // cargo_toml/target boundary: the target triple is hashed right after
        // cargo_toml, so a cargo_toml ending in those bytes could shift the
        // split. ("", "x" + TARGET) vs ("x", TARGET) collided before framing.
        let target = CellTarget::Executor.triple();
        let c = content_hash(
            "",
            &format!("x{target}"),
            &[],
            None,
            None,
            CellTarget::Executor,
        );
        let d = content_hash("x", target, &[], None, None, CellTarget::Executor);
        assert_ne!(c, d, "cargo_toml/target boundary must be unambiguous");
    }

    #[test]
    fn hash_is_64_hex_chars() {
        let h = content_hash("x", "y", &[], None, None, CellTarget::Executor);
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    // Single-char names are idiomatic for hash-comparison test fixtures.
    #[allow(clippy::many_single_char_names)]
    fn hash_changes_when_previous_types_change() {
        // The slot must be REFERENCED for its type to reach the key
        // (PRD-0060 normalization).
        let s = "let x = cell0;";
        let c = "[dependencies]";
        let a = content_hash(s, c, &[], None, None, CellTarget::Executor);
        let b = content_hash(s, c, &["u32".into()], None, None, CellTarget::Executor);
        let d = content_hash(s, c, &["String".into()], None, None, CellTarget::Executor);
        assert_ne!(a, b);
        assert_ne!(b, d);
    }

    #[test]
    fn hash_distinguishes_type_positions() {
        // Both slots referenced, so both positions survive normalization.
        let s = "cell0 + cell1";
        let c = "y";
        let a = content_hash(
            s,
            c,
            &["u32".into(), String::new()],
            None,
            None,
            CellTarget::Executor,
        );
        let b = content_hash(
            s,
            c,
            &[String::new(), "u32".into()],
            None,
            None,
            CellTarget::Executor,
        );
        assert_ne!(a, b);
    }

    #[test]
    // Single-char names are idiomatic for hash-comparison test fixtures.
    #[allow(clippy::many_single_char_names)]
    fn hash_changes_when_shared_cargo_toml_changes() {
        let s = "let x = 1;";
        let c = "[dependencies]";
        let a = content_hash(s, c, &[], None, None, CellTarget::Executor);
        let b = content_hash(
            s,
            c,
            &[],
            Some("[dependencies]\nserde = \"1\""),
            None,
            CellTarget::Executor,
        );
        let d = content_hash(
            s,
            c,
            &[],
            Some("[dependencies]\nrand = \"0.8\""),
            None,
            CellTarget::Executor,
        );
        assert_ne!(a, b);
        assert_ne!(b, d);
    }

    #[test]
    fn hash_with_none_shared_differs_from_empty_shared() {
        let s = "x";
        let c = "y";
        let a = content_hash(s, c, &[], None, None, CellTarget::Executor);
        let b = content_hash(s, c, &[], Some(""), None, CellTarget::Executor);
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

    // ── try_cache_hit / store_blob (integration).await ────────────────────────

    #[tokio::test]
    async fn miss_on_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(try_cache_hit(dir.path(), "nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn store_and_hit() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "deadbeef01234567deadbeef01234567deadbeef01234567deadbeef01234567";
        let blob = b"\x00asm\x01\x00\x00\x00";

        store_blob(dir.path(), hash, blob, None, &[]).await.unwrap();

        let hit = try_cache_hit(dir.path(), hash).await;
        assert!(hit.is_some());
        let hit = hit.unwrap();
        assert_eq!(hit.wasm_bytes, blob);
        assert!(hit.js_glue.is_none());
    }

    #[tokio::test]
    async fn store_and_hit_with_js_glue() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "deadbeef01234567deadbeef01234567deadbeef01234567deadbeef01234567";
        let blob = b"\x00asm\x01\x00\x00\x00";
        let glue = "export function init() {}";

        store_blob(dir.path(), hash, blob, Some(glue), &[])
            .await
            .unwrap();

        let hit = try_cache_hit(dir.path(), hash).await.unwrap();
        assert_eq!(hit.wasm_bytes, blob);
        assert_eq!(hit.js_glue.as_deref(), Some(glue));
    }

    #[tokio::test]
    async fn store_creates_blobs_dir() {
        let dir = tempfile::tempdir().unwrap();
        let blobs_dir = dir.path().join("blobs");
        assert!(!blobs_dir.exists());

        store_blob(dir.path(), "aabbccdd", b"wasm", None, &[])
            .await
            .unwrap();

        assert!(blobs_dir.exists());
    }

    #[tokio::test]
    async fn round_trip_with_real_hash() {
        let dir = tempfile::tempdir().unwrap();
        let source = "let x = 42;";
        let cargo = "[dependencies]";
        let hash = content_hash(source, cargo, &[], None, None, CellTarget::Executor);
        let blob = vec![0u8; 256];
        let glue = "// js glue content";

        store_blob(dir.path(), &hash, &blob, Some(glue), &[])
            .await
            .unwrap();

        let hit = try_cache_hit(dir.path(), &hash).await.unwrap();
        assert_eq!(hit.wasm_bytes, blob);
        assert_eq!(hit.js_glue.as_deref(), Some(glue));
    }

    // ── T-005: Additional edge-case tests ───────────────────────────────

    #[test]
    fn hash_empty_source_is_valid() {
        let h = content_hash("", "", &[], None, None, CellTarget::Executor);
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_same_shared_cargo_toml_is_deterministic() {
        let shared = "[dependencies]\nserde = \"1\"";
        let a = content_hash("x", "y", &[], Some(shared), None, CellTarget::Executor);
        let b = content_hash("x", "y", &[], Some(shared), None, CellTarget::Executor);
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn store_overwrites_existing_blob() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "aabbccdd";
        let blob_v1 = b"version-1";
        let blob_v2 = b"version-2-longer";

        store_blob(dir.path(), hash, blob_v1, None, &[])
            .await
            .unwrap();
        let hit1 = try_cache_hit(dir.path(), hash).await.unwrap();
        assert_eq!(hit1.wasm_bytes, blob_v1);

        store_blob(dir.path(), hash, blob_v2, None, &[])
            .await
            .unwrap();
        let hit2 = try_cache_hit(dir.path(), hash).await.unwrap();
        assert_eq!(hit2.wasm_bytes, blob_v2);
    }

    #[tokio::test]
    async fn store_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        store_blob(dir.path(), "deadbeef", b"wasm", Some("glue()"), &[])
            .await
            .unwrap();

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

    /// The cleanup arm: a rename that fails must not leave its `.tmp.`
    /// sidecar behind (review compiler-9). A directory at the destination
    /// makes the rename fail on every platform we run on.
    #[tokio::test]
    async fn a_failed_rename_leaves_no_temp_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        let src = dir.path().join("src.wasm");
        std::fs::write(&src, b"wasm").unwrap();

        assert!(atomic_write(&blocked, b"wasm").await.is_err());
        assert!(atomic_copy(&src, &blocked).await.is_err());

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
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

    #[tokio::test]
    async fn atomic_copy_copies_the_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.wasm");
        let dst = dir.path().join("dst.wasm");
        std::fs::write(&src, b"\x00asm-bytes").unwrap();

        atomic_copy(&src, &dst).await.unwrap();

        assert_eq!(std::fs::read(&dst).unwrap(), b"\x00asm-bytes");
    }

    #[tokio::test]
    async fn cache_hit_without_js_glue_then_with() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "test1234";

        // Store without JS glue.
        store_blob(dir.path(), hash, b"wasm", None, &[])
            .await
            .unwrap();
        let hit = try_cache_hit(dir.path(), hash).await.unwrap();
        assert!(hit.js_glue.is_none());

        // Store again with JS glue.
        store_blob(dir.path(), hash, b"wasm", Some("glue()"), &[])
            .await
            .unwrap();
        let hit = try_cache_hit(dir.path(), hash).await.unwrap();
        assert_eq!(hit.js_glue.as_deref(), Some("glue()"));
    }

    #[tokio::test]
    async fn diagnostics_survive_cache_round_trip() {
        use ironpad_common::{Diagnostic, Severity};
        let dir = tempfile::tempdir().unwrap();
        let hash = "cafef00d";
        let diags = vec![Diagnostic {
            message: "unused variable: `x`".into(),
            severity: Severity::Warning,
            spans: vec![],
            code: Some("unused_variables".into()),
        }];

        store_blob(dir.path(), hash, b"wasm", Some("glue()"), &diags)
            .await
            .unwrap();
        let hit = try_cache_hit(dir.path(), hash).await.unwrap();

        assert_eq!(hit.diagnostics.len(), 1, "warnings survive a cache hit");
        assert_eq!(hit.diagnostics[0].message, "unused variable: `x`");
        assert_eq!(hit.diagnostics[0].severity, Severity::Warning);
    }

    /// A hit is served with no build token and no per-cell lock, so an entry
    /// over the blob cap must not be read at all (review bug-security-13).
    /// The file is sparse: over the cap on disk, nothing allocated.
    #[tokio::test]
    async fn an_oversized_entry_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "0versized";
        store_blob(dir.path(), hash, b"wasm", Some("glue()"), &[])
            .await
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(cache_blob_path(dir.path(), hash))
            .unwrap()
            .set_len(MAX_CELL_BLOB_BYTES + 1)
            .unwrap();

        assert!(try_cache_hit(dir.path(), hash).await.is_none());
    }

    #[tokio::test]
    async fn empty_diagnostics_writes_no_file_and_reads_empty() {
        let dir = tempfile::tempdir().unwrap();
        store_blob(dir.path(), "beefbeef", b"wasm", None, &[])
            .await
            .unwrap();
        assert!(
            !cache_diag_path(dir.path(), "beefbeef").exists(),
            "no diag file when there are no diagnostics"
        );
        let hit = try_cache_hit(dir.path(), "beefbeef").await.unwrap();
        assert!(hit.diagnostics.is_empty());
    }

    /// Back-date a file's mtime by `age`.
    fn age_file(path: &Path, age: Duration) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - age)
            .unwrap();
    }

    fn hours(n: u64) -> Duration {
        Duration::from_hours(n)
    }

    /// The tier is bounded at write time (review bug-server-4,
    /// bug-security-13): past the cap the OLDEST entries go, whole, and the
    /// newest survives. An in-flight `.tmp.` write is never touched.
    #[tokio::test]
    async fn store_blob_capped_evicts_oldest_groups_past_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = dir.path().join("blobs");
        let diag = vec![ironpad_common::Diagnostic {
            message: "warning".into(),
            severity: ironpad_common::Severity::Warning,
            spans: vec![],
            code: None,
        }];
        // Three 110-byte entries (100-byte blob + 10-byte glue); only the
        // oldest also carries a diagnostics file.
        for (hash, age) in [("old", 72), ("mid", 48), ("new", 1)] {
            let diagnostics = if hash == "old" { diag.as_slice() } else { &[] };
            store_blob(dir.path(), hash, &[1; 100], Some("0123456789"), diagnostics)
                .await
                .unwrap();
            age_file(&blobs.join(format!("{hash}.wasm")), hours(age));
        }
        let in_flight = blobs.join("zzz.wasm.tmp.1234");
        std::fs::write(&in_flight, [0; 500]).unwrap();

        // Below the cap: a store evicts nothing.
        store_blob_capped(dir.path(), "fits", &[2; 10], None, &[], 10_000)
            .await
            .unwrap();
        for hash in ["old", "mid", "new", "fits"] {
            assert!(blobs.join(format!("{hash}.wasm")).exists(), "{hash} kept");
        }

        // 350 bytes plus the diagnostics file are stored and 100 more are
        // incoming, so a 460 cap is exceeded only by the diagnostics, and its
        // 345 low-water mark is met by dropping exactly the oldest entry.
        let diag_len = std::fs::metadata(blobs.join("old.diag.json"))
            .unwrap()
            .len();
        assert!(diag_len > 10, "the arithmetic above needs a real diag file");
        store_blob_capped(dir.path(), "incoming", &[3; 100], None, &[], 460)
            .await
            .unwrap();

        for gone in ["old.wasm", "old.js", "old.diag.json"] {
            assert!(!blobs.join(gone).exists(), "{gone} must be evicted whole");
        }
        for kept in ["mid.wasm", "mid.js", "new.wasm", "new.js", "incoming.wasm"] {
            assert!(blobs.join(kept).exists(), "{kept} must survive");
        }
        assert!(
            in_flight.exists(),
            "an in-flight temp file is never evicted"
        );
    }

    /// A hit refreshes a stale mtime, so eviction's age means unused rather
    /// than old (review bug-server-4). A recent one is left alone, which
    /// keeps the hot path free of a write per hit.
    #[tokio::test]
    async fn a_cache_hit_refreshes_a_stale_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let mtime = |hash: &str| {
            std::fs::metadata(cache_blob_path(dir.path(), hash))
                .unwrap()
                .modified()
                .unwrap()
        };

        store_blob(dir.path(), "stale", b"wasm", None, &[])
            .await
            .unwrap();
        age_file(&cache_blob_path(dir.path(), "stale"), hours(48));
        assert!(try_cache_hit(dir.path(), "stale").await.is_some());
        let age = SystemTime::now().duration_since(mtime("stale")).unwrap();
        assert!(age < hours(1), "a two-day-old hit is refreshed: {age:?}");

        store_blob(dir.path(), "fresh", b"wasm", None, &[])
            .await
            .unwrap();
        age_file(&cache_blob_path(dir.path(), "fresh"), hours(1));
        let before = mtime("fresh");
        assert!(try_cache_hit(dir.path(), "fresh").await.is_some());
        assert_eq!(mtime("fresh"), before, "an hour-old hit is not rewritten");
    }

    // Fingerprint-variation tests live with the shared recipe in
    // `ironpad-common/src/cache_key.rs`; the tests above exercise the
    // delegating entry point with the process fingerprint.
}
