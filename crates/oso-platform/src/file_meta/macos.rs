//! Access lists on macOS, through the calls in `<sys/acl.h>`. They live in libSystem, which every
//! program here links, and `libc` does not bind them (ADR 0065). The quarantine flag is an extended
//! attribute, which `libc` does (ADR 0067).
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::ffi::{CString, c_char, c_int, c_void};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::ptr;

use super::{AccessEntry, AccessList, Quarantine};

const ACL_TYPE_EXTENDED: c_int = 0x100;
const ACL_FIRST_ENTRY: c_int = 0;
const ACL_NEXT_ENTRY: c_int = -1;
const ACL_EXTENDED_DENY: c_int = 2;
const ACL_ENTRY_FILE_INHERIT: c_int = 1 << 5;
const ACL_ENTRY_DIRECTORY_INHERIT: c_int = 1 << 6;
const ACL_ENTRY_ONLY_INHERIT: c_int = 1 << 8;
/// On a whole list: what it is set on takes nothing from its directory.
const ACL_FLAG_NO_INHERIT: c_int = 1 << 17;
/// `filesec_property_t` values, from `<sys/fcntl.h>`.
const FILESEC_MODE: c_int = 4;
const FILESEC_ACL: c_int = 5;

const ACL_ADD_FILE: u64 = 1 << 2;
const ACL_SEARCH: u64 = 1 << 3;
const ACL_DELETE: u64 = 1 << 4;
const ACL_ADD_SUBDIRECTORY: u64 = 1 << 5;
const ACL_DELETE_CHILD: u64 = 1 << 6;
const ACL_READ_SECURITY: u64 = 1 << 11;
const ACL_WRITE_SECURITY: u64 = 1 << 12;
const ACL_CHANGE_OWNER: u64 = 1 << 13;
/// The name of the quarantine flag.
const QUARANTINE: &std::ffi::CStr = c"com.apple.quarantine";

/// `KAUTH_ACE_GENERIC_ALL`. The system never tests the generic rights, but a list may hold them,
/// so they count for what they say.
const GENERIC_ALL: u64 = 1 << 21;
/// `KAUTH_ACE_GENERIC_WRITE`.
const GENERIC_WRITE: u64 = 1 << 23;

/// The rights that change what a directory holds, or who may change it.
const CHANGING_RIGHTS: u64 = ACL_ADD_FILE
    | ACL_DELETE
    | ACL_ADD_SUBDIRECTORY
    | ACL_DELETE_CHILD
    | ACL_WRITE_SECURITY
    | ACL_CHANGE_OWNER
    | GENERIC_ALL
    | GENERIC_WRITE;

/// What making, renaming and removing a file in a directory needs. Renaming and removing go by
/// `delete_child` on the directory, also for a file whose owner this user is.
const WRITING_RIGHTS: u64 = ACL_ADD_FILE | ACL_DELETE_CHILD | ACL_SEARCH;

unsafe extern "C" {
    fn acl_get_link_np(path: *const c_char, kind: c_int) -> *mut c_void;
    fn acl_get_fd_np(fd: c_int, kind: c_int) -> *mut c_void;
    fn acl_set_fd_np(fd: c_int, list: *mut c_void, kind: c_int) -> c_int;
    fn acl_init(count: c_int) -> *mut c_void;
    fn acl_free(object: *mut c_void) -> c_int;
    fn acl_get_entry(list: *mut c_void, which: c_int, entry: *mut *mut c_void) -> c_int;
    fn acl_get_tag_type(entry: *mut c_void, tag: *mut c_int) -> c_int;
    fn acl_get_permset_mask_np(entry: *mut c_void, mask: *mut u64) -> c_int;
    fn acl_get_flagset_np(object: *mut c_void, flags: *mut *mut c_void) -> c_int;
    fn acl_get_flag_np(flags: *mut c_void, flag: c_int) -> c_int;
    fn acl_size(list: *mut c_void) -> isize;
    fn acl_copy_ext_native(buffer: *mut c_void, list: *mut c_void, size: isize) -> isize;
    fn acl_copy_int_native(buffer: *const c_void) -> *mut c_void;
    fn acl_add_flag_np(flags: *mut c_void, flag: c_int) -> c_int;
    fn acl_set_flagset_np(object: *mut c_void, flags: *mut c_void) -> c_int;
    fn filesec_init() -> *mut c_void;
    fn filesec_free(filesec: *mut c_void);
    fn filesec_set_property(filesec: *mut c_void, property: c_int, value: *const c_void) -> c_int;
    fn openx_np(path: *const c_char, flags: c_int, filesec: *mut c_void) -> c_int;
}

