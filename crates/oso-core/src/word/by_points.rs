//! The word motions and checks as they read display points before ADR 0101, kept as the
//! oracle that the ones reading offsets answer as, wherever no budget stops them.
//!
//! Must not depend on: anything but the display snapshot and the word rules above.

use super::{Kind, splits, starts_word};
use crate::display_map::{DisplayPoint, DisplaySnapshot};
use crate::motion::{self, SEEK_COST, StopWalk, WalkBudget};
use crate::sum_tree::Bias;

/// What the last cluster before `point` on its row is, passing marks: what a mark at
/// `point` belongs to. Not a word if it reaches the start of the row, or if the stop has
/// read all it may.
fn kind_before_marks(
    display: &DisplaySnapshot,
    mut point: DisplayPoint,
    walk: &mut StopWalk<'_>,
) -> Kind {
    let mut end = display.display_point_to_offset(point, Bias::Left);
    while point.column > 0 {
        let mut read = SEEK_COST;
        let mut last = None;
        let mut first = None;
        for cluster in motion::clusters_before(display, point, end) {
            first.get_or_insert((cluster.offset, cluster.column));
            read += cluster.text.len();
            let kind = Kind::of(cluster.kind, cluster.text);
            if !kind.is_mark() {
                last = Some(kind);
            }
        }
        if !walk.spend(read) {
            return Kind::NOT_WORD;
        }
        if let Some(last) = last {
            return last;
        }
        let Some((offset, column)) = first else {
            break;
        };
        end = offset;
        point = DisplayPoint::new(point.row, column);
    }
    Kind::NOT_WORD
}

/// Whether a word is at `offset`: the cluster there is a word's, as [`forward_word`]
/// reads it.
pub(super) fn word_at(display: &DisplaySnapshot, offset: usize) -> bool {
    let point = display.offset_to_display_point(offset);
    display
        .clusters(point)
        .next()
        .is_some_and(|cluster| Kind::of(cluster.kind, cluster.text).is_word())
}

/// Whether a word starts at `offset`, as [`backward_word`] finds starts: the cluster there
/// is a word's and no mark, and what comes before it, passing marks, is no word or splits
/// from it. Reads only the clusters around `offset`, within `budget`, and says yes when it
/// cannot tell, for a caller that checks with [`backward_word`] after (ADR 0091).
pub(super) fn may_start_word(
    display: &DisplaySnapshot,
    offset: usize,
    budget: &mut WalkBudget,
) -> bool {
    let mut walk = budget.stop();
    if !walk.spend(SEEK_COST) {
        return true;
    }
    let point = display.offset_to_display_point(offset);
    let mut clusters = display.clusters(point);
    let Some(kind) = clusters
        .next()
        .map(|cluster| Kind::of(cluster.kind, cluster.text))
    else {
        return false;
    };
    if kind.is_mark() {
        return false;
    }
    let after = clusters.next().map_or(Kind::NOT_WORD, |cluster| {
        Kind::of(cluster.kind, cluster.text)
    });
    let before = if point.column == 0 {
        Kind::NOT_WORD
    } else {
        kind_before_marks(display, point, &mut walk)
    };
    starts_word(before, kind, after)
}

/// Whether a word ends at `offset`, as [`forward_word`] finds ends: the last cluster before
/// it, passing marks, is a word's, and the cluster at `offset` is no mark and no word, or
/// splits from it, or the row ends there. Reads only the clusters around `offset`, within
/// `budget`: marks past it before `offset` count as no word (ADR 0091).
pub(super) fn may_end_word(
    display: &DisplaySnapshot,
    offset: usize,
    budget: &mut WalkBudget,
) -> bool {
    let mut walk = budget.stop();
    if !walk.spend(SEEK_COST) {
        return true;
    }
    let point = display.offset_to_display_point(offset);
    if point.column == 0 {
        return false;
    }
    let mut clusters = display.clusters(point);
    let next = clusters
        .next()
        .map(|cluster| Kind::of(cluster.kind, cluster.text));
    if next.is_some_and(Kind::is_mark) {
        return false;
    }
    let after = clusters.next().map_or(Kind::NOT_WORD, |cluster| {
        Kind::of(cluster.kind, cluster.text)
    });
    let before = kind_before_marks(display, point, &mut walk);
    before.is_word() && next.is_none_or(|next| !next.is_word() || splits(before, next, after))
}

