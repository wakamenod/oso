//! The binary `oso-restart` starts (ADR 0133, Q3): the file this instance came from, as its
//! path read when it started, once that file and every directory above it can be changed by no
//! one but the person and root, as the configuration is checked (ADR 0108), that is an
//! executable file and runs as the person, not as its owner or group. On macOS, the
//! signature's team identity is compared in a later step; until then, the path and these
//! checks are all.
//!
//! Must not depend on: `app` or threads; [`check`] looks at the file system, so the caller runs
//! it off the UI thread.
//!
//! The path is read at start, as Linux names a replaced binary `… (deleted)` from then on, and
//! an install replaces it. The check and the start both go by the path: a file put there in
//! between needs the person's or root's rights, which could as well wait for the next start.

// The restart needs the pair and terminals, which only Unix has for now; its parts are
// tested everywhere.
#![cfg_attr(not(unix), allow(dead_code, reason = "oso-restart runs on Unix only"))]

use std::path::{Path, PathBuf};

use oso_platform::privacy::{Writers, who_can_change};
use oso_platform::process::Executable;

/// This process's own binary, resolved, as it is when this runs: at start.
pub fn own_path() -> Option<PathBuf> {
    std::env::current_exe().and_then(std::fs::canonicalize).ok()
}

/// The binary at `path` if it may be started for a restart, or why not, naming only the path.
pub fn check(path: Option<&Path>) -> Result<Executable, String> {
    let path = path.ok_or("this instance does not know the binary it came from")?;
    let shown = path.display();
    let who = who_can_change(path).map_err(|e| format!("cannot look at {shown}: {e}"))?;
    if who.file != Writers::Nobody {
        return Err(format!("someone else can change {shown}"));
    }
    if who.way != Writers::Nobody {
        return Err(format!("someone else can change a directory above {shown}"));
    }
    if sets_its_user(&who.metadata) {
        return Err(format!("{shown} runs as its owner or group, not as you"));
    }
    Executable::absolute(path).map_err(|e| format!("cannot run {shown}: {e}"))
}

/// Whether the file `metadata` describes runs as its owner or its group (setuid, setgid).
#[cfg(unix)]
fn sets_its_user(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o6000 != 0
}

/// Windows has no such bits.
#[cfg(not(unix))]
fn sets_its_user(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::{check, own_path};

    /// A directory of the test's own, under the system's temporary one.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oso-binary-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn the_test_binary_is_its_own_path() {
        let path = own_path().unwrap();
        assert!(path.is_absolute());
        assert_eq!(
            path,
            std::env::current_exe().unwrap().canonicalize().unwrap()
        );
    }

    #[test]
    fn nothing_known_is_refused() {
        assert!(check(None).unwrap_err().contains("does not know"));
    }

    #[test]
    fn a_file_others_can_write_is_refused() {
        let dir = scratch("others");
        let file = dir.join("oso");
        fs::write(&file, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o757)).unwrap();
        let why = check(Some(&file)).unwrap_err();
        assert!(why.contains("someone else can change"), "{why}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_that_cannot_run_is_missing_is_a_directory_or_is_setuid_is_refused() {
        let dir = scratch("plain");
        let file = dir.join("oso");
        fs::write(&file, b"text").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            check(Some(&file))
                .unwrap_err()
                .contains("not an executable file")
        );
        assert!(check(Some(&dir.join("missing"))).is_err());
        assert!(
            check(Some(&dir))
                .unwrap_err()
                .contains("not an executable file")
        );
        fs::set_permissions(&file, fs::Permissions::from_mode(0o4755)).unwrap();
        assert!(
            check(Some(&file))
                .unwrap_err()
                .contains("runs as its owner")
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