/// A list in this process's memory, freed when this is dropped.
struct List(*mut c_void);

impl Drop for List {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from an `acl_*` call that hands over ownership, and only this
        // `Drop` frees it, once. A failure could only be a bad pointer, so there is none to report.
        unsafe { acl_free(self.0) };
    }
}

impl List {
    /// What a read returned, with the system's error still current if that is null. No list, and a
    /// file system with no lists, read as `None`. `ENOENT` is also what a name that is gone gives, and
    /// the caller has just found it there.
    ///
    /// # Safety
    ///
    /// `list` must be null, or a list an `acl_*` call has just handed over, which this then frees.
    unsafe fn read(list: *mut c_void) -> io::Result<Option<Self>> {
        if !list.is_null() {
            return Ok(Some(Self(list)));
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ENOENT | libc::ENOTSUP | libc::EOPNOTSUPP) => Ok(None),
            _ => Err(error),
        }
    }

    /// The list as bytes this process can keep, and hand back to `acl_copy_int_native`.
    fn to_access_list(&self) -> io::Result<AccessList> {
        let entries = self.entries()?;
        if entries.is_empty() {
            return Ok(no_list());
        }
        // SAFETY: `self.0` is a live list owned by `self`, which the call only reads.
        let size = unsafe { acl_size(self.0) };
        let mut bytes = vec![0u8; usize::try_from(size).map_err(|_| io::Error::last_os_error())?];
        // SAFETY: `bytes` is a live buffer of `size` bytes, the most the call writes, and `self.0` is a
        // live list the call only reads.
        let written = unsafe { acl_copy_ext_native(bytes.as_mut_ptr().cast(), self.0, size) };
        bytes.truncate(usize::try_from(written).map_err(|_| io::Error::last_os_error())?);
        Ok(AccessList {
            bytes,
            denies_deleting: entries
                .iter()
                .any(|entry| !entry.allows && entry.applies_here && entry.names_deleting),
        })
    }

    fn entries(&self) -> io::Result<Vec<AccessEntry>> {
        let mut entries = Vec::new();
        let mut which = ACL_FIRST_ENTRY;
        loop {
            let mut entry = ptr::null_mut();
            // SAFETY: `self.0` is a live list owned by `self`, and `entry` a local that the call
            // writes.
            if unsafe { acl_get_entry(self.0, which, &raw mut entry) } != 0 {
                // `EINVAL` is how the end of a list is told, also of an empty one, and `self.0`
                // cannot be the other thing it means, a pointer that is not a list.
                let error = io::Error::last_os_error();
                return match error.raw_os_error() {
                    Some(libc::EINVAL) => Ok(entries),
                    _ => Err(error),
                };
            }
            // SAFETY: the call just gave `entry` from `self`, which outlives this use of it.
            entries.push(unsafe { read_entry(entry) }?);
            which = ACL_NEXT_ENTRY;
        }
    }
}

/// What one entry says.
///
/// # Safety
///
/// `entry` must point into a list that stays alive for this call.
unsafe fn read_entry(entry: *mut c_void) -> io::Result<AccessEntry> {
    let (mut tag, mut rights, mut flags) = (0, 0, ptr::null_mut());
    // SAFETY: the caller keeps `entry`'s list alive, and each out-pointer is an initialized local
    // the call only writes. `flags` points into the same list, and is not kept past this function.
    let read = unsafe {
        acl_get_tag_type(entry, &raw mut tag) == 0
            && acl_get_permset_mask_np(entry, &raw mut rights) == 0
            && acl_get_flagset_np(entry, &raw mut flags) == 0
    };
    if !read {
        return Err(io::Error::last_os_error());
    }
    let is_set = |flag| {
        // SAFETY: `flags` was just set by a call that succeeded, and its list is still alive.
        match unsafe { acl_get_flag_np(flags, flag) } {
            -1 => Err(io::Error::last_os_error()),
            set => Ok(set == 1),
        }
    };
    Ok(classified(
        tag,
        rights,
        Flags {
            only_inherit: is_set(ACL_ENTRY_ONLY_INHERIT)?,
            to_files: is_set(ACL_ENTRY_FILE_INHERIT)?,
            to_directories: is_set(ACL_ENTRY_DIRECTORY_INHERIT)?,
        },
    ))
}

/// The flags of an entry that the rules ask about.
#[derive(Clone, Copy, Default)]
struct Flags {
    only_inherit: bool,
    to_files: bool,
    to_directories: bool,
}

fn no_list() -> AccessList {
    AccessList {
        bytes: Vec::new(),
        denies_deleting: false,
    }
}

