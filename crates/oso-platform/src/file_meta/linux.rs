//! File metadata on Linux. A POSIX access list lives in an extended attribute, whose bytes
//! `posix_list` knows, and needs no library. What it grants shows in the mode, so nothing asks of a
//! directory's list, and a new file made with a mode that gives its group nothing grants nothing through
//! one, so sealing has nothing to do. A save still has to carry a list (ADR 0065, ADR 0067).
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::ffi::{CString, c_void};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use super::{AccessEntry, AccessList, Quarantine, posix_list};

/// The name of a file's own list. The name holds no NUL but its last.
const ACCESS: &std::ffi::CStr = c"system.posix_acl_access";

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
pub(super) fn list_at(_path: &Path) -> io::Result<Vec<AccessEntry>> {
    Ok(Vec::new())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
pub(super) fn seal(_file: &File) -> io::Result<()> {
    Ok(())
}

/// Bytes read with `read`, as `read_sized` does. No list, and a file system with no lists, read as
/// none.
fn read_list(read: impl Fn(*mut c_void, usize) -> isize) -> io::Result<Vec<u8>> {
    match super::read_sized(read) {
        Err(e)
            if matches!(
                e.raw_os_error(),
                Some(libc::ENODATA | libc::EOPNOTSUPP | libc::ENOSYS)
            ) =>
        {
            Ok(Vec::new())
        }
        read => read,
    }
}

fn list_of(file: &File) -> io::Result<Vec<u8>> {
    read_list(|buffer, size| {
        // SAFETY: the descriptor is borrowed from `file` for the call, `ACCESS` is NUL-terminated and
        // only read, and `buffer` is null with a size of 0, or a live buffer of that size.
        unsafe { libc::fgetxattr(file.as_raw_fd(), ACCESS.as_ptr(), buffer, size) }
    })
}

fn access_list(bytes: Vec<u8>) -> AccessList {
    AccessList {
        bytes,
        denies_deleting: false,
    }
}

pub(super) fn access_list_at(path: &Path) -> io::Result<AccessList> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::from)?;
    read_list(|buffer, size| {
        // SAFETY: `path` and `ACCESS` are NUL-terminated, outlive the call and are only read, and
        // `buffer` is null with a size of 0, or a live buffer of that size.
        unsafe { libc::lgetxattr(path.as_ptr(), ACCESS.as_ptr(), buffer, size) }
    })
    .map(access_list)
}

pub(super) fn take_access_list(file: &File) -> io::Result<AccessList> {
    let bytes = list_of(file)?;
    if bytes.is_empty() {
        return Ok(access_list(bytes));
    }
    // SAFETY: the descriptor is borrowed from `file` for the call, and `ACCESS` is NUL-terminated and
    // only read.
    if unsafe { libc::fremovexattr(file.as_raw_fd(), ACCESS.as_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if list_of(file)?.is_empty() {
        Ok(access_list(bytes))
    } else {
        Err(io::Error::other(
            "an access list could not be removed from a new file",
        ))
    }
}

pub(super) fn give_access_list(list: &AccessList, file: &File) -> io::Result<()> {
    if list.bytes.is_empty() {
        return Ok(());
    }
    let shut = posix_list::shut(&list.bytes)?;
    // SAFETY: the descriptor is borrowed from `file` for the call, `ACCESS` is NUL-terminated and only
    // read, and `shut` is a live buffer the call reads for the length given.
    let set = unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            ACCESS.as_ptr(),
            shut.as_ptr().cast(),
            shut.len(),
            0,
        )
    };
    if set != 0 {
        return Err(io::Error::last_os_error());
    }
    if list_of(file)? == shut {
        Ok(())
    } else {
        Err(io::Error::other(
            "the file system did not keep an access list",
        ))
    }
}

