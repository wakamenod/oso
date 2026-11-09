//! Checkpoints in a long line (ADR 0099): cluster starts inside a chunk where a scan can
//! begin instead of at the chunk's start, so that finding the cluster or the column of an
//! offset reads at most a stride and a cluster of a long line, plus what the segmenter looks
//! back over.
//!
//! Must not depend on: cursors, windows or fonts; it sees only a chunk's text, where it
//! starts, its invalid bytes and the tab width.
//!
//! A checkpoint is taken in the pass that summarizes a chunk, at the first cluster start at
//! or past every [`STRIDE`] bytes since the line's start or the checkpoint before. It keeps
//! what a scan there needs: the line breaks before it in the chunk, the column function from
//! the start of its line (or of the chunk, on the chunk's first line), and the offset before
//! which characters are past a display cluster's limits. A scan from a checkpoint finds the
//! clusters a scan from the chunk's start finds after it, as each segments with the whole
//! chunk as context and starts in the same state: a display cluster depends only on its
//! chunk's text and where the chunk starts (ADR 0092). A line no longer than a stride gets
//! none.

use std::sync::Arc;
#[expect(
    clippy::disallowed_types,
    reason = "each item's own checkpoints, made once and shared by snapshots on any thread, not global state (ADR 0099)"
)]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use super::cell::{CellKind, Clusters, TabWidth};
use super::tab_map::ColumnFn;
use crate::decode::InvalidBytes;

/// How far apart checkpoints are, at least, in bytes of one line.
#[cfg(not(test))]
pub(super) const STRIDE: usize = 128;
/// Small in tests, whose chunks are small too, so that their lines get checkpoints.
#[cfg(test)]
pub(super) const STRIDE: usize = 8;

/// A cluster start inside a chunk where a scan can begin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Checkpoint {
    /// Where it is in the chunk.
    pub(super) at: u16,
    /// The line breaks before it in the chunk.
    pub(super) rows: u16,
    /// The column function from the start of its line, or of the chunk on the chunk's first
    /// line: cells before the first tab, and from its stop on, `u32::MAX` for no tab. A
    /// chunk's cells fit, as its bytes do.
    lead: u32,
    after_tab: u32,
    /// The offset in the chunk before which characters are past a display cluster's limits,
    /// as the scan from the chunk's start had it there; 0 for none.
    pub(super) past_limit_until: u16,
}

/// The checkpoints of `text`, a chunk that starts at `base`, or `None` if no line in it runs
/// past a stride. A line longer than a stride holds a whole block of half a stride with no
/// line break, which a fast count finds, so only around such a block is a line read.
pub(super) fn of_chunk(
    text: &str,
    base: usize,
    invalid: &InvalidBytes,
    tab_width: TabWidth,
) -> Option<Arc<[Checkpoint]>> {
    let bytes = text.as_bytes();
    let half = STRIDE / 2;
    let mut checkpoints = Vec::new();
    let mut measured_until = 0;
    for (index, block) in bytes.chunks_exact(half).enumerate() {
        let block_start = index * half;
        if block_start < measured_until || block.contains(&b'\n') {
            continue;
        }
        let line_start = bytes[..block_start]
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |newline| newline + 1);
        let line_end = bytes[block_start..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(text.len(), |newline| block_start + newline + 1);
        measured_until = line_end;
        if line_end - line_start > STRIDE {
            let rows = bytes[..line_start]
                .iter()
                .filter(|&&byte| byte == b'\n')
                .count();
            let line = (line_start, line_end - line_start, row(rows));
            of_line(text, base, invalid, tab_width, line, &mut checkpoints);
        }
    }
    (!checkpoints.is_empty()).then(|| checkpoints.into())
}

/// A count of line breaks in a chunk, which holds at most `MAX_CHUNK_BYTES`.
fn row(lines: usize) -> u16 {
    u16::try_from(lines).unwrap_or(u16::MAX)
}

