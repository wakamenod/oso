//! The local time zone's offset from UTC at a moment, for a listing's dates (ADR 0134).
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::time::SystemTime;

/// The local offset from UTC at `time`, in seconds east, as the system's time zone says.
#[cfg(unix)]
pub(super) fn utc_offset_at(time: SystemTime) -> Option<i32> {
    let seconds = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => libc::time_t::try_from(after.as_secs()).ok()?,
        Err(before) => -libc::time_t::try_from(before.duration().as_secs()).ok()?,
    };
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: `seconds` and `local` outlive the call, which reads one and fills the other, and
    // returns null if it could not.
    let filled = unsafe { libc::localtime_r(&raw const seconds, local.as_mut_ptr()) };
    if filled.is_null() {
        return None;
    }
    // SAFETY: `localtime_r` succeeded, so it filled `local`.
    let local = unsafe { local.assume_init() };
    i32::try_from(local.tm_gmtoff).ok()
}

/// Not read here yet: dates show in UTC.
#[cfg(not(unix))]
pub(super) fn utc_offset_at(_: SystemTime) -> Option<i32> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn an_offset_is_within_a_day() {
        let offset = super::utc_offset_at(std::time::SystemTime::now()).unwrap();
        assert!(offset.abs() <= 14 * 3600);
    }
}
