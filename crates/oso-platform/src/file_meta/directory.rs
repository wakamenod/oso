//! A directory opened to list it and to open what it holds relative to itself, following no
//! link on the way, as a project's walk needs (ADR 0114).
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.
//!
//! On Unix a [`Directory`] is a descriptor: each one below the first is opened from its parent
//! by one name, refusing a link, and what it holds is listed and opened from that descriptor,
//! so no path is looked up again. On Windows it is a path, looked at without following a link
//! at its last name; what is reached by it is not held, so a directory replaced between two
//! looks is not noticed there.

use std::ffi::{OsStr, OsString};
use std::fs::{File, Metadata};
use std::io;
use std::path::{Path, PathBuf};

/// What an entry of a directory is, without following it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Link,
    /// A device, a pipe, a socket or anything else.
    Other,
}

/// One entry of a directory: its name and what it is.
#[derive(Debug)]
pub struct Entry {
    pub name: OsString,
    pub kind: EntryKind,
}

/// An open directory. See the module.
#[derive(Debug)]
pub struct Directory {
    #[cfg(unix)]
    file: File,
    #[cfg(not(unix))]
    path: PathBuf,
}

/// The error for a name that is not one entry of a directory: empty, `.`, `..`, or holding a
/// separator or a NUL.
fn not_one_name() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "not one name in a directory")
}

/// Whether `name` names one entry of a directory.
fn is_one_name(name: &OsStr) -> bool {
    let bytes = name.as_encoded_bytes();
    !bytes.is_empty()
        && name != "."
        && name != ".."
        && !bytes.contains(&b'/')
        && !bytes.contains(&0)
        && (cfg!(unix) || !bytes.contains(&b'\\'))
}

/// The most bytes of a link's target read.
const MAX_LINK_BYTES: usize = 64 * 1024;

