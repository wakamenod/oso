//! What `*lsp-rename*` shows of a rename's edits before they apply (ADR 0138, step 14, Q3): each
//! buffer, and each line that changes as it is and as it would be, built off the UI thread from
//! the very texts and ranges `y` applies.
//!
//! Must not depend on: the window system or the UI thread's state; this reads only the texts it
//! is given, never a file.
//!
//! - Every file the rename edits is a buffer by now, opened for it if none held it, so what
//!   shows is what applies: the same text, the same ranges.
//! - Lines an edit spans, or that touch, are one group; of a group, only the lines that differ
//!   show, so that a server that rewrites a whole file shows what it changes.
//! - A long line shows the part that differs, with its columns, over as many rows as it takes;
//!   nothing is cut out of what changes.
//! - Every line shown has its controls, and its bidi and invisible characters, written as
//!   `\u{…}`.
//! - Past [`GROUP_BYTES_MAX`] of a group's text, [`LINE_SHOWN_MAX`] changed characters of a
//!   line or [`PREVIEW_BYTES_MAX`] in all, or with edits that overlap, the rename is refused
//!   rather than shown in part: nothing that applies is left out.

use std::fmt::Write as _;
use std::ops::Range;

use oso_core::display_map::cell::is_invisible;
use oso_core::editor::BufferEdit;
use oso_core::rope::Rope;

/// The most characters of a preview row.
const ROW_CHARS: usize = 200;

/// Characters shown either side of what differs in a long line.
const CONTEXT: usize = 20;

/// The most bytes of the text a group spans.
const GROUP_BYTES_MAX: usize = 1 << 20;

/// The most characters shown of a line that differs.
const LINE_SHOWN_MAX: usize = 2000;

/// The most bytes of a preview, which the UI thread puts in its buffer.
const PREVIEW_BYTES_MAX: usize = 512 << 10;

/// Why a rename too large to show is refused.
const TOO_LARGE: &str = "its edits are too large to show";

/// What a preview is built from: who renames what to what, how many files it opened, and each
/// buffer's text and the path it is shown by, in the order of the edits' parts.
pub(super) struct Snapshot {
    pub(super) server: String,
    pub(super) old: String,
    pub(super) new: String,
    pub(super) opened: usize,
    pub(super) files: Vec<(String, Rope)>,
}

/// The preview's text for `parts`, each the edits of `snapshot.files`' buffer at the same place,
/// or why the rename is refused; `None` once `live` says it is stale.
pub(super) fn build(
    snapshot: &Snapshot,
    parts: &[BufferEdit],
    live: &dyn Fn() -> bool,
) -> Option<Result<String, &'static str>> {
    let edits: usize = parts.iter().map(|part| part.edits.len()).sum();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} renames \u{2018}{}\u{2019} to \u{2018}{}\u{2019}: {} in {}{}",
        shown(&snapshot.server),
        shown(&snapshot.old),
        shown(&snapshot.new),
        plural(edits, "edit"),
        plural(parts.len(), "file"),
        match snapshot.opened {
            0 => String::new(),
            count => format!(" ({count} opened for it, unshown)"),
        }
    );
    out.push_str(
        "y applies it, each buffer's edits one step to undo; n or q leaves all as it is\n",
    );
    for ((path, text), part) in snapshot.files.iter().zip(parts) {
        let _ = writeln!(
            out,
            "\n{} ({})",
            shown(path),
            plural(part.edits.len(), "edit")
        );
        match file_rows(text, &part.edits, &mut out, live)? {
            Err(why) => return Some(Err(why)),
            Ok(()) if out.len() > PREVIEW_BYTES_MAX => return Some(Err(TOO_LARGE)),
            Ok(()) => {}
        }
    }
    Some(Ok(out))
}

/// Writes the rows of `edits` of `text` to `out`; `None` once `live` says the preview is stale.
fn file_rows(
    text: &Rope,
    edits: &[(Range<usize>, String)],
    out: &mut String,
    live: &dyn Fn() -> bool,
) -> Option<Result<(), &'static str>> {
    let mut order: Vec<usize> = (0..edits.len()).collect();
    // Stable: insertions at one place keep their order, as `y` makes them.
    order.sort_by_key(|&at| (edits[at].0.start, edits[at].0.end));
    // `y` refuses edits that overlap, so the preview does too.
    let overlaps = order
        .windows(2)
        .any(|pair| edits[pair[1]].0.start < edits[pair[0]].0.end);
    if overlaps {
        return Some(Err("its edits overlap"));
    }
    let rows: Vec<(usize, usize)> = (order.iter())
        .map(|&at| {
            let range = &edits[at].0;
            let first = text.offset_to_point(range.start).row;
            (first, text.offset_to_point(range.end).row.max(first))
        })
        .collect();
    for (first, last, members) in spans(&rows) {
        if !live() {
            return None;
        }
        let span = text.line_start(first)..text.line_end(last);
        if span.len() > GROUP_BYTES_MAX {
            return Some(Err(TOO_LARGE));
        }
        let local: Vec<(Range<usize>, &str)> = (members.iter())
            .map(|&row| {
                let (range, new) = &edits[order[row]];
                (
                    range.start - span.start..range.end - span.start,
                    new.as_str(),
                )
            })
            .collect();
        let before = text.slice(span).to_string();
        let after = applied(&before, &local);
        if let Err(why) = write_group(out, first, &before, &after) {
            return Some(Err(why));
        }
        if out.len() > PREVIEW_BYTES_MAX {
            return Some(Err(TOO_LARGE));
        }
    }
    Some(Ok(()))
}

