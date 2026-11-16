//! Reading the lines a list of a server's places shows (ADR 0138, step 8b): the lines of one
//! file a server's answer names, found by counting line breaks from its start in one pass, as
//! the preview finds a server's line, and reached as the preview reaches its file.
//!
//! Must not depend on: `app`, winit or the UI thread; this reads the file system for the rows'
//! worker.
//!
//! - The file is opened as the preview opens it: the root by its path, never one that is or
//!   holds the home directory, then each name on the way, following no link; only a regular
//!   file is read, and one that may hold secrets by its name is never.
//! - At most [`SCAN_MAX`] bytes of a file are read, and no more than the budget the caller
//!   keeps for all its files; a line past either is not found. At most [`LINE_KEPT`] bytes of
//!   each line are kept.
//! - `live` is asked before the file is opened, at each step on the way and before each part.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::{PART, SCAN_MAX, open};
use super::{may_hold_secrets, walk};

/// The most bytes of a line kept for its row, which shows less.
pub(crate) const LINE_KEPT: usize = 1024;

/// What was read of a file for the rows.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RowLines {
    /// The file may hold secrets: nothing of it was read.
    Withheld,
    /// It could not be reached or read as the preview would reach it.
    Unreadable,
    /// The lines found, each by its number from 1, ascending; one past the file's end or a
    /// bound is left out.
    Read(Vec<(usize, Vec<u8>)>),
}

/// The lines `lines` (from 1, ascending, each once) of the file at `path` from `root`, where
/// `leads` says the root leads, never under one that is or holds `home`; what is read comes off
/// `budget`. `None` once `live` says the read is stale.
pub(crate) fn read_lines(
    (root, leads): (&Path, Option<&[PathBuf]>),
    path: &Path,
    lines: &[usize],
    home: Option<&Path>,
    budget: &mut u64,
    live: &dyn Fn() -> bool,
) -> Option<RowLines> {
    if !live() {
        return None;
    }
    if may_hold_secrets(root, leads, path) {
        return Some(RowLines::Withheld);
    }
    if walk::holds_home(root, home) {
        return Some(RowLines::Unreadable);
    }
    let Some((file, _)) = open((root, leads), path, home, live) else {
        return live().then_some(RowLines::Unreadable);
    };
    scan(file, lines, budget, live).map(RowLines::Read)
}

/// Finds `lines` in `file` from its start, a part at a time.
fn scan(
    mut file: impl Read,
    lines: &[usize],
    budget: &mut u64,
    live: &dyn Fn() -> bool,
) -> Option<Vec<(usize, Vec<u8>)>> {
    let mut wanted = lines.iter().copied().filter(|line| *line > 0).peekable();
    let mut found = Vec::new();
    // The line the read is in, and what is kept of it while it is a wanted one.
    let mut line = 1;
    let mut kept: Option<Vec<u8>> = (wanted.peek() == Some(&1)).then(Vec::new);
    let mut part = vec![0; PART];
    let mut read: u64 = 0;
    let limit = SCAN_MAX.min(*budget);
    while read < limit && (kept.is_some() || wanted.peek().is_some()) {
        if !live() {
            return None;
        }
        let size = usize::try_from(limit - read).map_or(PART, |rest| rest.min(PART));
        let Ok(count) = file.read(&mut part[..size]) else {
            break;
        };
        if count == 0 {
            break;
        }
        read += count as u64;
        let bytes = &part[..count];
        let mut at = 0;
        while at < bytes.len() {
            if let Some(text) = &mut kept {
                let rest = &bytes[at..];
                let end = rest.iter().position(|&byte| byte == b'\n');
                let piece = &rest[..end.unwrap_or(rest.len())];
                let room = LINE_KEPT.saturating_sub(text.len());
                text.extend_from_slice(&piece[..piece.len().min(room)]);
                let Some(end) = end else {
                    break;
                };
                found.push((line, kept.take().unwrap_or_default()));
                wanted.next();
                line += 1;
                at += end + 1;
                if wanted.peek() == Some(&line) {
                    kept = Some(Vec::new());
                }
                continue;
            }
            let Some(&next) = wanted.peek() else {
                break;
            };
            if next <= line {
                // One asked for twice, or out of order, is passed; this line is wanted now.
                if next == line {
                    kept = Some(Vec::new());
                } else {
                    wanted.next();
                }
                continue;
            }
            let need = next - line;
            let rest = &bytes[at..];
            // A whole part's breaks counted at once, which the compiler vectorizes; only the
            // part the line starts in is walked.
            let breaks = rest.iter().filter(|&&byte| byte == b'\n').count();
            if breaks < need {
                line += breaks;
                break;
            }
            let Some((index, _)) = (rest.iter().enumerate())
                .filter(|&(_, &byte)| byte == b'\n')
                .nth(need - 1)
            else {
                break;
            };
            line = next;
            at += index + 1;
            kept = Some(Vec::new());
        }
    }
    *budget = budget.saturating_sub(read);
    // The last line, with no break after it, once the file ended there.
    if let Some(text) = kept
        && read < limit
    {
        found.push((line, text));
    }
    Some(found)
}

#[cfg(test)]
mod tests;
