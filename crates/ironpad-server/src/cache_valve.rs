//! Boot-time cache pressure valve: probe the cache filesystem and clear
//! cache tiers coarsest-first when usage crosses the high-water mark.
//!
//! Split out of `main.rs` (PRD-0055 T-003); behavior unchanged.

/// Maximum percentage of the cache filesystem that may be in use before the
/// startup pressure valve clears the rebuildable caches. The compile cache
/// shares its volume with the share store, and a full disk fails cell
/// compiles AND share writes — trading cold rebuilds for headroom is always
/// the right side of that bargain.
const CACHE_PRESSURE_MAX_USED_PCT: u8 = 80;

/// Absolute free-space floor for the pressure valve: a volume with at least
/// this much headroom is not under pressure no matter what the percentage
/// says. Percentage alone misfires on big disks — a dev box at 86% of 3TB
/// still has hundreds of GB free, and wiping its caches on every server
/// start makes the first live check of each e2e run minutes-cold. On the
/// 5GB Fly volume, available space can never reach this floor, so prod
/// behavior is decided by the percentage exactly as before.
const CACHE_PRESSURE_MIN_FREE_BYTES: u64 = 20 * 1024 * 1024 * 1024;

/// Usage of the filesystem holding the cache dir.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FsUsage {
    used_pct: u8,
    available_bytes: u64,
}

impl FsUsage {
    /// Under pressure only when the volume is BOTH proportionally full and
    /// short on absolute headroom.
    fn under_pressure(self) -> bool {
        self.used_pct >= CACHE_PRESSURE_MAX_USED_PCT
            && self.available_bytes < CACHE_PRESSURE_MIN_FREE_BYTES
    }
}

/// Usage of the filesystem holding `path`, or `None` when it can't be
/// measured (non-unix, or `statvfs` failure).
#[cfg(unix)]
pub(crate) fn fs_usage(path: &std::path::Path) -> Option<FsUsage> {
    use std::os::unix::ffi::OsStrExt as _;

    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid NUL-terminated path and `stat` is a valid
    // out-pointer for the duration of the call.
    if unsafe { libc::statvfs(c_path.as_ptr(), &raw mut stat) } != 0 {
        return None;
    }
    if stat.f_blocks == 0 {
        return None;
    }
    let used = u128::from(stat.f_blocks.saturating_sub(stat.f_bavail));
    let pct = used * 100 / u128::from(stat.f_blocks);
    let available = u64::try_from(u128::from(stat.f_bavail) * u128::from(stat.f_frsize)).ok()?;
    Some(FsUsage {
        used_pct: u8::try_from(pct).ok()?,
        available_bytes: available,
    })
}

#[cfg(not(unix))]
pub(crate) fn fs_usage(_path: &std::path::Path) -> Option<FsUsage> {
    None
}

/// Disk-pressure valve for the compile cache: when the cache volume is at or
/// above [`CACHE_PRESSURE_MAX_USED_PCT`], clear the rebuildable caches in two
/// tiers — `targets/` + `workspaces/` (pure compile-speed caches) first, then
/// `cargo-home/` (crates.io registry cache) only if pressure persists.
///
/// `blobs/` is never touched: it holds the content-addressed compiled cells,
/// so unchanged cells stay warm across a wipe and only the next *novel*
/// compile pays a cold build.
///
/// `usage_probe` measures the volume's usage; it is called once up front
/// and again after the first tier to decide on escalation (injected so tests
/// can drive both decisions without a real full disk).
pub(crate) fn cache_pressure_valve(
    cache_dir: &std::path::Path,
    usage_probe: impl Fn() -> Option<FsUsage>,
) {
    let Some(usage) = usage_probe() else {
        tracing::warn!("cache volume usage unmeasurable; pressure valve skipped");
        return;
    };
    if !usage.under_pressure() {
        tracing::info!(
            used_pct = usage.used_pct,
            available_bytes = usage.available_bytes,
            "cache volume below pressure threshold"
        );
        return;
    }

    tracing::warn!(
        used_pct = usage.used_pct,
        available_bytes = usage.available_bytes,
        "cache volume under disk pressure — clearing rebuildable caches"
    );
    clear_cache_tier(cache_dir, &["targets", "workspaces"]);

    // Re-measure: only escalate to the registry cache if still under pressure.
    match usage_probe() {
        Some(still) if still.under_pressure() => {
            tracing::warn!(
                used_pct = still.used_pct,
                "pressure persists — clearing the cargo registry cache too"
            );
            clear_cache_tier(cache_dir, &["cargo-home"]);
        }
        Some(still) => tracing::info!(used_pct = still.used_pct, "pressure relieved"),
        None => {}
    }
}

