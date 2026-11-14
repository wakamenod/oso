//! A listing's rows as text (ADR 0134): the mode as `ls -l` writes it, the size as `ls -h`, the
//! time as `ls`, and names as their bytes, with what each part's face is.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or the file system; these are pure
//! functions of what was read.

use std::time::{Duration, SystemTime};

/// Unix mode bits as `ls -l` writes them, such as `drwxr-xr-x` or `-rwsr-x--T`.
pub(crate) fn mode_text(mode: u32) -> String {
    let kind = match mode & 0o170_000 {
        0o040_000 => 'd',
        0o120_000 => 'l',
        0o010_000 => 'p',
        0o020_000 => 'c',
        0o060_000 => 'b',
        0o140_000 => 's',
        _ => '-',
    };
    let mut text = String::with_capacity(10);
    text.push(kind);
    // Each of owner, group and others: read, write, and run with what a set id or the sticky
    // bit puts in its place.
    let specials = [(0o4000, 's'), (0o2000, 's'), (0o1000, 't')];
    for (index, (special, letter)) in specials.into_iter().enumerate() {
        let shift = 6 - 3 * index;
        let bits = (mode >> shift) & 0o7;
        text.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        text.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        let runs = bits & 0o1 != 0;
        text.push(match (mode & special != 0, runs) {
            (true, true) => letter,
            (true, false) => letter.to_ascii_uppercase(),
            (false, true) => 'x',
            (false, false) => '-',
        });
    }
    text
}

/// Windows attribute bits as PowerShell's `Mode` column writes them: `d----`, `-a---`, `la---`.
pub(crate) fn attributes_text(attributes: u32, directory: bool) -> String {
    const ARCHIVE: u32 = 0x20;
    const READ_ONLY: u32 = 0x1;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    const REPARSE: u32 = 0x400;
    let flag = |bit: u32, letter: char| if attributes & bit != 0 { letter } else { '-' };
    let first = if attributes & REPARSE != 0 {
        'l'
    } else if directory {
        'd'
    } else {
        '-'
    };
    [
        first,
        flag(ARCHIVE, 'a'),
        flag(READ_ONLY, 'r'),
        flag(HIDDEN, 'h'),
        flag(SYSTEM, 's'),
    ]
    .into_iter()
    .collect()
}

/// A size as `ls -h` writes it: bytes below 1024, then one decimal below ten of a unit and
/// whole units above, each rounded up, in powers of 1024: `512`, `4.0K`, `1.2M`, `12G`.
pub(crate) fn size_text(bytes: u64) -> String {
    const UNITS: [char; 6] = ['K', 'M', 'G', 'T', 'P', 'E'];
    if bytes < 1024 {
        return bytes.to_string();
    }
    let mut unit = 0;
    let mut scale: u128 = 1024;
    let bytes = u128::from(bytes);
    while bytes.div_ceil(scale) >= 1024 && unit + 1 < UNITS.len() {
        scale *= 1024;
        unit += 1;
    }
    let tenths = (bytes * 10).div_ceil(scale);
    if tenths < 100 {
        return format!("{}.{}{}", tenths / 10, tenths % 10, UNITS[unit]);
    }
    let whole = bytes.div_ceil(scale);
    // A value that rounds up to 1024 of a unit is one of the next, as `ls` says it.
    if whole >= 1024 && unit + 1 < UNITS.len() {
        return format!("1.0{}", UNITS[unit + 1]);
    }
    format!("{whole}{}", UNITS[unit])
}

/// Six months, as GNU `ls` counts them: half of 365.2425 days.
const SIX_MONTHS: Duration = Duration::from_secs(15_778_476);

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A time as `ls -l` writes it, 12 cells: `Nov 15 09:12` within six months before `now`, else
/// `Nov 15  2025`, as also for a time after `now`. `offset` is the local zone's at `time`, in
/// seconds east of UTC. An unknown time is blank.
pub(crate) fn time_text(time: Option<SystemTime>, now: SystemTime, offset: i32) -> String {
    let Some(time) = time else {
        return " ".repeat(12);
    };
    let seconds = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
    };
    let local = seconds.saturating_add(i64::from(offset));
    let days = local.div_euclid(86_400);
    let of_day = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let month = MONTHS[usize::try_from(month - 1).unwrap_or(0).min(11)];
    let recent = time <= now && now.duration_since(time).is_ok_and(|ago| ago < SIX_MONTHS);
    if recent {
        format!(
            "{month} {day:>2} {:02}:{:02}",
            of_day / 3600,
            of_day % 3600 / 60
        )
    } else {
        format!("{month} {day:>2} {year:>5}")
    }
}

/// The year, month (1–12) and day (1–31) of a count of days since 1970-01-01, in the proleptic
/// Gregorian calendar (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The bytes `bytes` take in a buffer's text once decoded, as a load decodes them: valid UTF-8
/// as it is, and three bytes, a U+FFFD, for each invalid byte (ADR 0022).
pub(crate) fn decoded_len(bytes: &[u8]) -> usize {
    bytes
        .utf8_chunks()
        .map(|chunk| chunk.valid().len() + 3 * chunk.invalid().len())
        .sum()
}

/// A name's bytes as a row shows them: a line break or a carriage return as `\n` or `\r`, so
/// that no name starts a row or ends one early. Anything else is left for the display, which
/// shows control, invisible and bidirectional characters and invalid bytes as such.
pub(crate) fn push_name(out: &mut Vec<u8>, name: &[u8]) {
    for byte in name {
        match byte {
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            _ => out.push(*byte),
        }
    }
}

#[cfg(test)]
mod tests;