/// Adds the checkpoints of the line of `len` bytes at `start` in `text`, after `rows` line
/// breaks, to `out`, and gives its column function up to its line break or its end.
fn of_line(
    text: &str,
    base: usize,
    invalid: &InvalidBytes,
    tab_width: TabWidth,
    (start, len, rows): (usize, usize, u16),
    out: &mut Vec<Checkpoint>,
) -> ColumnFn {
    let mut column = ColumnFn::default();
    let mut last = start;
    // After a line break, or at the chunk's start, no cluster past its limits is open, and
    // nothing before it decides a cluster after it, so the line is segmented on its own.
    let line = &text[start..start + len];
    let mut clusters = Clusters::with_invalid(line, base + start, invalid);
    while let Some((at, _, kind, cells)) = clusters.next() {
        let index = start + at;
        if kind == CellKind::LineBreak {
            break;
        }
        if index >= last + STRIDE {
            let past = match clusters.past_limit_until_at(at) {
                0 => 0,
                until => start + until,
            };
            let (lead, after_tab) = column.parts();
            out.push(Checkpoint {
                at: to_u16(index),
                rows,
                lead: u32::try_from(lead).unwrap_or(u32::MAX - 1),
                after_tab: after_tab.map_or(u32::MAX, |cells| {
                    u32::try_from(cells).unwrap_or(u32::MAX - 1)
                }),
                past_limit_until: to_u16(past),
            });
            last = index;
        }
        column = match kind {
            CellKind::Tab => column.then_tab(tab_width),
            _ => column.then_cells(cells),
        };
    }
    column
}

impl Checkpoint {
    /// The column function from the start of its line, or of the chunk on its first line.
    pub(super) fn column(&self) -> ColumnFn {
        ColumnFn::new(
            self.lead as usize,
            (self.after_tab != u32::MAX).then_some(self.after_tab as usize),
        )
    }
}

/// An offset in a chunk, which holds at most `MAX_CHUNK_BYTES`.
fn to_u16(offset: usize) -> u16 {
    u16::try_from(offset).unwrap_or(u16::MAX)
}

/// How many scans read a chunk before it makes its checkpoints: more than typing reads one
/// between two edits, far fewer than a search reads it.
#[cfg(not(test))]
const READS_BEFORE_CHECKPOINTS: u8 = 8;
/// In tests a chunk makes them at once, so that every scan there starts from them.
#[cfg(test)]
const READS_BEFORE_CHECKPOINTS: u8 = 1;

/// What a chunk keeps of its checkpoints: how many scans have read it while it had none,
/// and once they have read it [`READS_BEFORE_CHECKPOINTS`] times, the checkpoints, made then
/// and kept for good. An edit makes a new chunk, so typing, which reads one a few times
/// between edits, makes none, and a scan that reads one over and over makes them after its
/// first reads. Snapshots share them.
///
/// They hold no buffer offsets, only offsets in the chunk. Where the chunk starts is used
/// only to find its invalid bytes, as for its summary: a buffer that has any is read-only,
/// and loading only adds chunks after it, so a chunk that has them never moves. The count of
/// reads stops at [`READS_BEFORE_CHECKPOINTS`], as the checkpoints are made then.
#[derive(Debug, Default)]
#[expect(
    clippy::disallowed_types,
    reason = "each item's own checkpoints, made once and shared by snapshots on any thread, not global state (ADR 0099)"
)]
pub(super) struct Kept {
    made: OnceLock<Option<Arc<[Checkpoint]>>>,
    reads: AtomicU8,
}

impl Clone for Kept {
    fn clone(&self) -> Self {
        Self {
            made: self.made.clone(),
            reads: AtomicU8::new(self.reads.load(Ordering::Relaxed)),
        }
    }
}

