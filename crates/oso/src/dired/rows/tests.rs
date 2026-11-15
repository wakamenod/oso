//! Tests of a listing's row text.
//!
//! Must not depend on: the file system.

use std::time::{Duration, SystemTime};

use super::*;

#[cfg(unix)]
#[test]
fn a_mode_reads_as_ls_writes_it() {
    assert_eq!(mode_text(0o040_755), "drwxr-xr-x");
    assert_eq!(mode_text(0o104_755), "-rwsr-xr-x");
}

#[test]
fn a_size_reads_as_ls_h_writes_it() {
    assert_eq!(size_text(0), "0");
    assert_eq!(size_text(1023), "1023");
    assert_eq!(size_text(1024), "1.0K");
    assert_eq!(size_text(1025), "1.1K");
    assert_eq!(size_text(4096), "4.0K");
    assert_eq!(size_text(10 * 1024 - 1), "10K");
    assert_eq!(size_text(10 * 1024), "10K");
    assert_eq!(size_text(1024 * 1024 - 1), "1.0M");
    assert_eq!(size_text(1_258_291), "1.2M");
    assert_eq!(size_text(1_258_292), "1.3M");
    assert_eq!(size_text(5 * 1024 * 1024 * 1024), "5.0G");
    assert_eq!(size_text(u64::MAX), "16E");
}

#[test]
fn a_time_reads_as_ls_writes_it() {
    // 2026-11-15 09:12:00 UTC.
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_794_733_920);
    let now = at + Duration::from_secs(3600);
    assert_eq!(time_text(Some(at), now, 0), "Nov 15 09:12");
    assert_eq!(time_text(Some(at), now, 9 * 3600), "Nov 15 18:12");
    assert_eq!(time_text(Some(at), now, -10 * 3600), "Nov 14 23:12");
    let old = at - Duration::from_secs(200 * 86_400);
    assert_eq!(time_text(Some(old), now, 0), "Apr 29  2026");
    let ahead = now + Duration::from_secs(86_400);
    assert_eq!(time_text(Some(ahead), now, 0), "Nov 16  2026");
    assert_eq!(time_text(None, now, 0), " ".repeat(12));
    let before = SystemTime::UNIX_EPOCH - Duration::from_secs(86_400);
    assert_eq!(time_text(Some(before), now, 0), "Dec 31  1969");
}

#[test]
fn days_become_dates() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(-1), (1969, 12, 31));
    assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    assert_eq!(civil_from_days(20_772), (2026, 11, 15));
}

#[test]
fn a_decoded_length_counts_a_replacement_for_each_invalid_byte() {
    assert_eq!(decoded_len(b"notes.md"), 8);
    assert_eq!(decoded_len("日本".as_bytes()), 6);
    assert_eq!(decoded_len(b"a\xffb"), 5);
    assert_eq!(decoded_len(b"\xe6\x97"), 6);
}

#[test]
fn a_line_break_in_a_name_never_starts_a_row() {
    let mut out = Vec::new();
    push_name(&mut out, b"a\nb\rc\td\\n");
    assert_eq!(out, b"a\\nb\\rc\td\\\\n");
}
