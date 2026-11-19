//! Cells in `*Spaces*`: how many a text takes as oso draws it, and cutting and padding to a
//! width, so that no row of the list runs past its window.
//!
//! Must not depend on: anything but oso-core's cell classes; plain text in, text out.

use oso_core::display_map::cell::{Clusters, classify};

use super::{GUIDES, MARKS};

/// How many cells `text` takes as oso draws it, cluster by cluster, so that an emoji sequence
/// counts as the one glyph it is drawn as (ADR 0159). Clusters of two pieces joined in a row
/// take at most what the pieces take apart, so a line cut by it never runs past its width.
pub(super) fn cells(text: &str) -> usize {
    Clusters::new(text).map(|(.., cells)| cells).sum()
}

/// `text`'s clusters as oso draws them, each with its cells.
fn clusters(text: &str) -> impl Iterator<Item = (&str, usize)> {
    Clusters::new(text).map(|(_, cluster, _, cells)| (cluster, cells))
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
        c.is_whitespace() || GUIDES.contains(c) || MARKS.contains(c) || char_cells(c) == 0
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
    for (cluster, n) in clusters(text) {
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
    let all: Vec<(&str, usize)> = clusters(text).collect();
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
    for (cluster, n) in clusters(text) {
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
