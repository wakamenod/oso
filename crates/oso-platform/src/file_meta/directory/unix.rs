//! A directory as a descriptor on Unix: each one below the first opened from its parent by one
//! name with `O_NOFOLLOW`, listed with `fdopendir` on a copy of it, and what it holds looked at
//! with `fstatat`, opened with `openat` and read with `readlinkat`, each relative to it.
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use super::{Entry, EntryKind, MAX_LINK_BYTES, is_one_name, not_one_name};

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "android"
)))]
compile_error!("listing a directory needs `errno` to tell its end from an error on this system");

/// An open directory.
pub(super) type Inner = File;

/// `name` as the system takes it, if it is one entry's name.
fn c_name(name: &OsStr) -> io::Result<CString> {
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
fn stat_at(fd: RawFd, name: &CStr) -> io::Result<libc::stat> {
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
    Ok(unsafe { stat.assume_init() })
}

/// Whether a file's flags say its contents are not on this machine, as a file a sync service
/// keeps in the cloud is: reading it would fetch it.
#[cfg(target_os = "macos")]
fn dataless(stat: &libc::stat) -> bool {
    /// `SF_DATALESS` from `<sys/stat.h>`.
    const SF_DATALESS: u32 = 0x4000_0000;
    stat.st_flags & SF_DATALESS != 0
}

#[cfg(not(target_os = "macos"))]
fn dataless(_: &libc::stat) -> bool {
    false
}

/// Opens `name` in the directory `fd` with `flags`, and never a link at it: `None` for a link,
/// as `O_NOFOLLOW` refuses one, or for `refused`, an error that says it is no such thing as
/// `flags` ask.
fn open_at(
    fd: RawFd,
    name: &CStr,
    flags: libc::c_int,
    refused: &[libc::c_int],
) -> io::Result<Option<File>> {
    let flags = flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
    // SAFETY: `fd` is an open descriptor for the call and `name` is NUL-terminated and outlives
    // it. The descriptor it returns is owned by the `File` made from it below.
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

pub(super) fn open(path: &Path) -> io::Result<Inner> {
    let mut options = OpenOptions::new();
    options.read(true);
    std::os::unix::fs::OpenOptionsExt::custom_flags(
        &mut options,
        libc::O_DIRECTORY | libc::O_NONBLOCK,
    );
    options.open(path)
}

pub(super) fn open_directory(inner: &Inner, name: &OsStr) -> io::Result<Option<Inner>> {
    let name = c_name(name)?;
    // Listing one whose entries a sync service keeps elsewhere would fetch them.
    #[cfg(target_os = "macos")]
    if dataless(&stat_at(inner.as_raw_fd(), &name)?) {
        return Ok(None);
    }
    let flags = libc::O_RDONLY | libc::O_DIRECTORY;
    open_at(inner.as_raw_fd(), &name, flags, &[libc::ENOTDIR])
}

pub(super) fn kind_of(inner: &Inner, name: &OsStr) -> io::Result<EntryKind> {
    let stat = stat_at(inner.as_raw_fd(), &c_name(name)?)?;
    Ok(kind_of_mode(stat.st_mode))
}

pub(super) fn open_file(inner: &Inner, name: &OsStr) -> io::Result<Option<(File, Metadata)>> {
    let fd = inner.as_raw_fd();
    let name = c_name(name)?;
    let stat = stat_at(fd, &name)?;
    if kind_of_mode(stat.st_mode) != EntryKind::File || dataless(&stat) {
        return Ok(None);
    }
    let Some(file) = open_at(fd, &name, libc::O_RDONLY | libc::O_NOCTTY, &[])? else {
        return Ok(None);
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Ok(None);
    }
    super::super::clear_nonblocking(&file)?;
    Ok(Some((file, metadata)))
}

pub(super) fn read_link(inner: &Inner, name: &OsStr) -> io::Result<PathBuf> {
    let fd = inner.as_raw_fd();
    let name = c_name(name)?;
    let mut size = 1024;
    loop {
        let mut bytes = vec![0u8; size];
        // SAFETY: `fd` is an open descriptor for the call, `name` is NUL-terminated and
        // outlives it, and `bytes` has room for the `size` bytes it may write.
        let read = unsafe { libc::readlinkat(fd, name.as_ptr(), bytes.as_mut_ptr().cast(), size) };
        let Ok(read) = usize::try_from(read) else {
            return Err(io::Error::last_os_error());
        };
        if read < size {
            bytes.truncate(read);
            return Ok(PathBuf::from(OsString::from_vec(bytes)));
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

pub(super) fn metadata(inner: &Inner) -> io::Result<Metadata> {
    inner.metadata()
}

/// The entries of the directory `inner`, read from a copy of its descriptor, from its start. A
/// copy shares the original's place, so one directory is listed by one thread at a time.
pub(super) fn entries(inner: &Inner) -> io::Result<Entries> {
    let fd = inner.as_raw_fd();
    // SAFETY: `fd` is an open descriptor for the call; the copy is owned by the stream below,
    // or closed here if that cannot be made.
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if copy < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `copy` is a descriptor this process owns and nothing else uses; the stream takes
    // it over on success.
    let Some(stream) = NonNull::new(unsafe { libc::fdopendir(copy) }) else {
        let error = io::Error::last_os_error();
        // SAFETY: the stream was not made, so `copy` is still ours to close.
        unsafe { libc::close(copy) };
        return Err(error);
    };
    // SAFETY: `stream` is open, and a copy shares the original's place, which may be past the
    // start.
    unsafe { libc::rewinddir(stream.as_ptr()) };
    Ok(Entries {
        stream,
        failed: false,
    })
}

/// The entries of a directory, as [`Directory::entries`](super::Directory::entries) gives them.
pub struct Entries {
    stream: NonNull<libc::DIR>,
    /// Reading failed once, which ends it.
    failed: bool,
}

/// Sets `errno` to 0, so that the end of a stream can be told from an error.
fn clear_errno() {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    // SAFETY: `__error` gives this thread's `errno`, which is always there to write.
    unsafe {
        *libc::__error() = 0;
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: `__errno_location` gives this thread's `errno`, which is always there to write.
    unsafe {
        *libc::__errno_location() = 0;
    }
}

impl Iterator for Entries {
    type Item = io::Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        loop {
            clear_errno();
            // SAFETY: the stream is open while `self` is, and only this thread reads it.
            let entry = unsafe { libc::readdir(self.stream.as_ptr()) };
            let Some(entry) = NonNull::new(entry) else {
                let error = io::Error::last_os_error();
                self.failed = error.raw_os_error().unwrap_or(0) != 0;
                return self.failed.then_some(Err(error));
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
                    stat_at(fd, name).map(|stat| kind_of_mode(stat.st_mode))
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