/// Where the first word that ends after `offset` ends: past what is not a word, and
/// on to the word's end. The end of the buffer if there is none. It stops short if
/// the stop or the command has read all it may.
pub(super) fn forward_word(
    display: &DisplaySnapshot,
    offset: usize,
    budget: &mut WalkBudget,
) -> usize {
    let mut walk = budget.stop();
    if !walk.spend(SEEK_COST) {
        return offset;
    }
    let mut point = display.offset_to_display_point(offset);
    let mut end = offset;
    // The word's last cluster, passing marks. A cursor on a mark is in what it follows.
    let starts_on_mark = display
        .clusters(point)
        .next()
        .is_some_and(|cluster| Kind::of(cluster.kind, cluster.text).is_mark());
    let mut word = if starts_on_mark {
        Some(kind_before_marks(display, point, &mut walk)).filter(|kind| kind.is_word())
    } else {
        None
    };
    loop {
        let mut clusters = display.clusters(point).peekable();
        // The kind of the next cluster, found by looking ahead.
        let mut peeked = None;
        while let Some(cluster) = clusters.next() {
            if !walk.spend(cluster.text.len()) {
                return end;
            }
            let kind = peeked
                .take()
                .unwrap_or_else(|| Kind::of(cluster.kind, cluster.text));
            let after = clusters.peek().map_or(Kind::NOT_WORD, |cluster| {
                Kind::of(cluster.kind, cluster.text)
            });
            peeked = Some(after);
            if !kind.is_mark() {
                match word {
                    Some(before) if !kind.is_word() || splits(before, kind, after) => {
                        return cluster.offset;
                    }
                    _ => word = kind.is_word().then_some(kind),
                }
            }
            end = cluster.offset + cluster.text.len();
        }
        if word.is_some() || point.row + 1 >= display.row_count() || !walk.spend(SEEK_COST) {
            return end;
        }
        point = DisplayPoint::new(point.row + 1, 0);
        end = motion::line_start(display, point.row);
    }
}

/// The first cluster of a window that has been read, for the window before it.
#[derive(Clone, Copy)]
struct Read {
    offset: usize,
    column: usize,
    len: usize,
    kind: Kind,
}

