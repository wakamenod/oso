//! A review's message to an agent session (ADR 0122, "Sending comments to the agent", step
//! 12): the person's comments that were not sent, each with its place and its hunk, as one
//! text the person reads and may change in `*review message*` before it is pasted.
//!
//! Must not depend on: `app`, `dispatch`, running git, the file system or any thread; a pure function
//! over the model's comments and readings, bounded by the message's size, so the UI thread
//! runs it.
//!
//! - Each comment goes as the previous setup wrote it: `## PATH  L42 (new)`, its hunk in a
//!   `diff` fence longer than any run of backticks in it, the comment it replies to, and the
//!   comment. A hunk of more than [`MAX_HUNK_LINES`] lines is cut to the [`CUT_TO`] lines
//!   around the comment, saying so, and a line to [`MAX_LINE_CHARS`] characters.
//! - An outdated comment goes with the place it was on and no hunk, as its reading's lines may
//!   be gone (Q4); a file whose name may hold secrets with its place alone.
//! - Paths, and the text of the comment replied to, which may be an agent's, are escaped as
//!   git's text is shown, so a name cannot start a line that reads as another comment's; a
//!   hunk's line has its control characters but tabs, and the line separators, escaped, so
//!   that no line of it becomes several once pasted, where a lone `\r` breaks a line.
//! - The message holds the comments in the order given until the next would pass `max`; the
//!   rest are left for a later message, and the caller says how many.

use std::fmt::Write as _;

use oso_core::review::{Anchor, Author, Comment, Hunk, Shape, Side};

use super::hunk_header;
use crate::git::printable;

/// The line a message starts with.
pub(crate) const HEAD: &str = "Review comments on the changes below. Please act on each of them.";
/// The most lines of a hunk a message holds whole.
pub(crate) const MAX_HUNK_LINES: usize = 60;
/// The lines around the comment a longer hunk is cut to.
pub(crate) const CUT_TO: usize = 30;
/// The most characters of a hunk's line a message holds; past them, `…`.
pub(crate) const MAX_LINE_CHARS: usize = 500;
/// The most characters of a path shown.
const MAX_PATH_CHARS: usize = 300;
/// The most characters of the comment replied to: its first line.
const MAX_PARENT_CHARS: usize = 200;

/// One comment to send, with what its entry needs from the model.
pub(crate) struct Item<'a> {
    pub(crate) comment: &'a Comment,
    /// The comment it replies to, if the review still holds it.
    pub(crate) parent: Option<&'a Comment>,
    /// The model's current reading of the comment's file, in which a comment that is not
    /// outdated is placed.
    pub(crate) shape: Option<&'a Shape>,
}

/// A message, and how many of the items given, from the first, it holds.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Message {
    pub(crate) text: String,
    pub(crate) taken: usize,
}

