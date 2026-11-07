//! The text of the list of emergency copies: one row per copy, as `*Messages*` escapes names
//! (ADR 0064). It holds no text of a copy.
//!
//! Must not depend on: `app`, `dispatch` or winit, or the file system.

use std::time::{Duration, SystemTime};

use oso_core::display_map::TabWidth;
use oso_core::editor::row_text;
use oso_core::load::{LoadSnapshot, decode_all};

use super::{MAX_SHOWN, Row};

/// How long ago `written` was, for a person.
pub(in crate::emergency) fn age(written: Option<u64>, now: SystemTime) -> String {
    let Some(written) = written else {
        return "?".to_owned();
    };
    let then = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(written));
    let Some(ago) = then.and_then(|then| now.duration_since(then).ok()) else {
        return "in the future".to_owned();
    };
    let plural = |count: u64, one: &str| {
        let s = if count == 1 { "" } else { "s" };
        format!("{count} {one}{s} ago")
    };
    match ago.as_secs() {
        0..60 => "just now".to_owned(),
        seconds @ 60..3600 => format!("{} min ago", seconds / 60),
        seconds @ 3600..86_400 => plural(seconds / 3600, "hour"),
        seconds => plural(seconds / 86_400, "day"),
    }
}

/// `text`, escaped and no longer than a row shows.
fn shown_in_a_row(text: &str) -> String {
    let mut shown = row_text(text);
    if shown.len() > MAX_SHOWN {
        shown.truncate(shown.floor_char_boundary(MAX_SHOWN));
        shown.push('…');
    }
    shown
}

fn size_text(size: u64) -> String {
    if size < 10 * 1024 {
        format!("{size} B")
    } else if size < 10 * 1024 * 1024 {
        format!("{} KiB", size / 1024)
    } else {
        format!("{} MiB", size / (1024 * 1024))
    }
}

/// The text of the list, one row per copy after [`HEADER_ROWS`], as `*Messages*` escapes names.
fn list_text(rows: &[Row], more: usize, problem: Option<&str>, now: SystemTime) -> String {
    use std::fmt::Write as _;
    let count = rows.len() + more;
    let copies = if count == 1 { "copy" } else { "copies" };
    let mut text = format!(
        "{count} emergency {copies}.  v look  = compare  r recover  d delete  g refresh  q back\n\n"
    );
    for row in rows {
        let _ = writeln!(
            text,
            "{:>13}  {:<5}  {:>8}  {:<15}  {}",
            age(row.written, now),
            row.why.unwrap_or("?"),
            size_text(row.size),
            row.word.as_str(),
            shown_in_a_row(&row.shown),
        );
    }
    if more > 0 {
        let _ = writeln!(text, "and {more} more");
    }
    if let Some(problem) = problem {
        let _ = writeln!(text, "Not read: {}", shown_in_a_row(problem));
    }
    text
}

pub(super) fn list_of(
    rows: &[Row],
    more: usize,
    problem: Option<&str>,
    now: SystemTime,
) -> LoadSnapshot {
    decode_all(
        list_text(rows, more, problem, now).as_bytes(),
        TabWidth::default(),
    )
}
