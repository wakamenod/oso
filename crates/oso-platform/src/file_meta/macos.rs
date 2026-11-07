//! Access lists on macOS, through the calls in `<sys/acl.h>`. They live in libSystem, which every
//! program here links, and `libc` does not bind them (ADR 0065).
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::ffi::{CString, c_char, c_int, c_void};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use super::AccessEntry;

const ACL_TYPE_EXTENDED: c_int = 0x100;
const ACL_FIRST_ENTRY: c_int = 0;
const ACL_NEXT_ENTRY: c_int = -1;
const ACL_EXTENDED_DENY: c_int = 2;
const ACL_ENTRY_FILE_INHERIT: c_int = 1 << 5;
const ACL_ENTRY_DIRECTORY_INHERIT: c_int = 1 << 6;
const ACL_ENTRY_ONLY_INHERIT: c_int = 1 << 8;

const ACL_ADD_FILE: u64 = 1 << 2;
const ACL_SEARCH: u64 = 1 << 3;
const ACL_DELETE: u64 = 1 << 4;
const ACL_ADD_SUBDIRECTORY: u64 = 1 << 5;
const ACL_DELETE_CHILD: u64 = 1 << 6;
const ACL_READ_SECURITY: u64 = 1 << 11;
const ACL_WRITE_SECURITY: u64 = 1 << 12;
const ACL_CHANGE_OWNER: u64 = 1 << 13;
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
    /// The entries of what a read returned, with the system's error still current if that is null.
    /// No list, and a file system with no lists, read as no entries. `ENOENT` is also what a name
    /// that is gone gives, and the caller has just found it there.
    ///
    /// # Safety
    ///
    /// `list` must be null, or a list an `acl_*` call has just handed over, which this then frees.
    unsafe fn read(list: *mut c_void) -> io::Result<Vec<AccessEntry>> {
        if !list.is_null() {
            return Self(list).entries();
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ENOENT | libc::ENOTSUP | libc::EOPNOTSUPP) => Ok(Vec::new()),
            _ => Err(error),
        }
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
        applies_here: !flags.only_inherit,
        reaches_new_files: flags.to_files,
        reaches_new_directories: flags.to_directories,
    }
}

pub(super) fn list_at(path: &Path) -> io::Result<Vec<AccessEntry>> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::from)?;
    // SAFETY: `path` is NUL-terminated, outlives the call and is only read, and `List::read` takes
    // what the call hands over, with its error still current.
    unsafe { List::read(acl_get_link_np(path.as_ptr(), ACL_TYPE_EXTENDED)) }
}

fn list_of(file: &File) -> io::Result<Vec<AccessEntry>> {
    // SAFETY: the descriptor is borrowed from `file` for the call, and `List::read` takes what the
    // call hands over, with its error still current.
    unsafe { List::read(acl_get_fd_np(file.as_raw_fd(), ACL_TYPE_EXTENDED)) }
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
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::test_list::{clear, set_for_everyone, set_on_link_for_everyone};
    use super::{
        ACL_EXTENDED_DENY, CHANGING_RIGHTS, Flags, List, acl_get_entry, acl_init, classified,
        list_at, list_of, seal,
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
}