/// The edits' rows, each `(first, last)` in ascending order, joined where they overlap or
/// touch: each group's first and last row, and its members by their place in `rows`.
fn spans(rows: &[(usize, usize)]) -> Vec<(usize, usize, Vec<usize>)> {
    let mut groups: Vec<(usize, usize, Vec<usize>)> = Vec::new();
    for (at, &(first, last)) in rows.iter().enumerate() {
        match groups.last_mut() {
            Some(group) if first <= group.1 + 1 => {
                group.1 = group.1.max(last);
                group.2.push(at);
            }
            _ => groups.push((first, last, vec![at])),
        }
    }
    groups
}

/// `text` with `edits`, ranges of its bytes in ascending order that do not overlap, made; at one
/// place, in their order, as `y` makes them.
fn applied(text: &str, edits: &[(Range<usize>, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (range, new) in edits {
        out.push_str(&text[at..range.start]);
        out.push_str(new);
        at = range.end;
    }
    out.push_str(&text[at..]);
    out
}

/// Writes the lines of a group from row `first` that differ between `before` and `after`,
/// numbered from 1: as they are (`-`), then as they would be (`+`).
fn write_group(
    out: &mut String,
    first: usize,
    before: &str,
    after: &str,
) -> Result<(), &'static str> {
    let old: Vec<&str> = before.split('\n').collect();
    let new: Vec<&str> = after.split('\n').collect();
    let same_start = (old.iter().zip(&new)).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - same_start;
    let same_end = (old.iter().rev().zip(new.iter().rev()))
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    let old = &old[same_start..old.len() - same_end];
    let new = &new[same_start..new.len() - same_end];
    let number = first + same_start + 1;
    // Line by line, each shows the part that differs from its counterpart.
    let paired = old.len() == new.len();
    for (at, line) in old.iter().enumerate() {
        let window = paired.then(|| differing(line, new[at]).0);
        write_line(out, (number + at, '-'), line, window)?;
    }
    for (at, line) in new.iter().enumerate() {
        let window = paired.then(|| differing(old[at], line).1);
        write_line(out, (number + at, '+'), line, window)?;
    }
    Ok(())
}

/// The characters of `a` and of `b` that differ between them: what lies between their common
/// start and their common end.
fn differing(a: &str, b: &str) -> (Range<usize>, Range<usize>) {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let start = (a.iter().zip(&b)).take_while(|(x, y)| x == y).count();
    let room = a.len().min(b.len()) - start;
    let end = (a.iter().rev().zip(b.iter().rev()))
        .take(room)
        .take_while(|(x, y)| x == y)
        .count();
    (start..a.len() - end, start..b.len() - end)
}

/// Writes `line`, number `number` with `sign`: whole if it fits a row; else the characters of
/// `window`, what differs, and [`CONTEXT`] either side, from their columns, over as many rows as
/// they take.
fn write_line(
    out: &mut String,
    (number, sign): (usize, char),
    line: &str,
    window: Option<Range<usize>>,
) -> Result<(), &'static str> {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= ROW_CHARS {
        let _ = writeln!(out, "{number:>6} {sign} {}", shown(line));
        return Ok(());
    }
    let window = window.unwrap_or(0..chars.len());
    let start = window.start.saturating_sub(CONTEXT);
    let end = (window.end + CONTEXT).min(chars.len());
    if end - start > LINE_SHOWN_MAX {
        return Err(TOO_LARGE);
    }
    let _ = writeln!(
        out,
        "{number:>6} {sign} (columns {} to {} of {})",
        start + 1,
        end,
        chars.len()
    );
    for row in chars[start..end].chunks(ROW_CHARS) {
        let text: String = row.iter().collect();
        let _ = writeln!(out, "       {sign} {}", shown(&text));
    }
    Ok(())
}

/// `text` on one line: its controls and its bidi and invisible characters as `\u{…}`.
pub(super) fn shown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() || is_invisible(c) {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

fn plural(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        count => format!("{count} {noun}s"),
    }
}

#[cfg(test)]
mod tests;