/// What an entry means to the rules. Only a deny takes rights away, so a tag this does not know
/// counts as a grant.
fn classified(tag: c_int, rights: u64, flags: Flags) -> AccessEntry {
    AccessEntry {
        allows: tag != ACL_EXTENDED_DENY,
        lets_change: rights & CHANGING_RIGHTS != 0,
        names_writing: rights & WRITING_RIGHTS != 0,
        names_searching: rights & ACL_SEARCH != 0,
        names_making_directories: rights & (ACL_ADD_SUBDIRECTORY | ACL_SEARCH) != 0,
        names_reading_the_list: rights & ACL_READ_SECURITY != 0,
        names_deleting: rights & ACL_DELETE != 0,
        applies_here: !flags.only_inherit,
        reaches_new_files: flags.to_files,
        reaches_new_directories: flags.to_directories,
    }
}

/// The list of the file at `path`, not following a link.
fn list_at_path(path: &Path) -> io::Result<Option<List>> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::from)?;
    // SAFETY: `path` is NUL-terminated, outlives the call and is only read, and `List::read` takes
    // what the call hands over, with its error still current.
    unsafe { List::read(acl_get_link_np(path.as_ptr(), ACL_TYPE_EXTENDED)) }
}

fn list_of_file(file: &File) -> io::Result<Option<List>> {
    // SAFETY: the descriptor is borrowed from `file` for the call, and `List::read` takes what the
    // call hands over, with its error still current.
    unsafe { List::read(acl_get_fd_np(file.as_raw_fd(), ACL_TYPE_EXTENDED)) }
}

pub(super) fn list_at(path: &Path) -> io::Result<Vec<AccessEntry>> {
    list_at_path(path)?.map_or_else(|| Ok(Vec::new()), |list| list.entries())
}

fn list_of(file: &File) -> io::Result<Vec<AccessEntry>> {
    list_of_file(file)?.map_or_else(|| Ok(Vec::new()), |list| list.entries())
}

pub(super) fn access_list_at(path: &Path) -> io::Result<AccessList> {
    list_at_path(path)?.map_or_else(|| Ok(no_list()), |list| list.to_access_list())
}

fn access_list_of(file: &File) -> io::Result<AccessList> {
    list_of_file(file)?.map_or_else(|| Ok(no_list()), |list| list.to_access_list())
}

pub(super) fn take_access_list(file: &File) -> io::Result<AccessList> {
    let list = access_list_of(file)?;
    if !list.bytes.is_empty() {
        empty_list(file)?;
        if !list_of(file)?.is_empty() {
            return Err(io::Error::other(
                "an access list could not be removed from a new file",
            ));
        }
    }
    Ok(list)
}

pub(super) fn give_access_list(list: &AccessList, file: &File) -> io::Result<()> {
    if list.bytes.is_empty() {
        return Ok(());
    }
    // SAFETY: the bytes came from `acl_copy_ext_native` in this process, so they are a whole list, and
    // the call only reads them. A result that is not null goes straight into `List`.
    let held = unsafe { acl_copy_int_native(list.bytes.as_ptr().cast()) };
    if held.is_null() {
        return Err(io::Error::last_os_error());
    }
    let held = List(held);
    // SAFETY: the descriptor is borrowed from `file` for the call, and `held` is a live list the call
    // only reads.
    if unsafe { acl_set_fd_np(file.as_raw_fd(), held.0, ACL_TYPE_EXTENDED) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if access_list_of(file)?.bytes == list.bytes {
        Ok(())
    } else {
        Err(io::Error::other(
            "the file system did not keep an access list",
        ))
    }
}

/// Through a descriptor of the file itself, opened to write, which does not wait: mode `0600`
/// first, then no list, so that nothing widens. A path alone could not do the first without following
/// a link, so it is not tried. A directory cannot be opened to write, and a link opened this way is the
/// link itself, so nothing else is touched, and nothing needs to be read of the file first, which its
/// list may deny.
pub(super) fn clear_access_list_at(path: &Path) -> io::Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_SYMLINK | libc::O_NONBLOCK)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    empty_list(&file)
}

/// A file system's own record of security for a new file, freed when this is dropped.
struct Filesec(*mut c_void);

impl Drop for Filesec {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from `filesec_init`, and only this `Drop` frees it, once.
        unsafe { filesec_free(self.0) };
    }
}

