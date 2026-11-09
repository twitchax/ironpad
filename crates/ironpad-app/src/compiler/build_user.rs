//! The unprivileged user cell builds run as (`IRONPAD_BUILD_UID`).
//!
//! The deploy image's server runs as root, and a cell build runs code the
//! cell's author chose: cargo, rustc, build scripts and proc macros. When
//! `IRONPAD_BUILD_UID` names a uid, every cargo process runs as that uid (and
//! the gid of the same number), so a build cannot read the accounts database,
//! cannot read the server's environment through `/proc`, and cannot rewrite
//! the blob cache every reader is served from. Unset (dev boxes, CI), builds
//! run as whoever runs the server, exactly as before.
//!
//! The ownership model is deliberately narrow. The build user owns what cargo
//! writes: `CARGO_HOME`, the target dirs, its own `HOME`, and each
//! micro-crate's `Cargo.lock`. The micro-crate directory itself, its sources
//! and its `wasm-bindgen-out` stay with the server's user, because the server
//! writes into them on every compile and reads blobs back out of them: a
//! directory the build user could write is one it could leave a symlink in
//! for the server to follow. The one artifact the server must read out of the
//! build user's tree is copied out without following links first
//! (`build::take_artifact`).
//!
//! Known residual, recorded in PRD-0030: the target dirs and `CARGO_HOME` are
//! shared by every build, so one build can still read other cells' sources and
//! artifacts and poison a later build's dependencies, and a process a build
//! leaves running keeps the build user's rights.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

/// The environment variable naming the build uid.
pub const BUILD_UID_ENV: &str = "IRONPAD_BUILD_UID";

/// The configured build uid, or `None` when builds run as the server's own
/// user.
///
/// # Errors
///
/// A value that is not a uid. This fails closed on purpose: a typo in the
/// deployment must stop builds, not quietly run every one of them as root.
pub fn configured_uid() -> anyhow::Result<Option<u32>> {
    parse_uid(std::env::var(BUILD_UID_ENV).ok().as_deref())
}

fn parse_uid(value: Option<&str>) -> anyhow::Result<Option<u32>> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(uid) => uid
            .parse()
            .map(Some)
            .with_context(|| format!("{BUILD_UID_ENV}={uid:?} is not a uid")),
    }
}

/// `HOME` for build processes: a directory the build user owns, so tools
/// that write under `$HOME` (`.cache`, `.config`) can.
pub fn home_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("build-home")
}

/// Give `path` itself to the build user: never what it points to, never
/// what it holds. A no-op when no build uid is configured.
///
/// # Errors
///
/// The `lchown` failed.
pub fn chown_for_build(path: &Path, uid: Option<u32>) -> std::io::Result<()> {
    let Some(uid) = uid else {
        return Ok(());
    };
    #[cfg(unix)]
    {
        std::os::unix::fs::lchown(path, Some(uid), Some(uid))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, uid);
        Ok(())
    }
}

/// Configure `cmd` to run as the build user, handing it the directories it
/// writes and a `HOME` of its own. Called after the environment is set up,
/// since it sets `HOME`.
///
/// `writable` are handed over at their top level only. The entrypoint hands
/// each existing tree over once per volume and everything cargo creates
/// inside one is the build user's already, so this only has to cover a
/// directory the server itself just created: a new session's target dir, or
/// any of them after a cache wipe.
///
/// # Errors
///
/// Creating the home directory or handing a directory over failed.
pub async fn run_as(
    cmd: &mut tokio::process::Command,
    uid: u32,
    cache_dir: &Path,
    writable: &[&Path],
) -> anyhow::Result<()> {
    let home = home_dir(cache_dir);
    tokio::fs::create_dir_all(&home).await?;
    for dir in writable.iter().copied().chain([home.as_path()]) {
        chown_for_build(dir, Some(uid))
            .with_context(|| format!("handing {} to the build user", dir.display()))?;
    }
    #[cfg(unix)]
    {
        cmd.uid(uid);
        cmd.gid(uid);
    }
    cmd.env("HOME", &home);
    Ok(())
}

/// Make a micro-crate's `Cargo.lock` the one thing in it the build user can
/// write. A no-op when no build uid is configured.
///
/// cargo needs nothing else in the crate directory: it writes the lockfile
/// and nothing beside it (its artifacts go to `CARGO_TARGET_DIR`). An empty
/// lockfile resolves exactly like a missing one, so a fresh crate gets an
/// empty file to own rather than a writable directory to create one in.
///
/// # Errors
///
/// Creating or handing over the lockfile failed.
pub fn hand_lockfile_to_build(crate_dir: &Path, uid: Option<u32>) -> std::io::Result<()> {
    if uid.is_none() {
        return Ok(());
    }
    let lockfile = crate_dir.join("Cargo.lock");
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lockfile)
    {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    chown_for_build(&lockfile, uid)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::MetadataExt as _;

    use super::*;

    /// The test process's own uid: the only one an unprivileged test can
    /// chown to, which still runs the real `lchown`.
    fn own_uid() -> u32 {
        std::fs::metadata(env!("CARGO_MANIFEST_DIR")).unwrap().uid()
    }

    #[test]
    fn the_uid_is_parsed_and_a_bad_one_fails_closed() {
        assert_eq!(parse_uid(None).unwrap(), None);
        assert_eq!(parse_uid(Some("")).unwrap(), None);
        assert_eq!(parse_uid(Some(" 2000 ")).unwrap(), Some(2000));
        assert!(parse_uid(Some("cellbuild")).is_err());
        assert!(parse_uid(Some("-1")).is_err());
    }

    #[test]
    fn a_fresh_crate_gets_an_empty_lockfile_for_the_build_user() {
        let dir = tempfile::tempdir().unwrap();
        hand_lockfile_to_build(dir.path(), Some(own_uid())).unwrap();
        let meta = std::fs::symlink_metadata(dir.path().join("Cargo.lock")).unwrap();
        assert!(meta.is_file());
        assert_eq!(meta.len(), 0);
        assert_eq!(meta.uid(), own_uid());
    }

    #[test]
    fn an_existing_lockfile_keeps_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        let lockfile = dir.path().join("Cargo.lock");
        std::fs::write(&lockfile, "version = 4\n").unwrap();
        hand_lockfile_to_build(dir.path(), Some(own_uid())).unwrap();
        assert_eq!(std::fs::read_to_string(&lockfile).unwrap(), "version = 4\n");
    }

    #[tokio::test]
    async fn run_as_gives_the_build_its_own_home() {
        let cache = tempfile::tempdir().unwrap();
        let target = cache.path().join("targets/s1");
        std::fs::create_dir_all(&target).unwrap();
        let mut cmd = tokio::process::Command::new("cargo");
        run_as(&mut cmd, own_uid(), cache.path(), &[target.as_path()])
            .await
            .unwrap();

        let home = home_dir(cache.path());
        assert!(home.is_dir());
        assert_eq!(std::fs::metadata(&home).unwrap().uid(), own_uid());
        let home_env = cmd
            .as_std()
            .get_envs()
            .find(|(k, _)| *k == "HOME")
            .and_then(|(_, v)| v)
            .map(PathBuf::from);
        assert_eq!(home_env, Some(home));
    }

    #[test]
    fn without_a_build_uid_nothing_is_created_or_changed() {
        let dir = tempfile::tempdir().unwrap();
        hand_lockfile_to_build(dir.path(), None).unwrap();
        assert!(!dir.path().join("Cargo.lock").exists());
        chown_for_build(&dir.path().join("missing"), None).unwrap();
    }
}
