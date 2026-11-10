//! Where a view row breaks (ADR 0100): after a run of spaces, between CJK clusters, with
//! basic kinsoku, one space hanging into the mark's column, and a cluster that does not fit
//! moved to the next row whole.
//!
//! Must not depend on: the tree of rows, windows or the renderer. It sees clusters of one
//! logical line, from a row's start, and says where each row ends.

use std::collections::VecDeque;

use crate::display_map::DisplayCluster;

/// How far kinsoku moves a break back, in clusters, before it gives up.
pub(super) const KINSOKU_REACH: usize = 2;

/// What the rules need of a cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Cluster {
    /// Where it starts in the buffer.
    pub(super) offset: usize,
    /// Its first cell on its logical line.
    pub(super) column: usize,
    /// How many cells it takes.
    pub(super) cells: usize,
    class: Class,
    /// Its first character, for kinsoku.
    first: char,
    /// Whether it is a tab, whose cells depend on its column.
    tab: bool,
}

/// The classes the rules tell apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// U+0020, the only space a row may break after or hang.
    Space,
    /// A no-break space, beside which a row never breaks.
    NoBreak,
    /// Han, kana, fullwidth forms and CJK punctuation, between which a row may break.
    Cjk,
    /// Anything else, which breaks only after a space or beside CJK.
    Other,
}

impl Cluster {
    /// The rules' view of `cluster`.
    pub(super) fn of(cluster: DisplayCluster<'_>) -> Self {
        let first = cluster.text.chars().next().unwrap_or(' ');
        let class = match first {
            ' ' if cluster.text.len() == 1 => Class::Space,
            '\u{a0}' | '\u{202f}' | '\u{2007}' => Class::NoBreak,
            c if is_cjk(c) => Class::Cjk,
            _ => Class::Other,
        };
        Self {
            offset: cluster.offset,
            column: cluster.column,
            cells: cluster.cells,
            class,
            first,
            tab: cluster.kind == crate::display_map::CellKind::Tab,
        }
    }
}

/// Whether `c` is CJK for the rules: Han, kana, CJK symbols and punctuation, and fullwidth
/// and halfwidth forms.
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

/// Whether a row may not start with `next`, after `before` (JIS X 4051's core sets, with
/// iteration marks, the wave dash, closing quotes and their halfwidth forms).
fn starts_badly(before: &Cluster, next: &Cluster) -> bool {
    const CJK: &str = concat!(
        "、。，．・：；？！ー）］｝〕〉》」』】ぁぃぅぇぉっゃゅょゎゕゖァィゥェォッャュョヮヵヶ",
        "々ゝゞヽヾ〜‼”’｡｣､･ｧｨｩｪｫｬｭｮｯｰ",
    );
    CJK.contains(next.first)
        || (before.class == Class::Cjk && ",.:;?!)]}".contains(next.first))
        || matches!(u32::from(next.first), 0x31f0..=0x31ff)
}

/// Whether a row may not end with `last`, before `next`.
fn ends_badly(last: &Cluster, next: &Cluster) -> bool {
    "（［｛〔〈《「『【“‘｢".contains(last.first)
        || (next.class == Class::Cjk && "([{".contains(last.first))
}

/// Whether a row may break between `last` and `next`, leaving kinsoku aside.
fn may_break(last: &Cluster, next: &Cluster) -> bool {
    match (last.class, next.class) {
        (Class::NoBreak, _) | (_, Class::NoBreak) | (_, Class::Space) => false,
        (Class::Space, _) | (Class::Cjk, _) | (_, Class::Cjk) => true,
        (Class::Other, Class::Other) => false,
    }
}

/// Whether a break between `last` and `next` keeps kinsoku.
fn keeps_kinsoku(last: &Cluster, next: &Cluster) -> bool {
    !starts_badly(last, next) && !ends_badly(last, next)
}

/// Where one view row ends, from what [`Rows`] has read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RowEnd {
    /// The offset of the first cluster of the next row, or the line's end.
    pub(super) end: usize,
    /// Whether the logical line goes on in another row.
    pub(super) continues: bool,
    /// The cell column on the logical line where the next row starts, 0 past the line.
    pub(super) next_column: usize,
    /// Whether the row holds a tab.
    pub(super) tabs: bool,
}