/// Creates it with a list that takes nothing from the directory and has no entries, which the system
/// stores as no list at all.
pub(super) fn create_private(path: &Path) -> io::Result<File> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::from)?;
    // SAFETY: a count of zero is valid. A result that is not null goes straight into `List`.
    let list = unsafe { acl_init(0) };
    if list.is_null() {
        return Err(io::Error::last_os_error());
    }
    let list = List(list);
    let mut flags = ptr::null_mut();
    // SAFETY: `list.0` is a live list, `flags` a local the first call writes, and the flag set it
    // gives points into that list, which outlives these calls.
    let flagged = unsafe {
        acl_get_flagset_np(list.0, &raw mut flags) == 0
            && acl_add_flag_np(flags, ACL_FLAG_NO_INHERIT) == 0
            && acl_set_flagset_np(list.0, flags) == 0
    };
    if !flagged {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: no arguments. A result that is not null goes straight into `Filesec`.
    let filesec = unsafe { filesec_init() };
    if filesec.is_null() {
        return Err(io::Error::last_os_error());
    }
    let filesec = Filesec(filesec);
    let mode: libc::mode_t = 0o600;
    // SAFETY: `filesec.0` is live, and each value is a pointer to a live local of the type the
    // property takes, `mode_t` and `acl_t`, which the call copies.
    let set = unsafe {
        filesec_set_property(filesec.0, FILESEC_MODE, (&raw const mode).cast()) == 0
            && filesec_set_property(filesec.0, FILESEC_ACL, (&raw const list.0).cast()) == 0
    };
    if !set {
        return Err(io::Error::last_os_error());
    }
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC;
    // SAFETY: `path` is NUL-terminated and only read, and `filesec.0` is live for the call.
    let fd = unsafe { openx_np(path.as_ptr(), flags, filesec.0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just opened here and nothing else owns it.
    Ok(unsafe { <File as std::os::fd::FromRawFd>::from_raw_fd(fd) })
}

pub(super) fn quarantine_at(path: &Path) -> io::Result<Option<Quarantine>> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::from)?;
    let read = super::read_sized(|buffer: *mut c_void, size| {
        // SAFETY: `path` and `QUARANTINE` are NUL-terminated, outlive the call and are only read,
        // `buffer` is null with a size of 0 or a live buffer of that size, and position 0 is what the
        // man page asks of any attribute but a resource fork.
        unsafe {
            libc::getxattr(
                path.as_ptr(),
                QUARANTINE.as_ptr(),
                buffer,
                size,
                0,
                libc::XATTR_NOFOLLOW,
            )
        }
    });
    match read {
        Ok(bytes) => Ok(Some(Quarantine(bytes))),
        Err(e)
            if matches!(
                e.raw_os_error(),
                Some(libc::ENOATTR | libc::ENOTSUP | libc::EOPNOTSUPP)
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

pub(super) fn give_quarantine(flag: &Quarantine, file: &File) -> io::Result<()> {
    // SAFETY: the descriptor is borrowed from `file` for the call, `QUARANTINE` is NUL-terminated and
    // only read, the flag's bytes are read for the length given, and position 0 is what the man page
    // asks of any attribute but a resource fork.
    let set = unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            QUARANTINE.as_ptr(),
            flag.0.as_ptr().cast(),
            flag.0.len(),
            0,
            0,
        )
    };
    if set == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Gives `file` a list with no entries.
fn empty_list(file: &File) -> io::Result<()> {
    // SAFETY: a count of zero is valid. A result that is not null goes straight into `List`.
    let empty = unsafe { acl_init(0) };
    if empty.is_null() {
        return Err(io::Error::last_os_error());
    }
    let empty = List(empty);
    // SAFETY: the descriptor is borrowed from `file` for the call, and `empty` is a live list the
    // call only reads.
    if unsafe { acl_set_fd_np(file.as_raw_fd(), empty.0, ACL_TYPE_EXTENDED) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(super) fn seal(file: &File) -> io::Result<()> {
    let grants = match list_of(file) {
        Ok(found) if found.is_empty() => return Ok(()),
        Ok(found) => found.iter().any(|entry| entry.allows),
        // A list can deny its owner reading it. Nothing can be told of one, so it counts as a grant.
        Err(e) if e.raw_os_error() == Some(libc::EACCES) => true,
        Err(e) => return Err(e),
    };
    empty_list(file)?;
    if !list_of(file)?.is_empty() {
        return Err(io::Error::other(
            "an access list could not be removed from a new file",
        ));
    }
    if grants {
        return Err(io::Error::other(
            "an access list on the directory may give access to what is written there",
        ));
    }
    Ok(())
}

/// Setting lists, for tests, on what they made themselves. `everyone` is named by its fixed
/// identifier, so that no test asks a directory service.
#[cfg(test)]
pub(crate) mod test_list {
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;

    use super::{ACL_TYPE_EXTENDED, List, acl_init, acl_set_fd_np};

    unsafe extern "C" {
        fn acl_from_text(text: *const c_char) -> *mut c_void;
        fn acl_set_file(path: *const c_char, kind: c_int, list: *mut c_void) -> c_int;
    }

    fn set(path: &Path, list: &List) {
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `path` is NUL-terminated and outlives the call, and `list` is a live list the
        // call only reads.
        let set = unsafe { acl_set_file(path.as_ptr(), ACL_TYPE_EXTENDED, list.0) };
        assert_eq!(set, 0, "{}", std::io::Error::last_os_error());
    }

    /// Gives `path` one entry for `everyone`: `how` is `allow` or `deny` with any flags, as in
    /// `allow,directory_inherit`, and `rights` as in `read,execute`. `acl_from_text` does not know
    /// `list`, `search`, `add_file` or `add_subdirectory`, but the names for files are the same bits.
    pub(crate) fn set_for_everyone(path: &Path, how: &str, rights: &str) {
        set(path, &for_everyone(how, rights));
    }

    /// [`set_for_everyone`] on a link, and not on what it leads to, through a descriptor of the
    /// link itself, as `acl_set_link_np` refuses links.
    pub(crate) fn set_on_link_for_everyone(link: &Path, how: &str, rights: &str) {
        let list = for_everyone(how, rights);
        let link = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_SYMLINK)
            .open(link)
            .unwrap();
        // SAFETY: the descriptor is borrowed from `link` for the call, and `list` is a live list the
        // call only reads.
        let set = unsafe { acl_set_fd_np(link.as_raw_fd(), list.0, ACL_TYPE_EXTENDED) };
        assert_eq!(set, 0, "{}", std::io::Error::last_os_error());
    }

    fn for_everyone(how: &str, rights: &str) -> List {
        let text = CString::new(format!(
            "!#acl 1\ngroup:ABCDEFAB-CDEF-ABCD-EFAB-CDEF0000000C:everyone:12:{how}:{rights}\n"
        ))
        .unwrap();
        // SAFETY: `text` is NUL-terminated, outlives the call and is only read.
        let list = unsafe { acl_from_text(text.as_ptr()) };
        assert!(!list.is_null(), "{}", std::io::Error::last_os_error());
        List(list)
    }

    /// Takes every entry off `path`, which also works on a list its owner may not read.
    pub(crate) fn clear(path: &Path) {
        // SAFETY: a count of zero is valid.
        let empty = unsafe { acl_init(0) };
        assert!(!empty.is_null());
        set(path, &List(empty));
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File, OpenOptions};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::test_list::{clear, set_for_everyone, set_on_link_for_everyone};
    use super::{
        ACL_EXTENDED_DENY, CHANGING_RIGHTS, Flags, List, Quarantine, access_list_at, acl_get_entry,
        acl_init, classified, clear_access_list_at, create_private, give_access_list,
        give_quarantine, list_at, list_of, quarantine_at, seal, take_access_list,
    };

    /// The `allow` tag.
    const ALLOW: i32 = 1;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("osom-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
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

    #[test]
    fn a_deny_never_counts_as_a_grant_and_an_unknown_tag_does() {
        assert!(!classified(ACL_EXTENDED_DENY, !0, Flags::default()).allows);
        assert!(classified(ALLOW, 0, Flags::default()).allows);
        assert!(classified(0, 0, Flags::default()).allows);
    }

    #[test]
    fn rights_that_change_a_directory_are_told_from_rights_that_read_it() {
        for write in [
            1 << 2,
            1 << 4,
            1 << 5,
            1 << 6,
            1 << 12,
            1 << 13,
            1 << 21,
            1 << 23,
        ] {
            assert_eq!(CHANGING_RIGHTS & write, write, "{write:#x}");
        }
        // Read or list, execute or search, the attributes and the list itself.
        let reading = (1 << 1) | (1 << 3) | (1 << 7) | (1 << 9) | (1 << 11) | (1 << 20) | (1 << 24);
        assert!(!classified(ALLOW, reading, Flags::default()).lets_change);
    }

    #[test]
    fn rights_a_copy_needs_are_told_from_those_it_does_not() {
        let entry = |rights| classified(ACL_EXTENDED_DENY, rights, Flags::default());
        for writing in [1 << 2, 1 << 3, 1 << 6] {
            assert!(entry(writing).names_writing, "{writing:#x}");
        }
        // Listing, deleting the directory itself, and making a directory in it.
        for other in [1 << 1, 1 << 4, 1 << 5] {
            assert!(!entry(other).names_writing, "{other:#x}");
        }
        assert!(entry(1 << 3).names_searching);
        assert!(!entry(1 << 2).names_searching);
        assert!(entry(1 << 5).names_making_directories);
        assert!(entry(1 << 3).names_making_directories);
        assert!(!entry(1 << 2).names_making_directories);
        assert!(entry(1 << 11).names_reading_the_list);
        assert!(!entry(1 << 12).names_reading_the_list);
    }

    #[test]
    fn the_end_of_a_list_is_einval_and_so_only_that_reads_as_the_end() {
        // SAFETY: a count of zero is valid. The list goes straight into `List`.
        let empty = List(unsafe { acl_init(0) });
        let mut entry = std::ptr::null_mut();
        // SAFETY: `empty.0` is a live list, and `entry` a local the call writes.
        let got = unsafe { acl_get_entry(empty.0, 0, &raw mut entry) };
        assert_eq!(got, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EINVAL)
        );
        assert_eq!(empty.entries().unwrap(), []);

        let scratch = Scratch::new();
        set_for_everyone(&scratch.0, "deny", "delete");
        assert_eq!(list_at(&scratch.0).unwrap().len(), 1, "past the end too");
        clear(&scratch.0);
    }

    #[test]
    fn no_list_reads_as_empty() {
        let scratch = Scratch::new();
        assert_eq!(list_at(&scratch.0).unwrap(), []);
    }

    #[test]
    fn a_real_list_reads_as_what_was_set() {
        let scratch = Scratch::new();

        set_for_everyone(&scratch.0, "deny", "delete");
        let [entry] = list_at(&scratch.0).unwrap()[..] else {
            panic!("one entry");
        };
        assert!(!entry.allows);
        assert!(!entry.reaches_new_directories);

        set_for_everyone(&scratch.0, "allow", "read,execute");
        let [entry] = list_at(&scratch.0).unwrap()[..] else {
            panic!("one entry");
        };
        assert!(entry.allows);
        assert!(!entry.lets_change);

        set_for_everyone(&scratch.0, "allow,directory_inherit", "write");
        let [entry] = list_at(&scratch.0).unwrap()[..] else {
            panic!("one entry");
        };
        assert!(entry.lets_change);
        assert!(entry.reaches_new_directories);

        set_for_everyone(&scratch.0, "allow,file_inherit", "read");
        let [entry] = list_at(&scratch.0).unwrap()[..] else {
            panic!("one entry");
        };
        assert!(!entry.reaches_new_directories, "only to files");

        clear(&scratch.0);
        assert_eq!(list_at(&scratch.0).unwrap(), []);
    }

    #[test]
    fn a_link_is_not_followed_and_holds_a_list_of_its_own() {
        let scratch = Scratch::new();
        let real = scratch.join("real");
        fs::create_dir(&real).unwrap();
        set_for_everyone(&real, "allow", "read");
        let link = scratch.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(list_at(&link).unwrap(), []);

        clear(&real);
        set_on_link_for_everyone(&link, "allow", "delete");
        let [entry] = list_at(&link).unwrap()[..] else {
            panic!("one entry");
        };
        assert!(entry.lets_change);
        assert_eq!(list_at(&real).unwrap(), [], "the list is the link's");
    }

    #[test]
    fn a_list_hidden_from_its_owner_is_an_error_and_not_an_empty_list() {
        let scratch = Scratch::new();
        let hidden = scratch.join("hidden");
        fs::create_dir(&hidden).unwrap();
        set_for_everyone(&hidden, "deny", "readsecurity");
        let read = list_at(&hidden);
        clear(&hidden);
        assert_eq!(read.unwrap_err().raw_os_error(), Some(libc::EACCES));
    }

    #[test]
    fn sealing_a_file_with_no_list_is_nothing_to_do() {
        let scratch = Scratch::new();
        let file = File::create(scratch.join("f")).unwrap();
        seal(&file).unwrap();
        assert_eq!(list_of(&file).unwrap(), []);
    }

    #[test]
    fn a_new_file_takes_its_directorys_list_and_a_grant_refuses_it_with_the_list_gone() {
        let scratch = Scratch::new();
        set_for_everyone(&scratch.0, "allow,file_inherit", "read");
        let file = File::create(scratch.join("f")).unwrap();
        assert_eq!(list_of(&file).unwrap().len(), 1, "it inherited the entry");
        let error = seal(&file).unwrap_err().to_string();
        assert!(error.contains("access list"), "{error}");
        assert_eq!(list_of(&file).unwrap(), []);
        assert_eq!(list_at(&scratch.join("f")).unwrap(), []);
        assert_eq!(
            list_at(&scratch.0).unwrap().len(),
            1,
            "the directory keeps it"
        );
    }

    #[test]
    fn sealing_takes_a_deny_off_too_so_that_the_file_can_be_renamed_and_removed() {
        let scratch = Scratch::new();
        set_for_everyone(&scratch.0, "deny,file_inherit", "delete");
        let stuck = scratch.join("stuck");
        let file = File::create(&stuck).unwrap();
        assert!(fs::rename(&stuck, scratch.join("moved")).is_err());
        assert!(fs::remove_file(&stuck).is_err());

        seal(&file).unwrap();
        fs::rename(&stuck, scratch.join("moved")).unwrap();
        fs::remove_file(scratch.join("moved")).unwrap();
        clear(&scratch.0);
    }

    fn inheriting(scratch: &Scratch, rights: &str) -> File {
        set_for_everyone(&scratch.0, "deny,file_inherit,only_inherit", rights);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(scratch.join("f"))
            .unwrap()
    }

    #[test]
    fn a_deny_the_owner_keeps_clear_of_is_removed_and_the_mode_stays() {
        for rights in ["writesecurity", "delete"] {
            let scratch = Scratch::new();
            let file = inheriting(&scratch, rights);
            seal(&file).unwrap();
            assert_eq!(list_of(&file).unwrap(), [], "{rights}");
            assert_eq!(file.metadata().unwrap().mode() & 0o7777, 0o600, "{rights}");
            clear(&scratch.0);
        }
    }

    #[test]
    fn a_list_that_cannot_be_read_counts_as_a_grant_and_is_gone_all_the_same() {
        let scratch = Scratch::new();
        let file = inheriting(&scratch, "readsecurity");
        let error = seal(&file).unwrap_err().to_string();
        assert!(error.contains("access list"), "{error}");
        assert_eq!(list_of(&file).unwrap(), []);
        clear(&scratch.0);
        fs::remove_file(scratch.join("f")).unwrap();
    }

    #[test]
    fn no_list_comes_off_and_goes_on_as_nothing() {
        let scratch = Scratch::new();
        let file = File::create(scratch.join("f")).unwrap();
        let list = take_access_list(&file).unwrap();
        assert_eq!(list.bytes, b"");
        give_access_list(&list, &file).unwrap();
        assert_eq!(access_list_at(&scratch.join("f")).unwrap(), list);
    }

    #[test]
    fn a_list_goes_from_one_file_to_another_exactly_and_can_be_compared() {
        let scratch = Scratch::new();
        let (old, new) = (scratch.join("old"), scratch.join("new"));
        fs::write(&old, b"made up").unwrap();
        set_for_everyone(&old, "allow", "read");
        let list = access_list_at(&old).unwrap();
        assert!(!list.bytes.is_empty());
        assert!(!list.denies_deleting());

        let file = File::create(&new).unwrap();
        assert_ne!(access_list_at(&new).unwrap(), list);
        give_access_list(&list, &file).unwrap();
        assert_eq!(access_list_at(&new).unwrap(), list);

        set_for_everyone(&old, "deny", "read");
        assert_ne!(access_list_at(&old).unwrap(), list, "a change shows");
    }

    #[test]
    fn what_a_new_file_took_from_its_directory_comes_off_and_can_go_back() {
        let scratch = Scratch::new();
        set_for_everyone(&scratch.0, "allow,file_inherit", "read");
        let file = File::create(scratch.join("f")).unwrap();
        let taken = take_access_list(&file).unwrap();
        assert!(!taken.bytes.is_empty());
        assert_eq!(list_of(&file).unwrap(), [], "nothing is left on it");
        give_access_list(&taken, &file).unwrap();
        assert_eq!(list_of(&file).unwrap().len(), 1);
    }

    #[test]
    fn a_list_that_cannot_be_read_is_an_error_at_a_path_and_on_a_file() {
        let scratch = Scratch::new();
        let path = scratch.join("f");
        fs::write(&path, b"made up").unwrap();
        set_for_everyone(&path, "deny", "readsecurity");
        let at = access_list_at(&path);
        let file = File::open(&path);
        clear(&path);
        assert_eq!(at.unwrap_err().raw_os_error(), Some(libc::EACCES));
        drop(file);

        set_for_everyone(&scratch.0, "deny,file_inherit,only_inherit", "readsecurity");
        let file = File::create(scratch.join("g")).unwrap();
        let taken = take_access_list(&file);
        clear(&scratch.0);
        assert_eq!(taken.unwrap_err().raw_os_error(), Some(libc::EACCES));
    }

    #[test]
    fn denying_deleting_is_told_whoever_it_names() {
        let scratch = Scratch::new();
        let path = scratch.join("f");
        fs::write(&path, b"made up").unwrap();
        set_for_everyone(&path, "deny", "delete");
        assert!(access_list_at(&path).unwrap().denies_deleting());
        set_for_everyone(&path, "allow", "delete");
        assert!(!access_list_at(&path).unwrap().denies_deleting());
    }

    #[test]
    fn a_list_that_stops_a_file_being_removed_is_cleared_after_the_mode_is_shut() {
        for rights in [
            "delete",
            "delete,readattr",
            "delete,readsecurity",
            "delete,read",
        ] {
            let scratch = Scratch::new();
            let path = scratch.join("f");
            fs::write(&path, b"made up").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            set_for_everyone(&path, "deny", rights);
            assert!(fs::remove_file(&path).is_err(), "{rights}");
            clear_access_list_at(&path).unwrap();
            assert_eq!(
                fs::metadata(&path).unwrap().mode() & 0o777,
                0o600,
                "{rights}"
            );
            fs::remove_file(&path).unwrap();
        }
    }

    #[test]
    fn a_file_that_cannot_be_opened_to_write_is_left_as_it_was() {
        let scratch = Scratch::new();
        let path = scratch.join("f");
        fs::write(&path, b"made up").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        set_for_everyone(&path, "deny", "delete,write");
        assert!(clear_access_list_at(&path).is_err());
        assert_eq!(list_at(&path).unwrap().len(), 1, "the list stays");
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o644);
        clear(&path);
    }

    #[test]
    fn a_new_private_file_never_holds_what_its_directory_gives() {
        let scratch = Scratch::new();
        set_for_everyone(&scratch.0, "allow,file_inherit", "read");
        let plain = File::create(scratch.join("plain")).unwrap();
        assert_eq!(
            list_of(&plain).unwrap().len(),
            1,
            "an ordinary file takes it"
        );

        let path = scratch.join("private");
        let private = create_private(&path).unwrap();
        assert_eq!(list_of(&private).unwrap(), [], "from the start");
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o600);
        set_for_everyone(&scratch.0, "deny,file_inherit", "delete");
        let private = create_private(&scratch.join("other")).unwrap();
        assert_eq!(list_of(&private).unwrap(), [], "a deny neither");
        clear(&scratch.0);
    }

    #[test]
    fn a_new_private_file_is_never_a_name_already_there() {
        let scratch = Scratch::new();
        let path = scratch.join("f");
        fs::write(&path, b"made up").unwrap();
        let error = create_private(&path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        std::os::unix::fs::symlink(scratch.join("nothing"), scratch.join("link")).unwrap();
        let error = create_private(&scratch.join("link")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(!scratch.join("nothing").exists());
    }

    #[test]
    fn only_a_denial_of_deleting_the_file_itself_counts() {
        let scratch = Scratch::new();
        set_for_everyone(&scratch.0, "deny,file_inherit,only_inherit", "delete");
        assert!(!access_list_at(&scratch.0).unwrap().denies_deleting());
        clear(&scratch.0);
    }

    #[test]
    fn clearing_does_not_follow_a_link() {
        let scratch = Scratch::new();
        let real = scratch.join("real");
        fs::write(&real, b"made up").unwrap();
        set_for_everyone(&real, "deny", "delete");
        std::os::unix::fs::symlink(&real, scratch.join("link")).unwrap();
        let _ = clear_access_list_at(&scratch.join("link"));
        assert_eq!(list_at(&real).unwrap().len(), 1);
        clear(&real);
    }

    #[test]
    fn a_quarantine_flag_goes_from_one_file_to_another_and_most_files_have_none() {
        let scratch = Scratch::new();
        let (old, new) = (scratch.join("old"), scratch.join("new"));
        let old_file = File::create(&old).unwrap();
        let new_file = File::create(&new).unwrap();
        assert_eq!(quarantine_at(&old).unwrap(), None);

        give_quarantine(&Quarantine(b"0081;00000000;oso;".to_vec()), &old_file).unwrap();
        let flag = quarantine_at(&old).unwrap().unwrap();
        assert_eq!(flag.0, b"0081;00000000;oso;");
        give_quarantine(&flag, &new_file).unwrap();
        assert_eq!(quarantine_at(&new).unwrap(), Some(flag));
    }

    #[test]
    fn a_quarantine_flag_that_cannot_be_read_or_set_is_an_error() {
        let scratch = Scratch::new();
        let path = scratch.join("f");
        let file = File::create(&path).unwrap();
        set_for_everyone(&path, "deny", "readextattr");
        assert!(quarantine_at(&path).is_err());
        set_for_everyone(&path, "deny", "writeextattr");
        assert!(give_quarantine(&Quarantine(b"0081;00000000;oso;".to_vec()), &file).is_err());
    }
}
