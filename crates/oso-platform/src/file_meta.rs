//! What a file or directory holds beyond its mode that decides who may reach it, opening a
//! file with what could not be done with `std`, and where an open file is. This is the file-system
//! module ADR 0062 expected (ADR 0044, ADR 0065, ADR 0067). That means access lists, on macOS and
//! POSIX ones on Linux, and macOS's quarantine flag. Other systems are decided later.
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

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
/// file. The default is none at all. On Windows and other systems there are none yet, so all are empty (ADR 0067).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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

/// Gives the file at `path` mode `0600` and then no access list, so that a file whose list denies
/// removing it can be removed. It goes through a descriptor of the file itself, not a link, opened to
/// write and not waiting. If that cannot be opened, nothing is changed, so the file never holds its
/// text with its list gone and a wider mode. On Linux removing a file goes by its directory, so this
/// does nothing there.
///
/// # Errors
///
/// If it cannot be done. The file is then as it was.
pub fn clear_access_list_at(path: &Path) -> io::Result<()> {
    backend::clear_access_list_at(path)
}

/// Creates the file at `path` for writing, which must not exist in any form, with mode `0600` and
/// granting nothing through an access list at any moment. On macOS it takes nothing from its
/// directory's list, as a list on it would apply at once and a descriptor opened then would outlive its
/// removal. On Linux it may take a default list, whose mask the mode shuts. It touches the disk, so it
/// belongs off the UI thread.
///
/// # Errors
///
/// As creating a file does: `AlreadyExists` for a name already there, a link to nothing among them.
pub fn create_private(path: &Path) -> io::Result<File> {
    backend::create_private(path)
}

/// Reads a value whose size is asked for first, through `read`, which takes a buffer and its size, a
/// null one of 0 to ask the size, and returns what `getxattr` does. It asks again if the value grew in
/// between. The error is the system's.
#[cfg(unix)]
fn read_sized(read: impl Fn(*mut std::ffi::c_void, usize) -> isize) -> io::Result<Vec<u8>> {
    loop {
        let size = usize::try_from(read(std::ptr::null_mut(), 0))
            .map_err(|_| io::Error::last_os_error())?;
        let mut bytes = vec![0u8; size];
        match usize::try_from(read(bytes.as_mut_ptr().cast(), bytes.len())) {
            Ok(got) => {
                bytes.truncate(got);
                return Ok(bytes);
            }
            Err(_) if io::Error::last_os_error().raw_os_error() == Some(libc::ERANGE) => {}
            Err(_) => return Err(io::Error::last_os_error()),
        }
    }
}

/// Opens a new file for writing with mode `0600`, as `std` does.
#[cfg(not(target_os = "macos"))]
fn create_new_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
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

/// Opens the file at `path` for reading if it is a regular file, and gives it with its metadata, read
/// from the open file. `None` for anything else: a directory, a device or a pipe. The path is looked at
/// first, so such a thing is not opened at all, and then opened without waiting and without taking a
/// terminal as the controlling one, so that one swapped in after the look cannot hold the thread
/// (ADR 0087). Once the open file is known to be regular, it waits again, as a file usually does. Links
/// are followed. It touches the disk, so it belongs off the UI thread.
///
/// # Errors
///
/// If the path cannot be looked at or opened, with `NotFound` if nothing is there, or if the file's
/// flags cannot be read or set.
pub fn open_for_reading(path: &Path) -> io::Result<Option<(File, std::fs::Metadata)>> {
    open_for_reading_around(path, || {})
}

/// [`open_for_reading`], running `between` after the look and before the open, for a test.
fn open_for_reading_around(
    path: &Path,
    between: impl FnOnce(),
) -> io::Result<Option<(File, std::fs::Metadata)>> {
    if !std::fs::metadata(path)?.is_file() {
        return Ok(None);
    }
    between();
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(
        &mut options,
        libc::O_NONBLOCK | libc::O_NOCTTY,
    );
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Ok(None);
    }
    #[cfg(unix)]
    clear_nonblocking(&file)?;
    Ok(Some((file, metadata)))
}

/// Takes `O_NONBLOCK` off `file`.
#[cfg(unix)]
fn clear_nonblocking(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    // SAFETY: `fd` is open for as long as `file` is borrowed, and `F_GETFL` reads only its flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above; `F_SETFL` changes only the status flags of this open file.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
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

/// Where the open `file` is, as the system names it now: the file itself, whatever a path that led
/// to it leads to since. On macOS that is `F_GETPATH`'s answer, which for a file no longer in any
/// directory may be the path it had, or an error. On Linux it is the link `/proc` keeps, and `None`
/// without `/proc` or for a file no longer in any directory. Elsewhere it is always `None`. It
/// touches the disk, so it belongs off the UI thread.
///
/// # Errors
///
/// If the system could name it but did not.
pub fn path_of_opened(file: &File) -> io::Result<Option<PathBuf>> {
    backend::path_of_opened(file)
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::CString;
    use std::fs;
    use std::io::ErrorKind;
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::{open_for_reading, open_for_reading_around, open_for_writing};

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

    #[test]
    fn opening_for_reading_gives_a_regular_file_that_waits() {
        let scratch = Scratch::new();
        let path = scratch.0.join("f");
        fs::write(&path, b"made up").unwrap();
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        for path in [&path, &link] {
            let (mut file, metadata) = open_for_reading(path).unwrap().unwrap();
            assert_eq!(metadata.len(), 7);
            // SAFETY: the descriptor is open while `file` is, and only its flags are read.
            let flags =
                unsafe { libc::fcntl(std::os::fd::AsRawFd::as_raw_fd(&file), libc::F_GETFL) };
            assert_eq!(flags & libc::O_NONBLOCK, 0, "the flag is cleared");
            let mut read = String::new();
            std::io::Read::read_to_string(&mut file, &mut read).unwrap();
            assert_eq!(read, "made up");
        }
        assert_eq!(
            open_for_reading(&scratch.0.join("missing"))
                .unwrap_err()
                .kind(),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn opening_for_reading_refuses_what_is_no_regular_file_without_waiting() {
        let scratch = Scratch::new();
        let pipe = scratch.0.join("pipe");
        let name = CString::new(pipe.as_os_str().as_bytes()).unwrap();
        // SAFETY: `name` is NUL-terminated, outlives the call and is only read.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(open_for_reading(&pipe).unwrap().is_none());
        assert!(open_for_reading(&scratch.0).unwrap().is_none());
        assert!(
            open_for_reading(std::path::Path::new("/dev/null"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_pipe_swapped_in_after_the_look_is_refused_without_waiting() {
        let scratch = Scratch::new();
        let path = scratch.0.join("f");
        fs::write(&path, b"made up").unwrap();
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        let swap = || {
            fs::remove_file(&path).unwrap();
            // SAFETY: `name` is NUL-terminated, outlives the call and is only read.
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        };
        assert!(open_for_reading_around(&path, swap).unwrap().is_none());
    }
}
