//! The names of users and groups, from the system's databases, for a listing's owner column
//! (ADR 0134).
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

/// The largest buffer one lookup is given.
#[cfg(unix)]
const MAX_BUF: usize = 64 * 1024;

/// The name of the user `uid`, if the user database has one.
#[cfg(unix)]
pub(super) fn user_name(uid: u32) -> Option<String> {
    let mut size = 1024;
    loop {
        let mut buf = vec![0u8; size];
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: `getpwuid_r` fills `entry` and writes its strings into `buf`, up to
        // `buf.len()` bytes, and sets `found` to `entry` or null; all three outlive the call.
        let error = unsafe {
            libc::getpwuid_r(
                uid,
                entry.as_mut_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                &raw mut found,
            )
        };
        if error == libc::ERANGE && size < MAX_BUF {
            size *= 2;
            continue;
        }
        if error != 0 || found.is_null() {
            return None;
        }
        // SAFETY: `getpwuid_r` succeeded and found the user, so it filled `entry`.
        let entry = unsafe { entry.assume_init_ref() };
        return name_of(entry.pw_name);
    }
}

/// The name of the group `gid`, if the group database has one.
#[cfg(unix)]
pub(super) fn group_name(gid: u32) -> Option<String> {
    let mut size = 1024;
    loop {
        let mut buf = vec![0u8; size];
        let mut entry = std::mem::MaybeUninit::<libc::group>::uninit();
        let mut found: *mut libc::group = std::ptr::null_mut();
        // SAFETY: `getgrgid_r` fills `entry` and writes its strings into `buf`, up to
        // `buf.len()` bytes, and sets `found` to `entry` or null; all three outlive the call.
        let error = unsafe {
            libc::getgrgid_r(
                gid,
                entry.as_mut_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                &raw mut found,
            )
        };
        if error == libc::ERANGE && size < MAX_BUF {
            size *= 2;
            continue;
        }
        if error != 0 || found.is_null() {
            return None;
        }
        // SAFETY: `getgrgid_r` succeeded and found the group, so it filled `entry`.
        let entry = unsafe { entry.assume_init_ref() };
        return name_of(entry.gr_name);
    }
}

/// A copy of the NUL-terminated name at `name`, if it is not null and is UTF-8.
#[cfg(unix)]
fn name_of(name: *const libc::c_char) -> Option<String> {
    if name.is_null() {
        return None;
    }
    // SAFETY: a name in an entry the lookup filled, when not null, is a NUL-terminated string
    // in the buffer the caller still holds.
    let name = unsafe { std::ffi::CStr::from_ptr(name) };
    name.to_str().ok().map(str::to_owned)
}

#[cfg(not(unix))]
pub(super) fn user_name(_: u32) -> Option<String> {
    None
}

#[cfg(not(unix))]
pub(super) fn group_name(_: u32) -> Option<String> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn root_has_a_name() {
        assert_eq!(super::user_name(0).as_deref(), Some("root"));
        assert!(super::group_name(0).is_some());
    }

    #[test]
    fn an_unknown_id_has_none() {
        assert_eq!(super::user_name(0x7fff_fff0), None);
    }
}
