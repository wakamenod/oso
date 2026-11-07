//! What a file or directory holds beyond its mode that decides who may reach it, and opening a
//! file with what could not be done with `std`. This is the file-system module ADR 0062 expected
//! (ADR 0065, ADR 0067). That means access lists, on macOS and POSIX ones on Linux, and macOS's
//! quarantine flag. Other systems are decided later.
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as backend;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as backend;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod other;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use other as backend;
mod posix_list;

#[cfg(all(test, target_os = "macos"))]
pub(crate) use macos::test_list;

/// One entry of an access list, in what the private directory's rules ask of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AccessEntry {
    /// It grants something. Anything else only takes rights away.
    pub(crate) allows: bool,
    /// What it names lets someone add to or remove from a directory, remove the directory itself,
    /// or change its list or owner.
    pub(crate) lets_change: bool,
    /// What it names is what making, renaming and removing a file in a directory needs.
    pub(crate) names_writing: bool,
    /// What it names is what going through a directory needs.
    pub(crate) names_searching: bool,
    /// What it names is what making a directory in a directory needs.
    pub(crate) names_making_directories: bool,
    /// What it names is reading the list, which a file or directory that takes it could then
    /// not have read.
    pub(crate) names_reading_the_list: bool,
    /// What it names is deleting the file or directory it is on.
    pub(crate) names_deleting: bool,
    /// It counts for the directory itself, and not only for what is made in it.
    pub(crate) applies_here: bool,
    /// A file made in this directory takes the entry.
    pub(crate) reaches_new_files: bool,
    /// A directory made in this one takes the entry.
    pub(crate) reaches_new_directories: bool,
}

/// The access list of the directory at `path`, not following a link at its last name, as a link
/// can hold a list of its own. On macOS, having none and a file system with none are both an
/// empty list. Elsewhere this is always empty, as the rules have nothing to ask. It touches the
/// disk, so it belongs off the UI thread where it can be.
///
/// # Errors
///
/// If the list cannot be read, which says nothing of what it holds.
#[cfg_attr(not(unix), allow(dead_code, reason = "used only by the Unix backends"))]
pub(crate) fn directory_access_list(path: &Path) -> io::Result<Vec<AccessEntry>> {
    backend::list_at(path)
}

/// The access list of a file, as the system holds it. Two can be compared, and one can go on another
/// file. On Windows and other systems there are none yet, so all are empty (ADR 0067).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessList {
    bytes: Vec<u8>,
    denies_deleting: bool,
}

impl AccessList {
    /// The list of the file at `path`, not following a link at its last name. Having none, and a
    /// file system with none, are both an empty list. It touches the disk, so it belongs off the UI
    /// thread.
    ///
    /// # Errors
    ///
    /// If the list cannot be read, which says nothing of what it holds.
    pub fn at(path: &Path) -> io::Result<Self> {
        backend::access_list_at(path)
    }

    /// Takes the list off `file`, which this process has just created and so holds what it took
    /// from its directory, and makes sure none is left. Call this before anything goes in. It touches
    /// the disk, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// If the list cannot be read or removed.
    pub fn take_from(file: &File) -> io::Result<Self> {
        backend::take_access_list(file)
    }

    /// Puts this list on `file`, which holds none, and makes sure it arrived. On Linux what it
    /// grants opens only when the mode is next set, as the mode's group bits are its mask, so set the
    /// mode after this. On macOS it applies at once, so what it denies is in force before the mode opens
    /// anything. It touches the disk, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// If the list cannot be set, or the file system did not keep it. Then it must not be used.
    pub fn give_to(&self, file: &File) -> io::Result<()> {
        backend::give_access_list(self, file)
    }

    /// Whether an entry denies someone deleting the file. Who it names cannot be told, but the
    /// system may then refuse this user replacing or moving it.
    pub fn denies_deleting(&self) -> bool {
        self.denies_deleting
    }
}

/// Takes any access list off the file at `path`, not following a link, so that a file this process
/// made and may not remove can be removed.
///
/// # Errors
///
/// If it cannot be done. A file left behind is all that means.
pub fn clear_access_list_at(path: &Path) -> io::Result<()> {
    backend::clear_access_list_at(path)
}

/// The quarantine flag of a file on macOS, which says it came from elsewhere and that Gatekeeper
/// should check it when it is opened. Other systems have none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quarantine(Vec<u8>);

impl Quarantine {
    /// The flag of the file at `path`, if it has one, not following a link at its last name. It
    /// touches the disk, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// If it cannot be read.
    pub fn at(path: &Path) -> io::Result<Option<Self>> {
        backend::quarantine_at(path)
    }

    /// Puts this flag on `file`, before its access list, which may deny doing so. It touches the
    /// disk, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// If the system refuses.
    pub fn give_to(&self, file: &File) -> io::Result<()> {
        backend::give_quarantine(self, file)
    }
}

/// Opens the file at `path` for writing and writes nothing, to find out if this user may. It does
/// not follow a link at its last name, and does not wait, for a pipe with nobody reading for one, as
/// what is there may have changed since it was looked at. It touches the disk, so it belongs off the UI
/// thread.
///
/// # Errors
///
/// If this user may not, or it is a link, or a pipe with nobody reading.
pub fn open_for_writing(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(
        &mut options,
        libc::O_NOFOLLOW | libc::O_NONBLOCK,
    );
    options.open(path)
}

/// Makes `file`, which this process has just created, grant nothing but what its mode says. Call
/// this before anything goes in. On macOS it removes every access list entry and makes sure none
/// is left. On Linux a new file made with a mode that gives its group nothing already grants
/// nothing through a list, and on other systems nothing is done yet (ADR 0065). It touches the disk,
/// so it belongs off the UI thread.
///
/// # Errors
///
/// If a list cannot be removed, or what the file took from its directory granted anything or could
/// not be read: the directory is then no place for what goes in, whatever it said when it was
/// checked. The list is gone from the file if it could be removed, so the file can be, too.
pub fn seal(file: &File) -> io::Result<()> {
    backend::seal(file)
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::CString;
    use std::fs;
    use std::io::ErrorKind;
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::open_for_writing;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("osof-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn opening_for_writing_writes_nothing_and_needs_the_right() {
        let scratch = Scratch::new();
        let path = scratch.0.join("f");
        fs::write(&path, b"made up").unwrap();
        drop(open_for_writing(&path).unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"made up");
        assert_eq!(
            open_for_writing(&scratch.0.join("missing"))
                .unwrap_err()
                .kind(),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn a_link_is_refused_and_not_followed() {
        let scratch = Scratch::new();
        let path = scratch.0.join("f");
        fs::write(&path, b"made up").unwrap();
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(open_for_writing(&link).is_err());
    }

    #[test]
    fn a_pipe_with_nobody_reading_does_not_wait() {
        let scratch = Scratch::new();
        let pipe = scratch.0.join("pipe");
        let name = CString::new(pipe.as_os_str().as_bytes()).unwrap();
        // SAFETY: `name` is NUL-terminated, outlives the call and is only read.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(open_for_writing(&pipe).is_err());
    }
}