#[cfg(unix)]
mod unix {
    use std::ffi::{CStr, CString, OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::ptr::NonNull;

    use super::{Entry, EntryKind, MAX_LINK_BYTES, is_one_name, not_one_name};

    /// `name` as the system takes it, if it is one entry's name.
    pub(super) fn c_name(name: &OsStr) -> io::Result<CString> {
        if !is_one_name(name) {
            return Err(not_one_name());
        }
        CString::new(name.as_bytes()).map_err(|_| not_one_name())
    }

    /// The kind a mode says.
    fn kind_of_mode(mode: libc::mode_t) -> EntryKind {
        match mode & libc::S_IFMT {
            libc::S_IFREG => EntryKind::File,
            libc::S_IFDIR => EntryKind::Directory,
            libc::S_IFLNK => EntryKind::Link,
            _ => EntryKind::Other,
        }
    }

    /// What `name` in the directory `fd` is, not following it.
    pub(super) fn kind_at(fd: RawFd, name: &CStr) -> io::Result<EntryKind> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `fd` is an open descriptor for the call, `name` is NUL-terminated and outlives
        // it, and `stat` is written whole on success, which is the only case it is read.
        let done = unsafe {
            libc::fstatat(
                fd,
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if done != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fstatat` succeeded, so it filled `stat`.
        let stat = unsafe { stat.assume_init() };
        Ok(kind_of_mode(stat.st_mode))
    }

    /// Opens `name` in the directory `fd` with `flags`, and never a link at it: `None` for a
    /// link, as `O_NOFOLLOW` refuses one, or for `refused`, an error that says it is no such
    /// thing as `flags` ask.
    pub(super) fn open_at(
        fd: RawFd,
        name: &CStr,
        flags: libc::c_int,
        refused: &[libc::c_int],
    ) -> io::Result<Option<File>> {
        let flags = flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
        // SAFETY: `fd` is an open descriptor for the call and `name` is NUL-terminated and
        // outlives it. The descriptor it returns is owned by the `File` made from it below.
        let opened = unsafe { libc::openat(fd, name.as_ptr(), flags) };
        if opened < 0 {
            let error = io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(code) if code == libc::ELOOP || refused.contains(&code) => Ok(None),
                _ => Err(error),
            };
        }
        // SAFETY: `opened` is a descriptor just opened, which nothing else owns.
        Ok(Some(unsafe { File::from_raw_fd(opened) }))
    }

    /// The target of the link `name` in the directory `fd`, as its bytes say.
    pub(super) fn read_link_at(fd: RawFd, name: &CStr) -> io::Result<OsString> {
        let mut size = 1024;
        loop {
            let mut bytes = vec![0u8; size];
            // SAFETY: `fd` is an open descriptor for the call, `name` is NUL-terminated and
            // outlives it, and `bytes` has room for the `size` bytes it may write.
            let read =
                unsafe { libc::readlinkat(fd, name.as_ptr(), bytes.as_mut_ptr().cast(), size) };
            let Ok(read) = usize::try_from(read) else {
                return Err(io::Error::last_os_error());
            };
            if read < size {
                bytes.truncate(read);
                return Ok(OsString::from_vec(bytes));
            }
            if size >= MAX_LINK_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "a link's target is too long",
                ));
            }
            size *= 4;
        }
    }

    /// The entries of the directory `fd`, read from a copy of the descriptor, from its start.
    pub(super) fn entries(fd: RawFd) -> io::Result<Entries> {
        // SAFETY: `fd` is an open descriptor for the call; the copy is owned by the stream
        // below, or closed here if that cannot be made.
        let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if copy < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `copy` is a descriptor this process owns and nothing else uses; the stream
        // takes it over on success.
        let Some(stream) = NonNull::new(unsafe { libc::fdopendir(copy) }) else {
            let error = io::Error::last_os_error();
            // SAFETY: the stream was not made, so `copy` is still ours to close.
            unsafe { libc::close(copy) };
            return Err(error);
        };
        // SAFETY: `stream` is open, and a copy shares the original's place, which may be past
        // the start.
        unsafe { libc::rewinddir(stream.as_ptr()) };
        Ok(Entries { stream })
    }

    /// The entries of a directory, as [`Directory::entries`](super::Directory::entries) gives
    /// them.
    pub struct Entries {
        stream: NonNull<libc::DIR>,
    }

    /// Sets `errno` to 0, so that the end of a stream can be told from an error.
    fn clear_errno() {
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
        // SAFETY: `__error` gives this thread's `errno`, which is always there to write.
        unsafe {
            *libc::__error() = 0;
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        // SAFETY: `__errno_location` gives this thread's `errno`, which is always there to
        // write.
        unsafe {
            *libc::__errno_location() = 0;
        }
    }

    impl Iterator for Entries {
        type Item = io::Result<Entry>;

        fn next(&mut self) -> Option<Self::Item> {
            loop {
                clear_errno();
                // SAFETY: the stream is open while `self` is, and only this thread reads it.
                let entry = unsafe { libc::readdir(self.stream.as_ptr()) };
                let Some(entry) = NonNull::new(entry) else {
                    let error = io::Error::last_os_error();
                    return (error.raw_os_error().unwrap_or(0) != 0).then_some(Err(error));
                };
                // SAFETY: `readdir` gave an entry that stays valid until the next call on this
                // stream, and its name is NUL-terminated within it.
                let (name, kind) = unsafe {
                    let entry = entry.as_ref();
                    (CStr::from_ptr(entry.d_name.as_ptr()), entry.d_type)
                };
                if name.to_bytes() == b"." || name.to_bytes() == b".." {
                    continue;
                }
                let kind = match kind {
                    libc::DT_REG => Ok(EntryKind::File),
                    libc::DT_DIR => Ok(EntryKind::Directory),
                    libc::DT_LNK => Ok(EntryKind::Link),
                    // The file system did not say: it is looked at, not followed.
                    libc::DT_UNKNOWN => {
                        // SAFETY: the stream is open, so its descriptor is.
                        let fd = unsafe { libc::dirfd(self.stream.as_ptr()) };
                        kind_at(fd, name)
                    }
                    _ => Ok(EntryKind::Other),
                };
                let name = OsStr::from_bytes(name.to_bytes()).to_os_string();
                return Some(kind.map(|kind| Entry { name, kind }));
            }
        }
    }

    impl Drop for Entries {
        fn drop(&mut self) {
            // SAFETY: the stream is open and closed only here, once.
            unsafe { libc::closedir(self.stream.as_ptr()) };
        }
    }

    impl std::fmt::Debug for Entries {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Entries").finish_non_exhaustive()
        }
    }

    pub(super) fn raw(file: &File) -> RawFd {
        file.as_raw_fd()
    }
}

#[cfg(unix)]
pub use unix::Entries;

/// The entries of a directory, as [`Directory::entries`] gives them.
#[cfg(not(unix))]
#[derive(Debug)]
pub struct Entries(std::fs::ReadDir);

#[cfg(not(unix))]
impl Iterator for Entries {
    type Item = io::Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.0.next()?;
        Some(entry.and_then(|entry| {
            let kind = kind_of_type(entry.file_type()?);
            Ok(Entry {
                name: entry.file_name(),
                kind,
            })
        }))
    }
}

/// The kind a file type says.
#[cfg(not(unix))]
fn kind_of_type(kind: std::fs::FileType) -> EntryKind {
    if kind.is_symlink() {
        EntryKind::Link
    } else if kind.is_dir() {
        EntryKind::Directory
    } else if kind.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

impl Directory {
    /// The directory at `path`, which is followed if it is a link, as the place a walk starts
    /// from is the one the person or the editor named. It touches the disk, so it belongs off
    /// the UI thread.
    ///
    /// # Errors
    ///
    /// If it cannot be opened, or is no directory.
    pub fn open(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            std::os::unix::fs::OpenOptionsExt::custom_flags(
                &mut options,
                libc::O_DIRECTORY | libc::O_NONBLOCK,
            );
            Ok(Self {
                file: options.open(path)?,
            })
        }
        #[cfg(not(unix))]
        {
            if !std::fs::metadata(path)?.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "not a directory",
                ));
            }
            Ok(Self {
                path: path.to_path_buf(),
            })
        }
    }

    /// The directory `name` in this one, opened from it, if it is itself a directory: `None`
    /// for a link, as for a file or anything else. It touches the disk, so it belongs off the
    /// UI thread.
    ///
    /// # Errors
    ///
    /// If `name` is not one entry's name, or it cannot be opened, with `NotFound` if nothing is
    /// there.
    pub fn open_directory(&self, name: &OsStr) -> io::Result<Option<Self>> {
        #[cfg(unix)]
        {
            let name = unix::c_name(name)?;
            let opened = unix::open_at(
                unix::raw(&self.file),
                &name,
                libc::O_RDONLY | libc::O_DIRECTORY,
                &[libc::ENOTDIR],
            )?;
            Ok(opened.map(|file| Self { file }))
        }
        #[cfg(not(unix))]
        {
            if !is_one_name(name) {
                return Err(not_one_name());
            }
            let path = self.path.join(name);
            let kind = std::fs::symlink_metadata(&path)?.file_type();
            Ok((kind.is_dir() && !kind.is_symlink()).then_some(Self { path }))
        }
    }

    /// Its entries, `.` and `..` left out, each with what it is, not following it. It touches
    /// the disk, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// If it cannot be read. An entry that cannot be looked at is an error of its own.
    pub fn entries(&self) -> io::Result<Entries> {
        #[cfg(unix)]
        {
            unix::entries(unix::raw(&self.file))
        }
        #[cfg(not(unix))]
        {
            std::fs::read_dir(&self.path).map(Entries)
        }
    }

    /// What the entry `name` is, not following it. It touches the disk, so it belongs off the
    /// UI thread.
    ///
    /// # Errors
    ///
    /// If `name` is not one entry's name, or it cannot be looked at, with `NotFound` if nothing
    /// is there.
    pub fn kind_of(&self, name: &OsStr) -> io::Result<EntryKind> {
        #[cfg(unix)]
        {
            unix::kind_at(unix::raw(&self.file), &unix::c_name(name)?)
        }
        #[cfg(not(unix))]
        {
            if !is_one_name(name) {
                return Err(not_one_name());
            }
            Ok(kind_of_type(
                std::fs::symlink_metadata(self.path.join(name))?.file_type(),
            ))
        }
    }

    /// Opens the entry `name` for reading if it is itself a regular file, and gives it with its
    /// metadata, read from the open file, as
    /// [`open_regular_here`](super::open_regular_here) does: `None` for a link, a directory, a
    /// device or a pipe. It is looked at first, so such a thing is not opened at all, then
    /// opened without waiting and without taking a terminal as the controlling one, and waits
    /// again once it is known to be regular. It touches the disk, so it belongs off the UI
    /// thread.
    ///
    /// # Errors
    ///
    /// If `name` is not one entry's name, or it cannot be looked at or opened, with `NotFound`
    /// if nothing is there, or if the file's flags cannot be read or set.
    pub fn open_file(&self, name: &OsStr) -> io::Result<Option<(File, Metadata)>> {
        #[cfg(unix)]
        {
            let fd = unix::raw(&self.file);
            let name = unix::c_name(name)?;
            if unix::kind_at(fd, &name)? != EntryKind::File {
                return Ok(None);
            }
            let Some(file) = unix::open_at(fd, &name, libc::O_RDONLY | libc::O_NOCTTY, &[])? else {
                return Ok(None);
            };
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                return Ok(None);
            }
            super::clear_nonblocking(&file)?;
            Ok(Some((file, metadata)))
        }
        #[cfg(not(unix))]
        {
            if !is_one_name(name) {
                return Err(not_one_name());
            }
            super::open_regular_here(&self.path.join(name))
        }
    }

    /// The target of the link `name`, as its words say, not looked up. It touches the disk, so
    /// it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// If `name` is not one entry's name, is no link, or cannot be read, or its target is
    /// longer than 64 KiB.
    pub fn read_link(&self, name: &OsStr) -> io::Result<PathBuf> {
        #[cfg(unix)]
        {
            unix::read_link_at(unix::raw(&self.file), &unix::c_name(name)?).map(PathBuf::from)
        }
        #[cfg(not(unix))]
        {
            if !is_one_name(name) {
                return Err(not_one_name());
            }
            let target = std::fs::read_link(self.path.join(name))?;
            if target.as_os_str().len() > MAX_LINK_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "a link's target is too long",
                ));
            }
            Ok(target)
        }
    }

    /// Its own metadata: its owner, its device and inode. It touches the disk, so it belongs
    /// off the UI thread.
    ///
    /// # Errors
    ///
    /// If it cannot be read.
    pub fn metadata(&self) -> io::Result<Metadata> {
        #[cfg(unix)]
        {
            self.file.metadata()
        }
        #[cfg(not(unix))]
        {
            std::fs::symlink_metadata(&self.path)
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