/// The rows of one logical line from a row's start, at `width` cells a row, the mark's
/// column aside. Fed one cluster at a time, it holds at most a row and a cluster of them.
pub(super) struct Rows<I> {
    clusters: I,
    width: usize,
    /// The clusters read and not yet in a row, from the current row's start.
    pending: VecDeque<Cluster>,
    /// Where the line ends, past its last cluster, once the clusters have run out.
    line_end: usize,
    done: bool,
}

impl<I: Iterator<Item = Cluster>> Rows<I> {
    /// The rows of the clusters `clusters`, which run to the end of a logical line ending
    /// at `line_end`, `width` cells each.
    pub(super) fn new(clusters: I, width: usize, line_end: usize) -> Self {
        Self {
            clusters,
            width: width.max(1),
            pending: VecDeque::new(),
            line_end,
            done: false,
        }
    }

    /// The start column of the current row, if a cluster of it has been read.
    fn row_column(&self) -> Option<usize> {
        self.pending.front().map(|cluster| cluster.column)
    }

    /// Takes the first `count` pending clusters as the row, and says where it ends.
    fn take(&mut self, count: usize) -> RowEnd {
        let tabs = self.pending.drain(..count).any(|cluster| cluster.tab);
        match self.pending.front() {
            Some(next) => RowEnd {
                end: next.offset,
                continues: true,
                next_column: next.column,
                tabs,
            },
            None => self.after_pending(tabs),
        }
    }

    /// Where the row ends when nothing of the line is left pending: at the next cluster,
    /// which is read for it, or at the line's end.
    fn after_pending(&mut self, tabs: bool) -> RowEnd {
        match self.clusters.next() {
            Some(next) => {
                let (end, next_column) = (next.offset, next.column);
                self.pending.push_back(next);
                RowEnd {
                    end,
                    continues: true,
                    next_column,
                    tabs,
                }
            }
            None => {
                self.done = true;
                RowEnd {
                    end: self.line_end,
                    continues: false,
                    next_column: 0,
                    tabs,
                }
            }
        }
    }

    /// The break for a row whose cluster at `overflow` does not fit: how many clusters it
    /// keeps.
    fn break_before(&self, overflow: usize) -> usize {
        let pending = &self.pending;
        let at = |index: usize| &pending[index];
        // One space may hang into the mark's column, if the row may break after it or more
        // spaces follow, which start the next row.
        let start = self.row_column().unwrap_or(0);
        let over = at(overflow);
        let breaks_after = pending.get(overflow + 1).is_none_or(|next| {
            next.class == Class::Space || (may_break(over, next) && keeps_kinsoku(over, next))
        });
        if overflow > 0
            && over.class == Class::Space
            && over.column - start == self.width
            && breaks_after
        {
            return overflow + 1;
        }
        if overflow == 0 {
            // Wider than a whole row: a row of its own, cut at the edge.
            return 1;
        }
        let greedy = (1..=overflow)
            .rev()
            .find(|&kept| may_break(at(kept - 1), at(kept)))
            .unwrap_or(overflow);
        if keeps_kinsoku(at(greedy - 1), at(greedy)) {
            return greedy;
        }
        (1..greedy)
            .rev()
            .take(KINSOKU_REACH)
            .find(|&kept| {
                may_break(at(kept - 1), at(kept)) && keeps_kinsoku(at(kept - 1), at(kept))
            })
            .unwrap_or(greedy)
    }
}

impl<I: Iterator<Item = Cluster>> Iterator for Rows<I> {
    type Item = RowEnd;

