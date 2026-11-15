//! What the directory editor does to files on Unix (ADR 0134, step 1b): renaming, removing and
//! copying an entry, each reached from an open directory by one name and never through a link,
//! and making a directory.
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.
//!
//! A copy that replaces what is there is written beside it under a name of its own, then renamed
//! over it, so the old file stays whole until the new one is. A copy keeps the source's mode,
//! times and, on macOS, its extended attributes, the quarantine flag among them: by a clone
//! where the file system makes one, else by `fcopyfile`.

use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;

use super::{Inner, c_name, kind_of_mode, stat_at};
use crate::file_meta::directory::EntryKind;

/// A name for a copy written beside the file it replaces, which no listing shows for long: the
/// process and the time make it one no other copy takes, and creating it refuses one there.
fn temporary_name() -> CString {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let name = format!(".oso-copy-{}-{nanos}.tmp", std::process::id());
    CString::new(name).unwrap_or_default()
}

/// The error of a call that returned `done`, or nothing.
fn checked(done: libc::c_int) -> io::Result<()> {
    if done == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Removes `name` in `fd`, a file or a link; what it is was found already.
fn unlink_at(fd: RawFd, name: &CStr, directory: bool) -> io::Result<()> {
    let flags = if directory { libc::AT_REMOVEDIR } else { 0 };
    // SAFETY: `fd` is an open descriptor for the call and `name` is NUL-terminated and outlives
    // it; nothing is written through either.
    checked(unsafe { libc::unlinkat(fd, name.as_ptr(), flags) })
}

pub(in crate::file_meta::directory) fn identity(inner: &Inner) -> io::Result<Option<(u64, u64)>> {
    use std::os::unix::fs::MetadataExt;
    let metadata = inner.metadata()?;
    Ok(Some((metadata.dev(), metadata.ino())))
}

/// Renames `from` in `from_fd` to `to` in `to_fd`, over what is there only if `replace`.
fn rename_at(
    (from_fd, from): (RawFd, &CStr),
    (to_fd, to): (RawFd, &CStr),
    replace: bool,
) -> io::Result<()> {
    if replace {
        // SAFETY: both descriptors are open for the call, and both names are NUL-terminated
        // and outlive it.
        return checked(unsafe { libc::renameat(from_fd, from.as_ptr(), to_fd, to.as_ptr()) });
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: as above; `RENAME_EXCL` refuses a name that is there.
        checked(unsafe {
            libc::renameatx_np(
                from_fd,
                from.as_ptr(),
                to_fd,
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        })
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: as above; `RENAME_NOREPLACE` refuses a name that is there.
        checked(unsafe {
            libc::renameat2(
                from_fd,
                from.as_ptr(),
                to_fd,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        })
    }
    #[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
    {
        // No call refuses a name that is there: it is looked for first.
        match stat_at(to_fd, to) {
            Ok(_) => return Err(io::ErrorKind::AlreadyExists.into()),
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            Err(_) => {}
        }
        // SAFETY: as above.
        checked(unsafe { libc::renameat(from_fd, from.as_ptr(), to_fd, to.as_ptr()) })
    }
}

pub(in crate::file_meta::directory) fn rename_to(
    inner: &Inner,
    name: &OsStr,
    to: &Inner,
    to_name: &OsStr,
    replace: bool,
) -> io::Result<()> {
    let (name, to_name) = (c_name(name)?, c_name(to_name)?);
    rename_at(
        (inner.as_raw_fd(), &name),
        (to.as_raw_fd(), &to_name),
        replace,
    )
}

pub(in crate::file_meta::directory) fn remove(inner: &Inner, name: &OsStr) -> io::Result<()> {
    let fd = inner.as_raw_fd();
    let name = c_name(name)?;
    let kind = kind_of_mode(stat_at(fd, &name)?.st_mode);
    unlink_at(fd, &name, kind == EntryKind::Directory)
}

pub(in crate::file_meta::directory) fn make_directory(
    inner: &Inner,
    name: &OsStr,
    mode: u32,
) -> io::Result<Option<Inner>> {
    let c_name = c_name(name)?;
    let mode = libc::mode_t::try_from(mode & 0o7777).unwrap_or(0o700);
    // SAFETY: `inner`'s descriptor is open for the call, and `c_name` is NUL-terminated and
    // outlives it; nothing is written through either.
    let made = unsafe { libc::mkdirat(inner.as_raw_fd(), c_name.as_ptr(), mode) };
    if made != 0 {
        let error = io::Error::last_os_error();
        // One there already is gone into, if it is a directory and no link.
        if error.raw_os_error() != Some(libc::EEXIST) {
            return Err(error);
        }
    }
    super::open_directory(inner, name)
}

pub(in crate::file_meta::directory) fn copy_file_to(
    inner: &Inner,
    name: &OsStr,
    to: &Inner,
    to_name: &OsStr,
    replace: bool,
) -> io::Result<()> {
    let Some((source, _)) = super::open_file(inner, name)? else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file here, or one kept elsewhere",
        ));
    };
    let to_fd = to.as_raw_fd();
    let to_name = c_name(to_name)?;
    let written = if replace {
        temporary_name()
    } else {
        to_name.clone()
    };
    copy_into(&source, to_fd, &written)?;
    if replace && let Err(error) = rename_at((to_fd, &written), (to_fd, &to_name), true) {
        let _ = unlink_at(to_fd, &written, false);
        return Err(error);
    }
    Ok(())
}