/// Remove the given subdirectories of `cache_dir`, ignoring ones that don't
/// exist and logging (but not failing on) other errors — the valve must never
/// prevent startup.
fn clear_cache_tier(cache_dir: &std::path::Path, subdirs: &[&str]) {
    for sub in subdirs {
        let dir = cache_dir.join(sub);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => tracing::info!(dir = %dir.display(), "cleared cache tier"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "failed to clear cache tier");
            }
        }
    }
}

#[cfg(test)]
mod cache_pressure_tests {
    use super::{
        cache_pressure_valve, fs_usage, FsUsage, CACHE_PRESSURE_MAX_USED_PCT,
        CACHE_PRESSURE_MIN_FREE_BYTES,
    };

    /// A volume that is proportionally full AND short on headroom.
    fn pressured(used_pct: u8) -> FsUsage {
        FsUsage {
            used_pct,
            available_bytes: 1024 * 1024 * 1024,
        }
    }

    fn seed_cache(root: &std::path::Path) {
        for sub in ["targets/a", "workspaces/b", "blobs", "cargo-home/reg"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        std::fs::write(root.join("targets/a/artifact.rlib"), b"x").unwrap();
        std::fs::write(root.join("blobs/deadbeef.wasm"), b"\0asm").unwrap();
    }

    #[test]
    fn below_threshold_wipes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        seed_cache(tmp.path());
        cache_pressure_valve(tmp.path(), || {
            Some(pressured(CACHE_PRESSURE_MAX_USED_PCT - 1))
        });
        assert!(tmp.path().join("targets/a/artifact.rlib").exists());
        assert!(tmp.path().join("workspaces/b").exists());
    }

    #[test]
    fn at_threshold_wipes_rebuildable_tiers_but_never_blobs() {
        let tmp = tempfile::tempdir().unwrap();
        seed_cache(tmp.path());
        // First measurement: under pressure. Re-measurement: relieved.
        let calls = std::cell::Cell::new(0u8);
        cache_pressure_valve(tmp.path(), || {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Some(pressured(CACHE_PRESSURE_MAX_USED_PCT))
            } else {
                Some(pressured(40))
            }
        });
        assert!(!tmp.path().join("targets").exists());
        assert!(!tmp.path().join("workspaces").exists());
        // The warm-cell cache survives every tier.
        assert!(tmp.path().join("blobs/deadbeef.wasm").exists());
        // Pressure relieved after tier one, so the registry tier is spared.
        assert!(tmp.path().join("cargo-home/reg").exists());
    }

    #[test]
    fn persistent_pressure_escalates_to_the_registry_tier() {
        let tmp = tempfile::tempdir().unwrap();
        seed_cache(tmp.path());
        cache_pressure_valve(tmp.path(), || Some(pressured(95)));
        assert!(!tmp.path().join("targets").exists());
        assert!(!tmp.path().join("cargo-home").exists());
        // Blobs survive even full escalation.
        assert!(tmp.path().join("blobs/deadbeef.wasm").exists());
    }

    #[test]
    fn unmeasurable_usage_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        seed_cache(tmp.path());
        cache_pressure_valve(tmp.path(), || None);
        assert!(tmp.path().join("targets/a/artifact.rlib").exists());
    }

    #[test]
    fn high_percentage_with_ample_headroom_wipes_nothing() {
        // The dev-box case: a big disk past the percentage threshold but with
        // hundreds of GB free is NOT under pressure (the wipe would only slow
        // the next runs down; see CACHE_PRESSURE_MIN_FREE_BYTES).
        let tmp = tempfile::tempdir().unwrap();
        seed_cache(tmp.path());
        cache_pressure_valve(tmp.path(), || {
            Some(FsUsage {
                used_pct: 95,
                available_bytes: CACHE_PRESSURE_MIN_FREE_BYTES * 20,
            })
        });
        assert!(tmp.path().join("targets/a/artifact.rlib").exists());
        assert!(tmp.path().join("cargo-home/reg").exists());
    }

    #[test]
    fn fs_usage_measures_real_filesystems() {
        let tmp = tempfile::tempdir().unwrap();
        let usage = fs_usage(tmp.path()).expect("statvfs should work on a tempdir");
        assert!(usage.used_pct <= 100);
        assert!(usage.available_bytes > 0);
    }
}