/// A file made by a save cannot be stopped from going by its list on Linux, as removing it goes by
/// the directory, so this has nothing to do.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
pub(super) fn clear_access_list_at(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub(super) fn create_private(path: &Path) -> io::Result<File> {
    super::create_new_private(path)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
pub(super) fn quarantine_at(_path: &Path) -> io::Result<Option<Quarantine>> {
    Ok(None)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
pub(super) fn give_quarantine(_flag: &Quarantine, _file: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File, OpenOptions, Permissions};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::super::posix_list::bytes;
    use super::{ACCESS, access_list_at, give_access_list, list_of, take_access_list};
    use crate::file_meta::AccessList;

    const NO_ID: u32 = u32::MAX;
    const USER_OBJ: u16 = 0x01;
    const USER: u16 = 0x02;
    const GROUP_OBJ: u16 = 0x04;
    const MASK: u16 = 0x10;
    const OTHER: u16 = 0x20;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("osol-{}-{n}", std::process::id()));
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

    /// `u:12345:rw, g::---`, with the mask open, as `setfacl` would leave it on a `0660` file.
    fn list_for_another_user() -> Vec<u8> {
        bytes(&[
            (USER_OBJ, 6, NO_ID),
            (USER, 6, 12345),
            (GROUP_OBJ, 0, NO_ID),
            (MASK, 6, NO_ID),
            (OTHER, 0, NO_ID),
        ])
    }

    /// Sets the list called `name` on `path`, or says the file system has none. Only outside CI may
    /// that pass as nothing to test.
    fn set(path: &Path, name: &std::ffi::CStr, list: &[u8]) -> bool {
        let file = File::open(path).unwrap();
        // SAFETY: the descriptor is borrowed from `file`, and `name` and `list` outlive the call,
        // which only reads them, `list` for the length given.
        let set = unsafe {
            libc::fsetxattr(
                file.as_raw_fd(),
                name.as_ptr(),
                list.as_ptr().cast(),
                list.len(),
                0,
            )
        };
        if set == 0 {
            return true;
        }
        let error = std::io::Error::last_os_error();
        assert_eq!(error.raw_os_error(), Some(libc::EOPNOTSUPP), "{error}");
        assert!(
            std::env::var_os("CI").is_none(),
            "the file system must have ACLs for this test: {error}"
        );
        false
    }

    fn new_private(path: &Path) -> File {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap()
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().mode() & 0o7777
    }

    #[test]
    fn no_list_is_an_empty_one_that_goes_on_as_nothing() {
        let scratch = Scratch::new();
        let path = scratch.0.join("f");
        let file = new_private(&path);
        let list = access_list_at(&path).unwrap();
        assert_eq!(list.bytes, b"");
        give_access_list(&list, &file).unwrap();
        assert_eq!(take_access_list(&file).unwrap().bytes, b"");
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn a_list_goes_on_shut_and_setting_the_mode_opens_all_of_it_at_once() {
        let scratch = Scratch::new();
        let old = scratch.0.join("old");
        drop(new_private(&old));
        if !set(&old, ACCESS, &list_for_another_user()) {
            return;
        }
        assert_eq!(mode(&old), 0o660, "the mode's group bits are the mask");
        let list = access_list_at(&old).unwrap();
        assert_eq!(list.bytes, list_for_another_user());

        let new = scratch.0.join("new");
        let file = new_private(&new);
        give_access_list(&list, &file).unwrap();
        assert_eq!(mode(&new), 0o600, "nothing is open but to the owner");
        assert_ne!(list_of(&file).unwrap(), list.bytes);

        file.set_permissions(Permissions::from_mode(0o660)).unwrap();
        assert_eq!(mode(&new), 0o660);
        assert_eq!(AccessList::at(&new).unwrap(), list);
    }

    #[test]
    fn what_a_new_file_took_from_its_directory_comes_off_and_can_go_back() {
        let scratch = Scratch::new();
        if !set(
            &scratch.0,
            c"system.posix_acl_default",
            &list_for_another_user(),
        ) {
            return;
        }
        let path = scratch.0.join("f");
        let file = new_private(&path);
        assert_eq!(mode(&path), 0o600, "the mask is shut by the mode asked for");

        let taken = take_access_list(&file).unwrap();
        assert!(!taken.bytes.is_empty(), "it took the directory's");
        assert_eq!(list_of(&file).unwrap(), b"");
        assert_eq!(mode(&path), 0o600);

        give_access_list(&taken, &file).unwrap();
        file.set_permissions(Permissions::from_mode(0o640)).unwrap();
        assert_eq!(mode(&path), 0o640, "the mode opens it, as far as it says");
        assert!(!list_of(&file).unwrap().is_empty());
    }
}