pub(in crate::file_meta::directory) fn copy_link_to(
    inner: &Inner,
    name: &OsStr,
    to: &Inner,
    to_name: &OsStr,
    replace: bool,
) -> io::Result<()> {
    let target = super::read_link(inner, name)?;
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    let to_fd = to.as_raw_fd();
    let to_name = c_name(to_name)?;
    let written = if replace {
        temporary_name()
    } else {
        to_name.clone()
    };
    // SAFETY: `to_fd` is open for the call, and both strings are NUL-terminated and outlive it.
    checked(unsafe { libc::symlinkat(target.as_ptr(), to_fd, written.as_ptr()) })?;
    if replace && let Err(error) = rename_at((to_fd, &written), (to_fd, &to_name), true) {
        let _ = unlink_at(to_fd, &written, false);
        return Err(error);
    }
    Ok(())
}

/// Creates `name` in `to_fd`, refusing anything there and never through a link, open for
/// writing, readable by its owner only until the copy sets the source's mode.
fn create_at(to_fd: RawFd, name: &CStr) -> io::Result<File> {
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let mode: libc::c_uint = 0o600;
    // SAFETY: `to_fd` is open for the call, `name` is NUL-terminated and outlives it, and `mode`
    // is passed as the `unsigned int` the variadic argument is read as.
    let opened = unsafe { libc::openat(to_fd, name.as_ptr(), flags, mode) };
    if opened < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `opened` is a descriptor just opened, which nothing else owns.
    Ok(unsafe { File::from_raw_fd(opened) })
}

/// Copies the open file `source` to a new `name` in `to_fd`, with its mode, times and extended
/// attributes: by a clone where the file system makes one, else by `fcopyfile`.
#[cfg(target_os = "macos")]
fn copy_into(source: &File, to_fd: RawFd, name: &CStr) -> io::Result<()> {
    // SAFETY: both descriptors are open for the call and `name` is NUL-terminated and outlives
    // it. A clone refuses a name that is there.
    let cloned = unsafe { libc::fclonefileat(source.as_raw_fd(), to_fd, name.as_ptr(), 0) };
    if cloned == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::AlreadyExists {
        return Err(error);
    }
    let written = create_at(to_fd, name)?;
    let flags =
        libc::COPYFILE_DATA | libc::COPYFILE_STAT | libc::COPYFILE_XATTR | libc::COPYFILE_ACL;
    // SAFETY: both descriptors are open for the call; a null state asks for none.
    let copied = unsafe {
        libc::fcopyfile(
            source.as_raw_fd(),
            written.as_raw_fd(),
            std::ptr::null_mut(),
            flags,
        )
    };
    if copied != 0 {
        let error = io::Error::last_os_error();
        let _ = unlink_at(to_fd, name, false);
        return Err(error);
    }
    // On the disk before a move removes its source.
    if let Err(error) = written.sync_all() {
        let _ = unlink_at(to_fd, name, false);
        return Err(error);
    }
    Ok(())
}

/// Copies the open file `source` to a new `name` in `to_fd`, with its mode and times, the
/// bytes by `copy_file_range` where `std` can.
#[cfg(not(target_os = "macos"))]
fn copy_into(source: &File, to_fd: RawFd, name: &CStr) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut written = create_at(to_fd, name)?;
    let copied = (|| {
        let metadata = source.metadata()?;
        io::copy(&mut &*source, &mut written)?;
        let mode = metadata.permissions().mode() & 0o7777;
        written.set_permissions(std::fs::Permissions::from_mode(mode))?;
        let times = std::fs::FileTimes::new()
            .set_accessed(metadata.accessed()?)
            .set_modified(metadata.modified()?);
        written.set_times(times)?;
        // On the disk before a move removes its source.
        written.sync_all()
    })();
    if copied.is_err() {
        let _ = unlink_at(to_fd, name, false);
    }
    copied
}
