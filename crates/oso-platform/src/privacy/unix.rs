//! The private directory and the core dump limit on Unix.
//!
//! Must not depend on: `control`, `process` or `text`.

use std::ffi::OsString;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use super::{ancestor_is_safe, check_dir};

/// Sets `RLIMIT_CORE` to 0, both soft and hard, so that nothing in this process can
/// raise it again.
pub(super) fn disable_core_dumps() -> io::Result<()> {
    let none = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `none` is an initialized local that outlives the call, and setrlimit only
    // reads it. Lowering a limit is always allowed and touches no memory of ours.
    if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const none) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The effective user ID of this process.
pub(crate) fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions, cannot fail and touches no
    // memory of ours.
    unsafe { libc::geteuid() }
}

/// The per-user directory for state: `~/Library/Application Support` on macOS, otherwise
/// `$XDG_STATE_HOME` or `~/.local/state`. The XDG specification and common sense say a
/// relative path means nothing here.
pub(super) fn state_home_from(var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| {
        var(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    if cfg!(target_os = "macos") {
        return Some(
            absolute("HOME")?
                .join("Library")
                .join("Application Support"),
        );
    }
    match absolute("XDG_STATE_HOME") {
        Some(base) => Some(base),
        None => Some(absolute("HOME")?.join(".local").join("state")),
    }
}

/// Makes sure of the directory at `path`, creating it if missing. What leads to it is made
/// with mode `0700` too, and nothing on the way may let another user replace it.
pub(super) fn open_private(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(parent)
        .map_err(|e| with_path(&e, "cannot create", parent))?;
    for ancestor in parent.ancestors().filter(|a| !a.as_os_str().is_empty()) {
        // A link on the way counts as where it leads, and its own directory comes next.
        let meta = fs::metadata(ancestor).map_err(|e| with_path(&e, "cannot inspect", ancestor))?;
        if !ancestor_is_safe(meta.mode(), meta.uid(), euid()) {
            return Err(refused(ancestor, "can be changed by other users"));
        }
    }
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(with_path(&e, "cannot create", path)),
    }
    let meta = fs::symlink_metadata(path).map_err(|e| with_path(&e, "cannot inspect", path))?;
    check_dir(
        meta.file_type().is_symlink(),
        meta.is_dir(),
        meta.uid(),
        meta.mode(),
        euid(),
    )
    .map_err(|problem| refused(path, problem.message()))
}

fn refused(path: &Path, why: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "refusing to use {} as a private directory: it {why}",
            path.display()
        ),
    )
}

fn with_path(e: &io::Error, what: &str, path: &Path) -> io::Error {
    io::Error::new(e.kind(), format!("{what} {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::fs::{self, Permissions};
    use std::io::ErrorKind;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::{disable_core_dumps, euid};
    use crate::privacy::PrivateDir;

    /// A fresh scratch directory for one test, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("osop-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o7777
    }

    #[test]
    fn creates_it_and_what_leads_to_it_with_mode_0700() {
        let scratch = Scratch::new();
        let path = scratch.join("state/oso/emergency");
        let dir = PrivateDir::open(&path).unwrap();
        assert_eq!(dir.path(), path);
        assert_eq!(mode(&path), 0o700);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(mode(&scratch.join("state")), 0o700);
        PrivateDir::open(&path).expect("opening it again is fine");
    }

    #[test]
    fn refuses_a_looser_mode_and_does_not_repair_it() {
        let scratch = Scratch::new();
        let path = scratch.join("d");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, Permissions::from_mode(0o750)).unwrap();
        let err = PrivateDir::open(&path).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("mode is not 0700"), "{err}");
        assert_eq!(mode(&path), 0o750);
    }

    #[test]
    fn refuses_a_symlink_to_a_private_directory() {
        let scratch = Scratch::new();
        let target = scratch.join("real");
        PrivateDir::open(&target).unwrap();
        let link = scratch.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = PrivateDir::open(&link).unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
    }

    #[test]
    fn refuses_a_file() {
        let scratch = Scratch::new();
        let path = scratch.join("d");
        fs::write(&path, b"").unwrap();
        assert!(PrivateDir::open(&path).is_err());
    }

    #[test]
    fn refuses_another_owner() {
        // Only root can create one, so this runs where `/` is owned by another user,
        // which is everywhere except as root.
        if euid() == 0 {
            return;
        }
        let err = PrivateDir::open("/").unwrap_err();
        assert!(err.to_string().contains("owned by another user"), "{err}");
    }

    #[test]
    fn refuses_a_parent_other_users_can_write_and_creates_nothing_there() {
        let scratch = Scratch::new();
        let parent = scratch.join("shared");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, Permissions::from_mode(0o777)).unwrap();
        let err = PrivateDir::open(parent.join("d")).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(!parent.join("d").exists());
        // A sticky one, like /tmp, is fine.
        fs::set_permissions(&parent, Permissions::from_mode(0o1777)).unwrap();
        PrivateDir::open(parent.join("d")).unwrap();
    }

    #[test]
    fn refuses_anything_on_the_way_that_other_users_can_write() {
        let scratch = Scratch::new();
        let shared = scratch.join("shared");
        fs::create_dir(&shared).unwrap();
        fs::set_permissions(&shared, Permissions::from_mode(0o777)).unwrap();
        let path = shared.join("oso").join("emergency");
        let err = PrivateDir::open(&path).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(!path.exists(), "the directory itself is not made");
    }

    #[test]
    fn core_dumps_stay_off_for_good() {
        disable_core_dumps().unwrap();
        let mut limit = libc::rlimit {
            rlim_cur: 1,
            rlim_max: 1,
        };
        // SAFETY: `limit` is an initialized local that outlives the call, and getrlimit
        // writes only within it.
        let read = unsafe { libc::getrlimit(libc::RLIMIT_CORE, &raw mut limit) };
        assert_eq!(read, 0);
        assert_eq!((limit.rlim_cur, limit.rlim_max), (0, 0));
    }
}