impl Kept {
    /// The checkpoints of `text`, a chunk that starts at `base`, if scans have read it often
    /// enough; counts this read otherwise.
    pub(super) fn get(
        &self,
        text: &str,
        base: usize,
        invalid: &InvalidBytes,
        tab_width: TabWidth,
    ) -> Option<&[Checkpoint]> {
        if let Some(made) = self.made.get() {
            return made.as_deref();
        }
        let read = self.reads.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        if read < READS_BEFORE_CHECKPOINTS {
            return None;
        }
        self.made
            .get_or_init(|| of_chunk(text, base, invalid, tab_width))
            .as_deref()
    }
}

/// The clusters of `text`, a chunk at `base`, from `from`, and where their offsets count
/// from. At a checkpoint the text before it stays as context, as a display cluster can
/// start inside a grapheme cluster, and `past_limit_until` is what the scan from the chunk's
/// start had there. Otherwise `from` is the chunk's start, a line's or a place between two
/// ASCII characters, which nothing before moves, and the scan reads from there alone.
pub(super) fn clusters_from<'a>(
    text: &'a str,
    base: usize,
    from: usize,
    checkpoint: Option<usize>,
    invalid: &'a InvalidBytes,
) -> (Clusters<'a>, usize) {
    match checkpoint {
        Some(past_limit_until) => (
            Clusters::with_invalid_from(text, base, from, past_limit_until, invalid),
            0,
        ),
        None => (
            Clusters::with_invalid(&text[from..], base + from, invalid),
            from,
        ),
    }
}

/// The last of `checkpoints` on the row `rows` breaks into the chunk whose column is before
/// `target`, with that column, the row's part in the chunk starting at `column`. Before, not at:
/// a scan from the row's start stops at the first cluster that reaches `target`, which can be
/// a cluster of no cells just before a checkpoint at that column, and every cluster that
/// reaches it comes after a checkpoint before it.
pub(super) fn on_row(
    checkpoints: &[Checkpoint],
    (rows, column): (usize, usize),
    target: usize,
    tab_width: TabWidth,
) -> Option<(&Checkpoint, usize)> {
    let start = if rows == 0 { column } else { 0 };
    checkpoints
        .iter()
        .filter(|checkpoint| usize::from(checkpoint.rows) == rows)
        .map(|checkpoint| (checkpoint, checkpoint.column().apply(start, tab_width)))
        .take_while(|(_, at)| *at < target)
        .last()
}

/// The last checkpoint at or before `offset` in the chunk, if any.
pub(super) fn before_offset(checkpoints: &[Checkpoint], offset: usize) -> Option<&Checkpoint> {
    let after = checkpoints.partition_point(|checkpoint| usize::from(checkpoint.at) <= offset);
    after.checked_sub(1).map(|index| &checkpoints[index])
}

