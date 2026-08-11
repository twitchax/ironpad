//! The compile cache's tiers: what they are, how big they are, and how to
//! clear one (PRD-0063 T-003).
//!
//! Lives here rather than in the server binary because two callers need to
//! agree about it: the boot-time pressure valve (`ironpad-server`'s
//! `cache_valve`) and the admin panel's manual controls. When each owned its
//! own list of directory names, "a tier" meant whatever the nearest string
//! literal said.
//!
//! Tiers are an enum, not a name. An admin server fn taking a `&str` and
//! joining it onto the cache path is a directory traversal waiting to be
//! written; there is no spelling of [`CacheTier`] that escapes the cache
//! directory.

use std::path::{Path, PathBuf};

/// One directory under the cache root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CacheTier {
    /// cargo target dirs for cell builds. The largest tier by far (3.2GB in
    /// production) and pure derived state.
    Targets,
    /// Scaffolded micro-crates. Rebuilt per compile.
    Workspaces,
    /// The shared cargo registry. Rebuildable, but re-downloading it makes
    /// every first compile slow, so the valve only reaches for it under
    /// sustained pressure.
    CargoHome,
    /// Compiled cell blobs, keyed by content hash. **Not rebuildable without
    /// recompiling**, and what stands between a reader and a cold compile, so
    /// the automatic valve never touches it.
    Blobs,
}

impl CacheTier {
    /// Every tier, coarsest and most disposable first.
    pub const ALL: [Self; 4] = [
        Self::Targets,
        Self::Workspaces,
        Self::CargoHome,
        Self::Blobs,
    ];

    /// The directory name under the cache root.
    #[must_use]
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Targets => "targets",
            Self::Workspaces => "workspaces",
            Self::CargoHome => "cargo-home",
            Self::Blobs => "blobs",
        }
    }

    /// Whether the automatic pressure valve may clear this tier.
    ///
    /// [`Self::Blobs`] is excluded: the valve runs unattended at boot, and
    /// losing compiled output silently turns every subsequent page view into a
    /// cold compile. An administrator can still clear it deliberately.
    #[must_use]
    pub fn valve_may_clear(self) -> bool {
        !matches!(self, Self::Blobs)
    }

    /// Path of this tier under `cache_dir`.
    #[must_use]
    pub fn path(self, cache_dir: &Path) -> PathBuf {
        cache_dir.join(self.dir_name())
    }
}

/// Total bytes under a tier, or 0 when it does not exist.
///
/// Walks the tree rather than calling out to `du`: the caller is a request
/// handler, and shelling out from one is both slower and a dependency on the
/// image having the binary.
#[must_use]
pub fn tier_bytes(cache_dir: &Path, tier: CacheTier) -> u64 {
    fn walk(dir: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path()),
                Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => 0,
            })
            .sum()
    }
    walk(&tier.path(cache_dir))
}

/// Remove a tier, returning the bytes it held.
///
/// Never fails: a missing directory is success, and any other error is logged
/// rather than propagated. The boot valve must not be able to prevent startup,
/// and the admin panel would rather report a partial clear than a 500.
pub fn clear_tier(cache_dir: &Path, tier: CacheTier) -> u64 {
    let dir = tier.path(cache_dir);
    let before = tier_bytes(cache_dir, tier);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {
            tracing::info!(dir = %dir.display(), bytes = before, "cleared cache tier");
            before
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "failed to clear cache tier");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(root: &Path, tier: CacheTier, bytes: usize) {
        let dir = tier.path(root).join("nested");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.bin"), vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn tier_names_are_stable() {
        // These name real directories on a live volume; renaming one silently
        // orphans gigabytes that nothing will ever clear again.
        assert_eq!(CacheTier::Targets.dir_name(), "targets");
        assert_eq!(CacheTier::Workspaces.dir_name(), "workspaces");
        assert_eq!(CacheTier::CargoHome.dir_name(), "cargo-home");
        assert_eq!(CacheTier::Blobs.dir_name(), "blobs");
    }

    #[test]
    fn the_valve_may_never_clear_blobs() {
        assert!(!CacheTier::Blobs.valve_may_clear());
        for tier in CacheTier::ALL {
            if tier != CacheTier::Blobs {
                assert!(tier.valve_may_clear(), "{tier:?} should be valve-clearable");
            }
        }
    }

    #[test]
    fn clearing_one_tier_leaves_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed(root, CacheTier::Targets, 2048);
        seed(root, CacheTier::Blobs, 1024);

        let freed = clear_tier(root, CacheTier::Targets);
        assert!(freed >= 2048, "reported bytes freed: {freed}");
        assert_eq!(tier_bytes(root, CacheTier::Targets), 0);
        assert!(
            tier_bytes(root, CacheTier::Blobs) >= 1024,
            "an unrelated tier must survive"
        );
    }

    #[test]
    fn clearing_an_absent_tier_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(clear_tier(dir.path(), CacheTier::Workspaces), 0);
        assert_eq!(tier_bytes(dir.path(), CacheTier::Workspaces), 0);
    }

    #[test]
    fn a_tier_path_stays_under_the_cache_root() {
        // The reason tiers are an enum: there is no value here that can walk
        // out of the cache directory the way a caller-supplied name could.
        let root = Path::new("/var/cache/ironpad");
        for tier in CacheTier::ALL {
            let p = tier.path(root);
            assert!(p.starts_with(root), "{p:?} escaped {root:?}");
            assert_eq!(p.components().count(), root.components().count() + 1);
        }
    }
}
