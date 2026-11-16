//! Cutting a patch down to the lines a region chose (ADR 0127, step 1c), as magit cuts it, and
//! the patch of a file's hunks, each whole or cut, with their headers counted again.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; these are pure functions.
//!
//! - Applied forward (staging), an unchosen `-` line becomes context and an unchosen `+` line
//!   goes. Applied in reverse (unstaging, discarding), an unchosen `+` line becomes context and
//!   an unchosen `-` line goes. So the side git looks for the hunk on, the old one forward and
//!   the new one in reverse, is the side git wrote, and its start stays.
//! - A `\ No newline at end of file` goes with the line before it, and becomes nothing when
//!   that line goes.
//! - The other side's start follows the hunks before it in the patch written, as git's own
//!   numbering would.

use super::patch::{Hunk, Patch, counts};

/// What a file's patch is made of: each hunk by its index, whole or cut down to the chosen
/// lines, by their index in the hunk, sorted.
pub(crate) type Picks = [(usize, Option<Vec<usize>>)];

/// The lines of `hunk` with only the changes of the `chosen` lines, as git applies them forward
/// or in `reverse`; `None` if no chosen line is a change.
pub(crate) fn cut(hunk: &Hunk, chosen: &[usize], reverse: bool) -> Option<Vec<Vec<u8>>> {
    let is_chosen = |index: usize| chosen.binary_search(&index).is_ok();
    let changes = chosen
        .iter()
        .filter_map(|&at| hunk.lines.get(at))
        .any(|line| matches!(line.first(), Some(b'-' | b'+')));
    if !changes {
        return None;
    }
    let (kept_as_context, dropped) = if reverse { (b'+', b'-') } else { (b'-', b'+') };
    let mut out: Vec<Vec<u8>> = Vec::with_capacity(hunk.lines.len());
    let mut last_dropped = false;
    for (index, line) in hunk.lines.iter().enumerate() {
        match line.first() {
            Some(b'\\') => {
                if !last_dropped {
                    out.push(line.clone());
                }
                continue;
            }
            Some(&mark) if mark == kept_as_context && !is_chosen(index) => {
                let mut context = line.clone();
                context[0] = b' ';
                out.push(context);
                last_dropped = false;
            }
            Some(&mark) if mark == dropped && !is_chosen(index) => last_dropped = true,
            _ => {
                out.push(line.clone());
                last_dropped = false;
            }
        }
    }
    Some(out)
}

/// The lengths of a hunk's old and new sides.
fn sides(lines: &[Vec<u8>]) -> (u64, u64) {
    let (mut old, mut new) = (0u64, 0u64);
    for line in lines {
        match line.first() {
            Some(b' ') => {
                old += 1;
                new += 1;
            }
            Some(b'-') => old += 1,
            Some(b'+') => new += 1,
            _ => {}
        }
    }
    (old, new)
}

/// The patch of `patch`'s file made of `picks`, applied forward or in `reverse`: its header,
/// then each hunk, whole as git wrote it or cut, with its header counted again; `None` if it
/// holds no change, or a hunk is not one that stages alone.
pub(crate) fn file_patch(patch: &Patch, picks: &Picks, reverse: bool) -> Option<Vec<u8>> {
    if patch.whole_only.is_some() {
        return None;
    }
    let mut out = patch.header.clone();
    let mut delta: i64 = 0;
    let mut any = false;
    for (index, chosen) in picks {
        let hunk = patch.hunks.get(*index).filter(|hunk| hunk.whole)?;
        let lines = match chosen {
            None => hunk.lines.clone(),
            Some(chosen) => match cut(hunk, chosen, reverse) {
                Some(lines) => lines,
                None => continue,
            },
        };
        let (old_start, was_old, new_start, was_new) = counts(&hunk.header)?;
        let (old_len, new_len) = sides(&lines);
        // A whole hunk after no cut one is git's own bytes, as staging a hunk always was.
        if chosen.is_none() && delta == 0 {
            out.extend_from_slice(&hunk.header);
        } else {
            // The side git looks on keeps its start; the other moves by what the cuts before
            // took out of it.
            let (old_start, new_start) = if reverse {
                (shifted(old_start, was_old, old_len, delta), new_start)
            } else {
                (old_start, shifted(new_start, was_new, new_len, delta))
            };
            out.extend_from_slice(
                format!("@@ -{old_start},{old_len} +{new_start},{new_len} @@").as_bytes(),
            );
            out.extend_from_slice(tail(&hunk.header));
        }
        let (moved, was) = if reverse {
            (old_len, was_old)
        } else {
            (new_len, was_new)
        };
        delta += i64::try_from(moved).ok()? - i64::try_from(was).ok()?;
        for line in &lines {
            out.extend_from_slice(line);
        }
        any = true;
    }
    any.then_some(out)
}

/// The start of a side git wrote at `start` with `was` lines, that now has `len`, moved by
/// `delta`: a side of no lines is written as the line before it.
fn shifted(start: u64, was: u64, len: u64, delta: i64) -> u64 {
    let first = if was == 0 { start + 1 } else { start };
    let first = first.saturating_add_signed(delta).max(1);
    if len == 0 { first - 1 } else { first }
}

/// What follows a hunk header's second `@@`: the function's line git names, and the line break.
fn tail(header: &[u8]) -> &[u8] {
    let from = header
        .windows(3)
        .skip(3)
        .position(|window| window == b" @@")
        .map_or(header.len(), |at| at + 3 + 3);
    &header[from.min(header.len())..]
}

#[cfg(test)]
mod tests;
