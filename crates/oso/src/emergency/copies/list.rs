//! The text of the list of emergency copies: one row per copy, as `*Messages*` escapes names
//! (ADR 0064). It holds no text of a copy.
//!
//! Must not depend on: `app`, `dispatch` or winit, or the file system.

use std::ffi::OsStr;
use std::path::Path;
use std::time::{Duration, SystemTime};

use oso_core::display_map::{TabWidth, text_clusters};
use oso_core::editor::row_text;
use oso_core::load::{LoadSnapshot, decode_all};

use super::{MAX_SHOWN, Row};
use crate::emergency::plural;

/// The most cells a name takes at the start of a row, as what follows it must stay in view.
const MAX_NAME_CELLS: usize = 32;

/// How long ago `written` was, for a person.
pub(in crate::emergency) fn age(written: Option<u64>, now: SystemTime) -> String {
    let Some(written) = written else {
        return "?".to_owned();
    };
    let then = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(written));
    let Some(ago) = then.and_then(|then| now.duration_since(then).ok()) else {
        return "in the future".to_owned();
    };
    let ago_in = |count: u64, one: &str| {
        let s = if count == 1 { "" } else { "s" };
        format!("{count} {one}{s} ago")
    };
    match ago.as_secs() {
        0..60 => "just now".to_owned(),
        seconds @ 60..3600 => format!("{} min ago", seconds / 60),
        seconds @ 3600..86_400 => ago_in(seconds / 3600, "hour"),
        seconds => ago_in(seconds / 86_400, "day"),
    }
}

/// The last name of what a row shows of where a copy belongs. It comes from the path and not
/// from a `.meta`'s `name`, so that a row cannot name one file and belong to another.
fn file_name_of(shown: &str) -> &str {
    Path::new(shown)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(shown)
}

/// The name that starts a row, escaped and cut to [`MAX_NAME_CELLS`], with the cells it takes.
/// A window cuts a row at its edge, so what says which file it is comes first, and all of it
/// is in the path later in the row.
fn name_in_a_row(shown: &str) -> (String, usize) {
    let escaped = row_text(file_name_of(shown));
    let mut end = 0;
    let mut cells = 0;
    for cluster in text_clusters(&escaped, 0) {
        if cells + cluster.cells >= MAX_NAME_CELLS {
            let mut cut = escaped[..end].to_owned();
            cut.push('…');
            return (cut, cells + 1);
        }
        cells += cluster.cells;
        end = cluster.offset + cluster.text.len();
    }
    (escaped, cells)
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
    let copies = plural(count, "copy", "copies");
    let mut text = format!(
        "{count} emergency {copies}.  v look  = compare  r recover  d delete  g refresh  q back\n\n"
    );
    let names: Vec<(String, usize)> = rows.iter().map(|row| name_in_a_row(&row.shown)).collect();
    let width = names.iter().map(|(_, cells)| *cells).max().unwrap_or(0);
    for (row, (name, cells)) in rows.iter().zip(names) {
        let _ = write!(
            text,
            "{name}{:padding$}  {:>13}  {:<5}  {:>8}  {:<15}",
            "",
            age(row.written, now),
            row.why.unwrap_or("?"),
            size_text(row.size),
            row.state.as_str(),
            padding = width - cells,
        );
        // A buffer with no file has only its name to show.
        if file_name_of(&row.shown) != row.shown {
            let _ = write!(text, "  {}", shown_in_a_row(&row.shown));
        }
        text.push('\n');
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