/// The message of `items`, in their order, under the line naming `scope`, at most `max`
/// bytes: the items past it are left out. `secret` says whether a file's name may hold
/// secrets, so that its lines are not sent; it is asked only of the items composed.
pub(crate) fn compose(
    scope: &str,
    items: &[Item<'_>],
    max: usize,
    secret: impl Fn(&[u8]) -> bool,
) -> Message {
    let mut text = format!("{HEAD}\nScope: {}.", printable(scope, MAX_PATH_CHARS));
    let mut taken = 0;
    for item in items {
        let entry = entry(item, secret(item.comment.path()));
        if text.len() + 2 + entry.len() > max {
            break;
        }
        text.push_str("\n\n");
        text.push_str(&entry);
        taken += 1;
    }
    Message { text, taken }
}

/// One comment's entry: its place, its hunk, what it replies to and its text.
fn entry(item: &Item<'_>, secret: bool) -> String {
    let comment = item.comment;
    let path = printable(&String::from_utf8_lossy(comment.path()), MAX_PATH_CHARS);
    let hunk = (!comment.is_outdated())
        .then(|| {
            item.shape
                .and_then(|shape| hunk_of(shape, comment.anchor()))
        })
        .flatten();
    let mut out = String::new();
    match (comment.is_outdated(), hunk) {
        (false, Some((hunk, _))) => {
            let _ = write!(out, "## {path}  {}", place(comment.anchor(), Some(hunk)));
        }
        _ => {
            let _ = write!(
                out,
                "## {path}  (outdated, was {})",
                place(comment.anchor(), None)
            );
        }
    }
    if let Some((hunk, at)) = hunk {
        if secret {
            out.push_str("\n(The file may hold secrets: its lines are not sent.)");
        } else {
            out.push('\n');
            lines_of(&mut out, hunk, at);
        }
    }
    if let Some(parent) = item.parent {
        let whose = match parent.author() {
            Author::Agent => "the agent's ",
            Author::Person => "",
        };
        let first = parent.text().lines().next().unwrap_or("");
        let _ = write!(
            out,
            "\nIn reply to {whose}#{}: {}",
            parent.id().get(),
            printable(first, MAX_PARENT_CHARS)
        );
    }
    out.push_str("\nComment: ");
    out.push_str(comment.text());
    out
}

/// The hunk `anchor` is in, and where in its lines, removed then added, the anchor is: `None`
/// for a whole hunk.
fn hunk_of(shape: &Shape, anchor: Anchor) -> Option<(&Hunk, Option<usize>)> {
    match anchor {
        Anchor::Line { side, number } => {
            let hunk = &shape.hunks()[shape.hunk_at(side, number)?];
            let at = match side {
                Side::Old => (number - hunk.old_start) as usize,
                Side::New => hunk.removed.len() + (number - hunk.new_start) as usize,
            };
            Some((hunk, Some(at)))
        }
        Anchor::Hunk { index } => Some((shape.hunks().get(index)?, None)),
    }
}

/// Where a comment is: `L42 (new)`, a whole hunk as the lines it spans, `L40-L48 (new)`, or,
/// with no hunk known, `hunk 3`.
fn place(anchor: Anchor, hunk: Option<&Hunk>) -> String {
    let side = |side: Side| match side {
        Side::Old => "old",
        Side::New => "new",
    };
    match (anchor, hunk) {
        (Anchor::Line { side: s, number }, _) => format!("L{number} ({})", side(s)),
        (Anchor::Hunk { .. }, Some(hunk)) => {
            let (start, count, s) = if hunk.added.is_empty() {
                (hunk.old_start, hunk.removed.len(), Side::Old)
            } else {
                (hunk.new_start, hunk.added.len(), Side::New)
            };
            let last = start.saturating_add(u32::try_from(count).unwrap_or(u32::MAX) - 1);
            if last == start {
                format!("L{start} ({})", side(s))
            } else {
                format!("L{start}-L{last} ({})", side(s))
            }
        }
        (Anchor::Hunk { index }, None) => format!("hunk {}", index.saturating_add(1)),
    }
}

/// Writes `hunk` in a `diff` fence, cut to the [`CUT_TO`] lines around `at` when it has more
/// than [`MAX_HUNK_LINES`], saying how many were left out.
fn lines_of(out: &mut String, hunk: &Hunk, at: Option<usize>) {
    let len = hunk.removed.len() + hunk.added.len();
    let (from, to) = if len > MAX_HUNK_LINES {
        let from = at
            .map_or(0, |at| at.saturating_sub(CUT_TO / 2))
            .min(len - CUT_TO);
        (from, from + CUT_TO)
    } else {
        (0, len)
    };
    // Only the lines shown are looked at, whatever the hunk holds.
    let line = |index: usize| match hunk.removed.get(index) {
        Some(line) => ('-', &**line),
        None => ('+', &*hunk.added[index - hunk.removed.len()]),
    };
    let shown: Vec<String> = (from..to)
        .map(line)
        .map(|(sign, line)| format!("{sign}{}", line_text(line)))
        .collect();
    let longest = shown.iter().map(|line| backticks(line)).max().unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    let _ = write!(out, "{fence}diff\n{}", hunk_header(hunk));
    for line in &shown {
        out.push('\n');
        out.push_str(line);
    }
    let _ = write!(out, "\n{fence}");
    if (from, to) != (0, len) {
        let _ = write!(
            out,
            "\n(The hunk's {len} lines are cut to the {CUT_TO} around the comment: {from} above \
             and {} below are left out.)",
            len - to
        );
    }
}

/// A line's text without its ending, at most [`MAX_LINE_CHARS`] characters.
pub(crate) fn line_text(line: &[u8]) -> String {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    // No character takes more than 4 bytes, so what is shown is in these, however long the
    // line: a cut character at their end is a replacement character past the cut.
    let line = &line[..line.len().min(MAX_LINE_CHARS * 4 + 4)];
    let text = String::from_utf8_lossy(line);
    let mut out = String::with_capacity(text.len().min(MAX_LINE_CHARS * 4));
    for (count, c) in text.chars().enumerate() {
        if count == MAX_LINE_CHARS {
            out.push('\u{2026}');
            break;
        }
        if breaks(c) {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether `c` could end a line once pasted, or does in some reader, though the preview shows
/// it inside one: a control character but a tab (a paste makes a lone `\r` a line break), and
/// the line and paragraph separators. So a hunk's line stays one line of the message.
fn breaks(c: char) -> bool {
    (c.is_control() && c != '\t') || matches!(c, '\u{2028}' | '\u{2029}')
}

/// The longest run of backticks in `text`.
pub(crate) fn backticks(text: &str) -> usize {
    let (mut longest, mut run) = (0, 0);
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

#[cfg(test)]
mod tests;
