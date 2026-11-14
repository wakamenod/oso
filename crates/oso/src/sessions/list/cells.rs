//! Cells in `*Sessions*`: how many a text takes as oso draws it, and cutting and padding to a
//! width, so that no row of the list runs past its window.
//!
//! Must not depend on: anything but oso-core's cell classes; plain text in, text out.

use oso_core::display_map::cell::classify;

use super::MARKS;

/// How many cells `text` takes, each character as oso draws it alone: a cluster of several
/// characters takes at most this, so that a line cut by it never runs past its width.
pub(super) fn cells(text: &str) -> usize {
    text.chars().map(char_cells).sum()
}

fn char_cells(c: char) -> usize {
    let mut buffer = [0; 4];
    classify(c.encode_utf8(&mut buffer)).1
}

/// `text` without what it begins with that could pass for a row's start: blanks, marks and
/// characters that take no cell.
pub fn unmarked(text: &str) -> &str {
    text.trim_start_matches(|c: char| c.is_whitespace() || MARKS.contains(c) || char_cells(c) == 0)
}

/// `text` cut to `width` cells, ending in `…` if cut; a line break ends it.
pub fn cut(text: &str, width: usize) -> String {
    let text = text.lines().next().unwrap_or_default();
    if cells(text) <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let n = char_cells(c);
        if used + n + 1 > width {
            break;
        }
        out.push(c);
        used += n;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

/// `text` cut to `width` cells in the middle, keeping its last third (ADR 0131), so that names
/// that share a long start still differ: `chore-survey-r…lifecycle-hook`. A line break ends it.
pub fn cut_middle(text: &str, width: usize) -> String {
    let text = text.lines().next().unwrap_or_default();
    if cells(text) <= width || width < 6 {
        return cut(text, width);
    }
    let tail_room = (width - 1) / 3;
    let mut tail = Vec::new();
    let mut used = 0;
    for c in text.chars().rev() {
        let n = char_cells(c);
        if used + n > tail_room {
            break;
        }
        tail.push(c);
        used += n;
    }
    let head_room = width - 1 - used;
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let n = char_cells(c);
        if used + n > head_room {
            break;
        }
        out.push(c);
        used += n;
    }
    out.push('…');
    out.extend(tail.into_iter().rev());
    out
}

/// `text` with spaces after it to `width` cells, if it is shorter.
pub(super) fn pad(text: &str, width: usize) -> String {
    let mut out = text.to_owned();
    out.extend(std::iter::repeat_n(' ', width.saturating_sub(cells(text))));
    out
}
