//! Cutting a prompt's text to its cells: a label into rows, and a message or a name to a
//! width with `…` (ADR 0072, ADR 0075).
//!
//! Must not depend on: `editor`, `command` or `keymap`; it measures text in cells.

use std::ops::Range;

use super::CUT;
use crate::display_map::text_clusters;

/// The cells of `text` as one row.
pub fn cells_of(text: &str) -> usize {
    text_clusters(text, 0).map(|cluster| cluster.cells).sum()
}

/// `text` cut into rows of at most `width` cells, after the last space that fits, or at a
/// whole cluster where a word is longer than a row, which then starts where it is rather
/// than on a row of its own, and at most `rows` of them. Returns the
/// rows and whether some was left over, in which case the last row leaves a cell for `…`.
pub(super) fn rows_of(text: &str, width: usize, rows: usize) -> (Vec<Range<usize>>, bool) {
    let width = width.max(1);
    let mut out = Vec::new();
    let (mut start, mut used) = (0, 0);
    // Where the row could end after a space, and the cells it would hold then.
    let mut space = None;
    for cluster in text_clusters(text, 0) {
        // What a break after a space carries over can still leave no room for a wide cluster,
        // so this breaks again, before it, until the cluster fits or starts a row.
        while used + cluster.cells > width && used > 0 {
            let (end, kept) = space
                .filter(|&(end, _)| end > start)
                // A word too long for any row is broken here, not moved to the next.
                .filter(|&(_, kept)| used - kept + word_cells(&text[cluster.offset..]) <= width)
                .unwrap_or((cluster.offset, used));
            out.push(start..end);
            if out.len() == rows {
                return (cut_last(text, out, width), true);
            }
            (start, used, space) = (end, used - kept, None);
        }
        used += cluster.cells;
        if cluster.text == " " {
            space = Some((cluster.offset + 1, used));
        }
    }
    out.push(start..text.len());
    (out, false)
}

/// The cells of the word `text` starts with, up to its first space.
fn word_cells(text: &str) -> usize {
    text_clusters(text, 0)
        .take_while(|cluster| cluster.text != " ")
        .map(|cluster| cluster.cells)
        .sum()
}

/// Shortens the last of `rows` so that a `…` fits after it in `width` cells.
fn cut_last(text: &str, mut rows: Vec<Range<usize>>, width: usize) -> Vec<Range<usize>> {
    if let Some(last) = rows.last_mut() {
        let mut used = 0;
        let mut end = last.start;
        for cluster in text_clusters(&text[last.clone()], 0) {
            used += cluster.cells;
            if used >= width {
                break;
            }
            end = last.start + cluster.offset + cluster.text.len();
        }
        last.end = end;
    }
    rows
}

/// `message` cut at a whole cluster to fit, with the brackets around it, in `columns` cells,
/// and `…` if it was cut: what a prompt's message row shows.
pub fn fit_message(message: &str, columns: usize) -> (&str, &'static str) {
    cut_to(message, columns.saturating_sub(3))
}

/// `text` cut at a whole cluster to fit in `cells` cells with a `…` after it, and the `…`,
/// or `text` whole and nothing if it fits. A name an untrusted file gives is cut this way
/// before a question says it, so that it cannot push the question's own words out of view.
pub fn cut_to(text: &str, cells: usize) -> (&str, &'static str) {
    if cells_of(text) <= cells {
        return (text, "");
    }
    let mut used = 0;
    let mut end = 0;
    for cluster in text_clusters(text, 0) {
        used += cluster.cells;
        if used >= cells {
            break;
        }
        end = cluster.offset + cluster.text.len();
    }
    (&text[..end], CUT)
}
