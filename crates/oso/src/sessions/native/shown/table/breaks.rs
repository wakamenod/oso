//! Where a cell's text breaks into rows, and how wide a table's columns are (ADR 0165, "The look:
//! tables").
//!
//! Must not depend on: the syntax threads, the editor or the theme.

use std::ops::Range;

use oso_core::display_map::text_clusters;

/// The narrowest a column is made, in cells, and how much of its longest word it keeps before
/// wider columns give way.
const MIN_CELLS: usize = 4;
const MAX_WORD: usize = 16;

/// A row of a cell: its place in the grid's text, its cells, and whether the next goes on inside a
/// word.
#[derive(Clone, Debug)]
pub(super) struct Wrapped {
    pub(super) text: Range<usize>,
    pub(super) cells: usize,
    pub(super) broken: bool,
}

/// What a break needs of a cluster. The rules are those of a wrapped line's rows (ADR 0100),
/// `breaks` of oso-core's display map, which no other crate sees: one changed there changes here.
#[derive(Clone, Copy)]
pub(super) struct Class {
    first: char,
    pub(super) space: bool,
    /// No line breaks beside it.
    joins: bool,
    cjk: bool,
}

impl Class {
    pub(super) fn of(cluster: &str) -> Self {
        let first = cluster.chars().next().unwrap_or(' ');
        Self {
            first,
            space: cluster == " ",
            joins: matches!(first, '\u{a0}' | '\u{202f}' | '\u{2007}'),
            cjk: is_cjk(first),
        }
    }
}

/// Whether `c` is Han, kana, CJK punctuation, or a fullwidth or halfwidth form.
fn is_cjk(c: char) -> bool {
    matches!(
        u32::from(c),
        0x2e80..=0x2fdf
            | 0x3000..=0x30ff
            | 0x3190..=0x31ff
            | 0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xf900..=0xfaff
            | 0xfe30..=0xfe4f
            | 0xff00..=0xffef
            | 0x20000..=0x3ffff
    )
}

/// What no line starts with (JIS X 4051's core sets), and what none ends with.
const CLOSES: &str = concat!(
    "、。，．・：；？！ー）］｝〕〉》」』】ぁぃぅぇぉっゃゅょゎゕゖァィゥェォッャュョヮヵヶ",
    "々ゝゞヽヾ〜‼”’｡｣､･ｧｨｩｪｫｬｭｮｯｰ",
);
const OPENS: &str = "（［｛〔〈《「『【“‘｢";

/// Whether a line may break between two clusters: after a space, and beside CJK, kinsoku kept.
pub(super) fn breaks(last: Class, next: Class) -> bool {
    if next.space || last.joins || next.joins {
        return false;
    }
    if !(last.space || last.cjk || next.cjk) {
        return false;
    }
    let closes = (!next.first.is_ascii() && CLOSES.contains(next.first))
        || (last.cjk && ",.:;?!)]}".contains(next.first))
        || matches!(u32::from(next.first), 0x31f0..=0x31ff);
    let opens = (!last.first.is_ascii() && OPENS.contains(last.first))
        || (next.cjk && "([{".contains(last.first));
    !closes && !opens
}

/// Wraps `range` of `text` into rows of at most `width` cells, each put in `rows`, at most
/// `most`: at the last place one may break, or in a word that is longer than a row, which is
/// marked as `broken`, as is any break but a space's where an address is. A row's spaces at its
/// end go.
pub(super) fn rows_of(
    text: &str,
    range: Range<usize>,
    width: usize,
    rows: &mut Vec<Wrapped>,
    most: usize,
) -> Option<()> {
    let first = rows.len();
    let address = text[range.clone()].contains("://");
    let base = range.start;
    let mut start = base;
    // The cells since `start`, and past the last cluster that is no space; where that one ends.
    let (mut used, mut inked, mut ink_end) = (0, 0, base);
    // The last place to break: the next row's start, this row's end and cells, and the cells
    // up to the next row's start.
    let mut place: Option<(usize, usize, usize, usize)> = None;
    let mut last: Option<Class> = None;
    for cluster in text_clusters(&text[range.clone()], 0) {
        let at = base + cluster.offset;
        let next = Class::of(cluster.text);
        if cluster.cells > width {
            return None;
        }
        if last.is_some_and(|last| breaks(last, next)) && at > start {
            place = Some((at, ink_end, inked, used));
        }
        while !next.space && used + cluster.cells > width && at > start {
            let in_word = place.is_none();
            let (from, end, cells, skipped) = place.take().unwrap_or((at, ink_end, inked, used));
            rows.push(Wrapped {
                text: start..end,
                cells,
                // Where it ends before the next row's start, spaces go.
                broken: end == from && (in_word || address),
            });
            if rows.len() - first > most {
                return None;
            }
            start = from;
            used -= skipped;
            inked = inked.saturating_sub(skipped);
        }
        used += cluster.cells;
        if !next.space {
            inked = used;
            ink_end = at + cluster.text.len();
        }
        last = Some(next);
    }
    if ink_end > start {
        rows.push(Wrapped {
            text: start..ink_end,
            cells: inked,
            broken: false,
        });
    }
    Some(())
}

/// Each column's width, in at most `room` cells in all: what its widest cell takes while they
/// fit, else as wide as they can all be, each at least its longest word within bounds, then at
/// least [`MIN_CELLS`]; `None` when they cannot.
pub(super) fn widths(widest: &[(usize, usize)], room: usize) -> Option<Vec<usize>> {
    let natural = |(cells, _): &(usize, usize)| (*cells).max(1);
    if widest.iter().map(natural).sum::<usize>() <= room {
        return Some(widest.iter().map(natural).collect());
    }
    let by_word = |column: &(usize, usize)| column.1.clamp(MIN_CELLS, MAX_WORD);
    let floors: Vec<usize> = (widest.iter())
        .map(|column| natural(column).min(by_word(column)))
        .collect();
    let floors = if floors.iter().sum::<usize>() <= room {
        floors
    } else {
        let least = widest.iter().map(|column| natural(column).min(MIN_CELLS));
        Some(least.collect::<Vec<_>>()).filter(|least| least.iter().sum::<usize>() <= room)?
    };
    let at = |level: usize| {
        (widest.iter().zip(&floors))
            .map(move |(column, floor)| level.clamp(*floor, natural(column)))
    };
    let mut level = 0;
    // It ends: at its widest, it would have fitted.
    while at(level + 1).sum::<usize>() <= room {
        level += 1;
    }
    let mut widths: Vec<usize> = at(level).collect();
    let mut left = room.saturating_sub(widths.iter().sum());
    for (width, column) in widths.iter_mut().zip(widest) {
        if left > 0 && *width == level && *width < natural(column) {
            *width += 1;
            left -= 1;
        }
    }
    Some(widths)
}
