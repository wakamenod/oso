//! Cells in `*Spaces*`: how many a text takes as oso draws it, and cutting and padding to a
//! width, so that no row of the list runs past its window.
//!
//! Must not depend on: anything but oso-core's cell classes; plain text in, text out.

use oso_core::display_map::cell::{Clusters, classify};

use super::row::REMOTE;
use super::{GUIDES, MARKS};

/// How many cells `text` takes as oso draws it, cluster by cluster, so that an emoji sequence
/// counts as the one glyph it is drawn as (ADR 0159). Clusters of two pieces joined in a row
/// take at most what the pieces take apart, so a line cut by it never runs past its width.
pub(super) fn cells(text: &str) -> usize {
    pieces(text).map(|(_, cells)| cells).sum()
}

/// `text` in the pieces it is measured and cut by, each with its cells: its characters, as
/// most text is, ASCII or wide, where each is a cluster of its own; else its clusters as oso
/// draws them, which segmenting costs several times more (ADR 0159).
fn pieces(text: &str) -> impl Iterator<Item = (&str, usize)> {
    let joins = text.chars().any(may_join);
    let by_char = (!joins).then(|| {
        text.char_indices()
            .map(move |(at, c)| (&text[at..at + c.len_utf8()], char_cells(c)))
    });
    let by_cluster =
        joins.then(|| Clusters::new(text).map(|(_, cluster, _, cells)| (cluster, cells)));
    by_char
        .into_iter()
        .flatten()
        .chain(by_cluster.into_iter().flatten())
}

/// Whether `c` may make one cluster with a character beside it, as oso draws text: a joiner,
/// a selector, a keycap, a regional indicator, a skin tone, a tag, a Hangul jamo, or a mark that
/// takes no cell of its own. ASCII never does, but in `\r\n`, which a row never holds.
fn may_join(c: char) -> bool {
    // Kana, ideographs and Hangul syllables, most of wide text, join nothing: told by their
    // ranges, as looking up a character's cells costs more. The kana's sound marks and the
    // tone marks among the symbols are left to the lookup.
    let plain_wide = matches!(
        u32::from(c),
        0x3000..=0x3029 | 0x3030..=0x3098 | 0x309b..=0x30ff | 0x4e00..=0x9fff | 0xac00..=0xd7a3
    );
    !c.is_ascii()
        && !plain_wide
        && (matches!(
            u32::from(c),
            0x1100..=0x11ff
                | 0x200c..=0x200d
                | 0x20e3
                | 0xa960..=0xa97f
                | 0xd7b0..=0xd7ff
                | 0xfe00..=0xfe0f
                | 0x1f1e6..=0x1f1ff
                | 0x1f3fb..=0x1f3ff
                | 0xe0000..=0xe0fff
        ) || oso_core::display_map::cell::is_invisible(c)
            || char_cells(c) == 0)
}

fn char_cells(c: char) -> usize {
    let mut buffer = [0; 4];
    classify(c.encode_utf8(&mut buffer)).1
}

/// `text` without what it begins with that could pass for a row's start: blanks, the guides
/// that hang a session's row (ADR 0140), marks and characters that take no cell. Every rule that
/// reads a row from the list's text skips blanks and guides, then reads a mark.
pub fn unmarked(text: &str) -> &str {
    text.trim_start_matches(|c: char| {
        c.is_whitespace()
            || GUIDES.contains(c)
            || MARKS.contains(c)
            || REMOTE.contains(c)
            || char_cells(c) == 0
    })
}

/// `text` cut to `width` cells between clusters, ending in `…` if cut; a line break ends it.
pub fn cut(text: &str, width: usize) -> String {
    let text = text.lines().next().unwrap_or_default();
    if cells(text) <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for (cluster, n) in pieces(text) {
        if used + n + 1 > width {
            break;
        }
        out.push_str(cluster);
        used += n;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

/// `text` cut to `width` cells in the middle, keeping its last third (ADR 0131), so that names
/// that share a long start still differ: `chore-survey-r…lifecycle-hook`. A line break ends it.
/// It is cut between clusters.
pub fn cut_middle(text: &str, width: usize) -> String {
    let text = text.lines().next().unwrap_or_default();
    if cells(text) <= width || width < 6 {
        return cut(text, width);
    }
    let tail_room = (width - 1) / 3;
    let mut tail = Vec::new();
    let mut used = 0;
    // Only a cut row's tail is gathered: most rows are not cut.
    let all: Vec<(&str, usize)> = pieces(text).collect();
    for &(cluster, n) in all.iter().rev() {
        if used + n > tail_room {
            break;
        }
        tail.push(cluster);
        used += n;
    }
    let head_room = width - 1 - used;
    let mut out = String::new();
    let mut used = 0;
    for (cluster, n) in pieces(text) {
        if used + n > head_room {
            break;
        }
        out.push_str(cluster);
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