/// Where the last word that starts before `offset` starts: past what is not a word,
/// and back to the word's start. The start of the buffer if there is none. It stops
/// short if the stop or the command has read all it may.
#[expect(
    clippy::too_many_lines,
    reason = "the display-point oracle, kept as it was before ADR 0101"
)]
pub(super) fn backward_word(
    display: &DisplaySnapshot,
    offset: usize,
    budget: &mut WalkBudget,
) -> usize {
    let mut walk = budget.stop();
    if !walk.spend(SEEK_COST) {
        return offset;
    }
    let mut point = display.offset_to_display_point(offset);
    let mut earliest = display.display_point_to_offset(point, Bias::Left);
    // What follows the window: its first cluster's next neighbor decides the last
    // cluster's start, and the first cluster of the window after waits for what
    // comes before it.
    let mut after = display
        .clusters(point)
        .next()
        .map_or(Kind::NOT_WORD, |cluster| {
            Kind::of(cluster.kind, cluster.text)
        });
    let mut waiting: Option<Read> = None;
    loop {
        let from_row_start = point.column <= motion::BACK_WINDOW;
        // What comes before the cluster under decision, passing marks.
        let mut before = from_row_start.then_some(Kind::NOT_WORD);
        let mut found = None;
        // The first cluster of the window, for where to go on from.
        let mut first: Option<Read> = None;
        // The first cluster with no known kind before it, which the window before
        // this one decides, and what follows it.
        let mut undecided: Option<(Read, Kind)> = None;
        let mut current: Option<Read> = None;
        let mut read = 0;
        let mut decide = |cluster: Read, next: Kind, before: &mut Option<Kind>| {
            match *before {
                Some(kind) if starts_word(kind, cluster.kind, next) => found = Some(cluster.offset),
                Some(_) => {}
                None => undecided = undecided.or(Some((cluster, next))),
            }
            *before = Some(cluster.kind);
        };
        let window = motion::clusters_before(display, point, earliest).map(|cluster| Read {
            offset: cluster.offset,
            column: cluster.column,
            len: cluster.text.len(),
            kind: Kind::of(cluster.kind, cluster.text),
        });
        for next in window.chain(waiting) {
            read += next.len;
            first.get_or_insert(next);
            if let Some(cluster) = current.take() {
                decide(cluster, next.kind, &mut before);
            }
            if !next.kind.is_mark() {
                current = Some(next);
            }
        }
        if let Some(cluster) = current {
            decide(cluster, after, &mut before);
        }
        let spent = walk.spend(read + SEEK_COST);
        if let Some(found) = found {
            return found;
        }
        earliest = first.map_or(earliest, |first| first.offset);
        if !spent {
            return earliest;
        }
        if from_row_start {
            if point.row == 0 {
                return 0;
            }
            let end = motion::line_end(display, point.row - 1);
            earliest = end;
            point = display.offset_to_display_point(end);
            after = Kind::NOT_WORD;
            waiting = None;
        } else {
            // Past the row's start is handled above, so a window always holds a
            // cluster here.
            let Some(first) = first else {
                return earliest;
            };
            // Nothing undecided means only marks, which decide nothing, or that the
            // kind after the window is the first cluster's.
            match undecided {
                Some((cluster, next)) => {
                    waiting = Some(cluster);
                    after = next;
                }
                None => {
                    waiting = None;
                    after = first.kind;
                }
            }
            point = DisplayPoint::new(point.row, first.column);
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use crate::decode::REPLACEMENT;
    use crate::display_map::{DisplayMap, DisplaySnapshot, TabWidth, pieces};
    use crate::motion::WalkBudget;
    use crate::rope::Rope;

    /// The display of `text`, every `every`th U+FFFD in it an invalid byte, built as a load
    /// appends it.
    fn display(text: &str, every: usize) -> DisplaySnapshot {
        let entries: Vec<_> = text
            .match_indices(REPLACEMENT)
            .step_by(every)
            .map(|(offset, _)| (offset, 0x80 + (offset % 100) as u8))
            .collect();
        let mut rope = Rope::new();
        let mut map = DisplayMap::new(&rope, TabWidth::new(4));
        map.extend_invalid(0, &entries);
        let edit = rope.replace(0..0, text);
        map.sync(&rope, &edit);
        map.snapshot()
    }

    /// Whether each word check and motion by offset answers at every offset of `text` as the
    /// one by display points, with nothing to stop them.
    fn answers_alike(text: &str, every: usize) -> Result<(), TestCaseError> {
        let display = display(text, every);
        for offset in (0..=text.len()).filter(|&offset| text.is_char_boundary(offset)) {
            let budget = WalkBudget::new;
            prop_assert_eq!(
                super::super::word_at(&display, offset),
                super::word_at(&display, offset),
                "word_at {} in {:?}",
                offset,
                text
            );
            prop_assert_eq!(
                super::super::may_start_word(&display, offset, &mut budget()),
                super::may_start_word(&display, offset, &mut budget()),
                "may_start_word {} in {:?}",
                offset,
                text
            );
            prop_assert_eq!(
                super::super::may_end_word(&display, offset, &mut budget()),
                super::may_end_word(&display, offset, &mut budget()),
                "may_end_word {} in {:?}",
                offset,
                text
            );
            prop_assert_eq!(
                super::super::forward_word(&display, offset, &mut budget()),
                super::forward_word(&display, offset, &mut budget()),
                "forward_word {} in {:?}",
                offset,
                text
            );
            prop_assert_eq!(
                super::super::backward_word(&display, offset, &mut budget()),
                super::backward_word(&display, offset, &mut budget()),
                "backward_word {} in {:?}",
                offset,
                text
            );
        }
        Ok(())
    }

    proptest! {
        /// Over text that reaches a display cluster's limits, with invalid bytes, regional
        /// indicators, conjuncts and line breaks of both kinds.
        #[test]
        fn reading_offsets_answers_as_reading_points(text in pieces(), every in 1usize..3) {
            answers_alike(&text, every)?;
        }

        /// Over words of several scripts, cases, marks and long vowel marks.
        #[test]
        fn reading_offsets_answers_as_reading_points_for_words(text in super::super::tests::word_text()) {
            answers_alike(&text, 1)?;
        }
    }
}