#[cfg(test)]
pub(super) mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::decode::REPLACEMENT;

    /// Text that reaches what checkpoints must keep: runs past a display cluster's limits,
    /// regional indicators alone and paired, Indic conjuncts, prepended marks, joiners, tag
    /// characters, controls, invalid bytes' U+FFFD, tabs and line breaks of both kinds.
    pub(in crate::display_map) fn pieces() -> impl Strategy<Value = String> {
        const PIECES: [&str; 26] = [
            "a",
            "a",
            "a",
            "b ",
            "\r\n",
            "\n",
            "\t",
            "\r",
            "日",
            "\u{1f1ef}",
            "\u{1f1f5}",
            "🇯🇵",
            "👍",
            "\u{1f3fd}",
            "\u{200d}",
            "\u{200c}",
            "\u{e0041}",
            "\u{fe0f}",
            "\u{301}",
            "\u{915}",
            "\u{94d}",
            "\u{937}",
            "\u{600}",
            "\u{fffd}",
            "\u{202e}",
            "\u{85}",
        ];
        // Repeats reach the limits; most pieces stay short.
        let repeats = prop_oneof![4 => Just(1usize), 1 => 2usize..24];
        prop::collection::vec((prop::sample::select(&PIECES[..]), repeats), 0..40).prop_map(
            |pieces| {
                pieces
                    .into_iter()
                    .map(|(piece, n)| piece.repeat(n))
                    .collect()
            },
        )
    }

    /// The invalid bytes of `text` if every `every`th U+FFFD in it stands for one.
    pub(in crate::display_map) fn invalid_in(text: &str, every: usize) -> InvalidBytes {
        let entries: Vec<_> = text
            .match_indices(REPLACEMENT)
            .step_by(every)
            .map(|(offset, _)| (offset, 0x80 + (offset % 100) as u8))
            .collect();
        let mut invalid = InvalidBytes::new();
        invalid.extend(0, &entries);
        invalid
    }

    /// Every cluster of `text` from the chunk's start, with the line breaks and the column
    /// function before it, as a scan from there finds them.
    fn scanned(
        text: &str,
        invalid: &InvalidBytes,
        tab_width: TabWidth,
    ) -> Vec<(usize, String, u16, ColumnFn)> {
        let (mut rows, mut column) = (0_u16, ColumnFn::default());
        let mut out = Vec::new();
        for (index, cluster, kind, cells) in Clusters::with_invalid(text, 0, invalid) {
            out.push((index, cluster.to_owned(), rows, column));
            match kind {
                CellKind::LineBreak => {
                    rows += 1;
                    column = ColumnFn::default();
                }
                CellKind::Tab => column = column.then_tab(tab_width),
                _ => column = column.then_cells(cells),
            }
        }
        out
    }

    #[test]
    fn a_long_line_has_one_every_stride_and_short_lines_none() {
        let (invalid, tab_width) = (InvalidBytes::default(), TabWidth::new(4));
        let long = "日本\tx\u{301}".repeat(8);
        let checkpoints = of_chunk(&long, 0, &invalid, tab_width).expect("a long line");
        // The first cluster start past each stride, so at most a cluster further.
        assert!(
            checkpoints.len() >= long.len() / (2 * STRIDE),
            "{}",
            checkpoints.len()
        );
        for pair in checkpoints.windows(2) {
            assert!(usize::from(pair[1].at) >= usize::from(pair[0].at) + STRIDE);
        }
        let short = "ab\n".repeat(20);
        assert!(of_chunk(&short, 0, &invalid, tab_width).is_none());
    }

    proptest! {
        /// A scan from any checkpoint finds the clusters a scan from the chunk's start finds
        /// after it, and the checkpoint holds the line breaks and the column there, with
        /// invalid bytes or none and any tab width.
        #[test]
        fn a_scan_from_a_checkpoint_is_the_scan_from_the_chunks_start(
            text in pieces(),
            every in 1usize..3,
            tab_width in 1u32..10,
        ) {
            let tab_width = TabWidth::new(tab_width);
            for invalid in [InvalidBytes::default(), invalid_in(&text, every)] {
                let whole = scanned(&text, &invalid, tab_width);
                let checkpoints = of_chunk(&text, 0, &invalid, tab_width);
                for checkpoint in checkpoints.as_deref().unwrap_or(&[]) {
                    let at = usize::from(checkpoint.at);
                    let tail: Vec<(usize, String)> = whole
                        .iter()
                        .filter(|(index, ..)| *index >= at)
                        .map(|(index, cluster, ..)| (*index, cluster.clone()))
                        .collect();
                    let past = usize::from(checkpoint.past_limit_until);
                    let resumed: Vec<(usize, String)> =
                        Clusters::with_invalid_from(&text, 0, at, past, &invalid)
                            .map(|(index, cluster, _, _)| (index, cluster.to_owned()))
                            .collect();
                    prop_assert_eq!(&resumed, &tail, "{:?} from {}", text, at);
                    let here = whole.iter().find(|(index, ..)| *index == at);
                    let (_, _, rows, column) = here.expect("a checkpoint is a cluster start");
                    prop_assert_eq!(checkpoint.rows, *rows);
                    prop_assert_eq!(checkpoint.column(), *column);
                }
            }
        }
    }
}
