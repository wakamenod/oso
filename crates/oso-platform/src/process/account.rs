//! The account oso runs as, from the user database, on Unix: its login shell,
//! for the order of shells a terminal tries (ADR 0120).
//!
//! Must not depend on: `control` or `text`.

use std::ffi::{CStr, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// The largest buffer the user database is given for one account.
const MAX_ACCOUNT_BUF: usize = 64 * 1024;

/// The login shell of the account oso runs as, if the user database has one
/// that is an absolute path.
pub(super) fn shell() -> Option<PathBuf> {
    let mut size = 1024;
    loop {
        let mut buf = vec![0u8; size];
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: `getpwuid_r` fills `entry` and writes its strings into `buf`, up to
        // `buf.len()` bytes, and sets `found` to `entry` or null. `geteuid` cannot
        // fail.
        let error = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                entry.as_mut_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                &raw mut found,
            )
        };
        if error == libc::ERANGE && size < MAX_ACCOUNT_BUF {
            size *= 2;
            continue;
        }
        if error != 0 || found.is_null() {
            return None;
        }
        // SAFETY: `getpwuid_r` succeeded and found the account, so it filled `entry`.
        let entry = unsafe { entry.assume_init_ref() };
        if entry.pw_shell.is_null() {
            return None;
        }
        // SAFETY: the shell of a filled entry, when not null, is a NUL-terminated
        // string in `buf`, which lives until the copy is made.
        let shell = unsafe { CStr::from_ptr(entry.pw_shell) }
            .to_bytes()
            .to_vec();
        let shell = PathBuf::from(OsStr::from_bytes(&shell));
        return shell.is_absolute().then_some(shell);
    }
}