    fn next(&mut self) -> Option<RowEnd> {
        if self.done {
            return None;
        }
        if self.pending.is_empty() {
            match self.clusters.next() {
                Some(first) => self.pending.push_back(first),
                None => {
                    self.done = true;
                    return Some(RowEnd {
                        end: self.line_end,
                        continues: false,
                        next_column: 0,
                        tabs: false,
                    });
                }
            }
        }
        // Read on until a cluster does not fit, or the line ends.
        let start = self.row_column().unwrap_or(0);
        let mut index = 0;
        loop {
            if index == self.pending.len() {
                match self.clusters.next() {
                    Some(cluster) => self.pending.push_back(cluster),
                    None => {
                        self.done = true;
                        let tabs = self.pending.drain(..).any(|cluster| cluster.tab);
                        return Some(RowEnd {
                            end: self.line_end,
                            continues: false,
                            next_column: 0,
                            tabs,
                        });
                    }
                }
            }
            let cluster = self.pending[index];
            if cluster.column - start + cluster.cells > self.width {
                // Whether a space may hang depends on what follows it.
                if index + 1 == self.pending.len()
                    && let Some(next) = self.clusters.next()
                {
                    self.pending.push_back(next);
                }
                let kept = self.break_before(index);
                return Some(self.take(kept));
            }
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows of `text`, one logical line of narrow characters or CJK, at `width` cells,
    /// as the text of each.
    fn rows(text: &str, width: usize) -> Vec<String> {
        let mut column = 0;
        let clusters: Vec<Cluster> = text
            .char_indices()
            .map(|(offset, c)| {
                let cells = if is_cjk(c) { 2 } else { 1 };
                let mut buf = [0; 4];
                let cluster = DisplayCluster {
                    offset,
                    column,
                    cells,
                    text: c.encode_utf8(&mut buf),
                    kind: crate::display_map::CellKind::Text,
                    whitespace: false,
                };
                column += cells;
                Cluster::of(cluster)
            })
            .collect();
        let mut start = 0;
        Rows::new(clusters.into_iter(), width, text.len())
            .map(|row| {
                let piece = text[start..row.end].to_owned();
                start = row.end;
                piece
            })
            .collect()
    }

    #[test]
    fn text_with_spaces_breaks_after_them() {
        assert_eq!(rows("one two three", 8), ["one two ", "three"]);
        assert_eq!(rows("one  two", 5), ["one  ", "two"]);
    }

    #[test]
    fn one_space_hangs_into_the_marks_column_and_the_rest_start_the_next_row() {
        assert_eq!(rows("abcd efgh", 4), ["abcd ", "efgh"]);
        assert_eq!(rows("abcd   efgh", 4), ["abcd ", "  ", "efgh"]);
    }

    #[test]
    fn a_word_longer_than_a_row_breaks_where_it_reaches_the_edge() {
        assert_eq!(rows("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn no_break_spaces_hold_their_neighbours_together() {
        assert_eq!(rows("ab cd\u{a0}ef", 6), ["ab ", "cd\u{a0}ef"]);
    }

    #[test]
    fn cjk_breaks_between_any_two_and_beside_other_text() {
        assert_eq!(rows("日本語の文章", 6), ["日本語", "の文章"]);
        assert_eq!(rows("abc日本", 4), ["abc", "日本"]);
    }

    #[test]
    fn kinsoku_moves_a_break_back_so_no_row_starts_or_ends_badly() {
        // `。` would start the second row: the cluster before it goes with it.
        assert_eq!(rows("日本語。です", 6), ["日本", "語。で", "す"]);
        // `「` would end the first row.
        assert_eq!(rows("日本「語」", 6), ["日本", "「語」"]);
        // ASCII punctuation after CJK.
        assert_eq!(rows("日本語,abc", 6), ["日本", "語,abc"]);
    }

    #[test]
    fn kinsoku_gives_up_past_two_clusters() {
        assert_eq!(rows("あ。。。い", 4), ["あ。", "。。", "い"]);
    }

    #[test]
    fn a_wide_cluster_that_does_not_fit_goes_to_the_next_row_whole() {
        assert_eq!(rows("abc日", 4), ["abc", "日"]);
    }

    #[test]
    fn a_cluster_wider_than_a_row_has_a_row_of_its_own() {
        assert_eq!(rows("日a", 1), ["日", "a"]);
    }

    #[test]
    fn a_space_hangs_only_where_the_row_may_break_after_it() {
        // Before a no-break space, it does not, and the word breaks at the edge.
        assert_eq!(rows("abcdefg \u{a0}xy", 7), ["abcdefg", " \u{a0}xy"]);
        // Before `。`, it does not either, so no row starts with `。`.
        assert_eq!(rows("日本語で 。すね", 8), ["日本語", "で 。す", "ね"]);
    }

    #[test]
    fn an_ideographic_space_never_hangs() {
        assert_eq!(rows("日本語あ\u{3000}い", 8), ["日本語あ", "\u{3000}い"]);
    }

    #[test]
    fn the_other_no_break_spaces_hold_their_neighbours_together() {
        assert_eq!(rows("ab cd\u{202f}ef", 6), ["ab ", "cd\u{202f}ef"]);
        assert_eq!(rows("ab cd\u{2007}ef", 6), ["ab ", "cd\u{2007}ef"]);
    }

    #[test]
    fn an_opening_bracket_before_cjk_does_not_end_a_row() {
        assert_eq!(rows("日本(日本", 6), ["日本", "(日本"]);
    }

    #[test]
    fn an_empty_line_is_one_row() {
        assert_eq!(rows("", 8), [""]);
    }
}
