//! Applying an undo group's batches a slice at a time, as an undo or a redo crosses it
//! (ADR 0119). Batches that lie apart apply as one run, as one batch would. A slice may stop
//! between batches, between the splices of one, or inside one splice's text, which then goes
//! back in pieces; however it is cut, the text, the batches that undo the group and the
//! offsets that follow it come out as at once.
//!
//! Must not depend on: `window`, `editor` or `command`; this sees only text.

use std::collections::BinaryHeap;
use std::mem;
use std::ops::Range;
use std::time::Instant;

use super::splice::{SpliceText, settle_wraps, sync, union};
use crate::display_map::DisplayMap;
use crate::display_map::wrap_map::Splice as WrapSplice;
use crate::rope::Rope;
use crate::selection::{OffsetEdit, follow_ranges};
use crate::sum_tree::Bias;
use crate::undo::{Batch, Counts, Splice};

/// What putting a splice back costs a slice besides its bytes: about what copying that many
/// bytes back costs.
pub(crate) const SPLICE_WORK: usize = 1024;

/// The most bytes of one splice's text put back at once. A longer text goes back in pieces
/// of this, at increasing offsets.
pub(crate) const PIECE: usize = 1024 * 1024;

/// What following an offset or a range through a part of a run costs, as [`Budget`] counts it.
pub(crate) const FOLLOW_WORK: usize = 2;

/// What a slice may still do.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    /// Work left, counted as [`SPLICE_WORK`] a splice and the bytes put back, and what
    /// following what applied costs.
    pub(crate) work: usize,
    /// When the slice must stop applying, if it has a time.
    pub(crate) until: Option<Instant>,
    /// The most bytes of a splice's text put back at once.
    pub(crate) piece: usize,
    /// What following each part of a run handed out costs besides its splices: every window's
    /// cursors and the highlights' batch.
    pub(crate) part: usize,
    /// What following each run that ends costs: the edited lines.
    pub(crate) run: usize,
}

impl Budget {
    /// A slice of `work` until `until`, putting a splice's text back `piece` bytes at a time,
    /// whose following costs nothing more.
    pub(crate) fn new(work: usize, until: Option<Instant>, piece: usize) -> Self {
        Self {
            work,
            until,
            piece,
            part: 0,
            run: 0,
        }
    }

    /// Whether the slice is to stop.
    pub(crate) fn spent(&self) -> bool {
        self.work == 0 || self.until.is_some_and(|until| Instant::now() >= until)
    }

    pub(crate) fn spend(&mut self, work: usize) {
        self.work = self.work.saturating_sub(work);
    }
}

/// How much work a group with these batches is, as [`Budget`] counts it.
pub(crate) fn work_of(counts: &[Counts]) -> usize {
    counts
        .iter()
        .map(|counts| counts.splices * SPLICE_WORK + counts.put)
        .sum()
}

/// An undo group's batches being applied, last first, a slice at a time.
#[derive(Default)]
pub(crate) struct GroupApply {
    /// How many batches, from the last, have gone into runs.
    taken: usize,
    /// The run being applied.
    run: Option<Run>,
    /// The batches that undo the runs applied, one a run, and their counts.
    undo: Vec<Batch>,
    counts: Vec<Counts>,
    /// Where display clusters may have changed, in the offsets now, ascending and apart
    /// (ADR 0091); the run being applied adds its own when it ends.
    changed: Vec<Range<usize>>,
    /// The work done so far, as [`work_of`] counts it.
    done: usize,
    /// What cursors follow of the run, from its last splice back, not yet handed out: a part
    /// of a batch goes to them only where a byte lies between it and the rest, or two
    /// replacements meet, as they could follow an insertion that meets a splice otherwise than
    /// the batch moves them.
    chunk: Vec<OffsetEdit>,
    /// What cursors follow of each run that ended since [`GroupApply::take_runs`], whole.
    runs: Vec<Vec<OffsetEdit>>,
}

/// Batches that lie apart, joined into one (ADR 0119): a batch whose splices all lie before
/// the run, or all after what it became, with a byte between, joins it, since one batch then
/// does what they did one after another.
struct Run {
    /// The batches, by their index in the group, in the order their splices apply, each with
    /// what puts its starts in the offsets before the run.
    parts: Vec<(usize, isize)>,
    part: usize,
    /// The current part's splices not yet applied; a part applies from its last back.
    left: usize,
    /// What the splices not yet applied replace, and the text they put back.
    old_below: usize,
    new_below: usize,
    /// The splice going back in pieces.
    piece: Option<Piece>,
    /// The batch that undoes the run, and what cursors follow of it, from its last splice back.
    undo: Batch,
    edits: Vec<OffsetEdit>,
    counts: Counts,
    /// Where the chunks each splice rebuilt start, in the offsets before the run, until the
    /// splices below have applied, with where they end after it.
    pending: BinaryHeap<(usize, usize)>,
    /// What the run changed so far, in the offsets after it, descending and apart.
    changed: Vec<Range<usize>>,
    /// Where the last splice applied starts, in the offsets before the run, and what it
    /// replaced.
    last: Option<(usize, usize)>,
}

/// A splice whose text goes back in pieces.
struct Piece {
    /// What it replaced.
    removed: Option<Rope>,
    /// The bytes of its text back so far.
    put: usize,
    /// Where the chunks its edits rebuilt start, and the most they reach past its text.
    rebuilt: usize,
    after: usize,
}

impl GroupApply {
    /// Applies `group`, whose batches `counts` count, from where it stopped, until the budget
    /// is spent or all have applied, as [`super::splice::apply_marking`] does. Each part of a
    /// run applied goes in `steps`, for cursors to follow. Returns whether all have.
    ///
    /// Each call reads `group` anew, and must be given the same group, and the text and the
    /// display where the last left them.
    #[expect(
        clippy::too_many_arguments,
        reason = "the text, display and wraps are the staged crossing's, borrowed apart"
    )]
    pub(crate) fn go_on(
        &mut self,
        group: &[Batch],
        counts: &[Counts],
        text: &mut Rope,
        display: &mut DisplayMap,
        mut wraps: Option<&mut Vec<WrapSplice>>,
        budget: &mut Budget,
        steps: &mut Vec<Vec<OffsetEdit>>,
    ) -> bool {
        let mut moved = false;
        let ended = loop {
            let mut fresh = false;
            if self.run.is_none() {
                match self.plan(group, counts, budget) {
                    Some(run) => (self.run, fresh) = (Some(run), true),
                    None => break true,
                }
            }
            let Some(run) = &mut self.run else {
                break true;
            };
            // What is left must pay for following the part handed out, and a run begun for
            // following it when it ends.
            let owed = budget.part + if fresh { budget.run } else { 0 };
            if moved && (budget.spent() || budget.work <= owed) {
                if run.apart(group) {
                    self.end_chunk(steps, budget);
                }
                break false;
            }
            moved = true;
            let ended = run.step(
                group,
                text,
                display,
                wraps.as_deref_mut(),
                budget,
                &mut self.done,
                &mut self.chunk,
            );
            if ended {
                self.end_chunk(steps, budget);
                self.end_run(budget);
            }
        };
        settle_wraps(display, wraps);
        ended
    }

    /// The work done so far, as [`work_of`] counts it.
    pub(crate) fn done(&self) -> usize {
        self.done
    }

    /// Where display clusters may have changed, once all have applied.
    pub(crate) fn changed(&self) -> &[Range<usize>] {
        &self.changed
    }

    /// What cursors follow of each run that ended since the last call, whole: for what costs
    /// as much to follow once as a part of a run, as the edited lines do.
    pub(crate) fn take_runs(&mut self) -> Vec<Vec<OffsetEdit>> {
        mem::take(&mut self.runs)
    }

    /// The batches that undo what applied, one a run, with their counts.
    pub(crate) fn into_undo(self) -> (Vec<Batch>, Vec<Counts>) {
        (self.undo, self.counts)
    }

    /// The next run: the next batch with a splice, from the last, and those after it that
    /// lie apart from all before them. O(batches), each counted as a unit of work.
    fn plan(&mut self, group: &[Batch], counts: &[Counts], budget: &mut Budget) -> Option<Run> {
        let initial = loop {
            let index = group.len().checked_sub(self.taken + 1)?;
            self.taken += 1;
            budget.spend(1);
            if !group[index].is_empty() {
                break index;
            }
        };
        let (Some(first), Some(last)) = (group[initial].first(), group[initial].last()) else {
            return None;
        };
        let mut front = first.start;
        // Where the run's last splice ends, in the offsets before it, and what it adds to
        // the length, so that a batch after it can be put in those offsets.
        let mut back = last.start + last.len;
        let mut grown = counts[initial].delta();
        let mut total = counts[initial];
        let (mut before, mut after) = (Vec::new(), Vec::new());
        while let Some(index) = group.len().checked_sub(self.taken + 1) {
            let batch = &group[index];
            if let (Some(first), Some(last)) = (batch.first(), batch.last()) {
                let lies_before = last.start + last.len < front;
                let lies_after = back
                    .checked_add_signed(grown)
                    .is_some_and(|end| first.start > end);
                if lies_before {
                    before.push(index);
                    front = first.start;
                } else if lies_after {
                    after.push((index, -grown));
                    back = last.start.saturating_add_signed(-grown) + last.len;
                } else {
                    break;
                }
                let more = counts[index];
                grown += more.delta();
                total.removed += more.removed;
                total.put += more.put;
                total.splices += more.splices;
            }
            self.taken += 1;
            budget.spend(1);
        }
        // Those after apply first, the last joined first; then the first batch; then those
        // before, as they joined.
        let mut parts: Vec<(usize, isize)> = after.into_iter().rev().collect();
        parts.push((initial, 0));
        parts.extend(before.into_iter().map(|index| (index, 0)));
        Some(Run {
            left: group[parts[0].0].len(),
            parts,
            part: 0,
            old_below: total.removed,
            new_below: total.put,
            piece: None,
            undo: Vec::with_capacity(total.splices),
            edits: Vec::with_capacity(total.splices),
            counts: Counts::new(),
            pending: BinaryHeap::new(),
            changed: Vec::new(),
            last: None,
        })
    }

    /// Hands this slice's part of the run to `steps`, ascending, and moves what earlier runs
    /// changed along with it.
    fn end_chunk(&mut self, steps: &mut Vec<Vec<OffsetEdit>>, budget: &mut Budget) {
        if self.chunk.is_empty() {
            return;
        }
        budget.spend(budget.part + self.changed.len() * FOLLOW_WORK);
        let mut chunk = mem::take(&mut self.chunk);
        chunk.reverse();
        follow_ranges(&mut self.changed, &chunk);
        steps.push(chunk);
    }

    /// Puts the run that ended with the batches that undo, and what it changed with the
    /// rest.
    fn end_run(&mut self, budget: &mut Budget) {
        let Some(mut run) = self.run.take() else {
            return;
        };
        budget.spend(budget.run);
        // Nothing is below what is still pending, so it stays where it is.
        while let Some((at, end)) = run.pending.pop() {
            add_descending(&mut run.changed, at..end);
        }
        run.undo.reverse();
        run.edits.reverse();
        run.changed.reverse();
        self.changed = union(mem::take(&mut self.changed), run.changed);
        self.runs.push(run.edits);
        self.undo.push(run.undo);
        self.counts.push(run.counts);
    }
}

impl Run {
    /// Whether a byte lies between the last splice applied and the next, or both replace text,
    /// so that what cursors follow may be cut there.
    fn apart(&self, group: &[Batch]) -> bool {
        let Some(&(index, shift)) = self.parts.get(self.part) else {
            return true;
        };
        let next = group[index].get(self.left.wrapping_sub(1));
        match (next, self.last) {
            (Some(next), Some((last, replaced))) => {
                let end = next.start.saturating_add_signed(shift) + next.len;
                end < last || (end == last && next.len > 0 && replaced > 0)
            }
            _ => true,
        }
    }

    /// Applies the next splice, or the next piece of one. Returns whether the run ended.
    #[expect(
        clippy::too_many_arguments,
        reason = "the text, display and wraps are the staged crossing's, borrowed apart"
    )]
    fn step(
        &mut self,
        group: &[Batch],
        text: &mut Rope,
        display: &mut DisplayMap,
        wraps: Option<&mut Vec<WrapSplice>>,
        budget: &mut Budget,
        done: &mut usize,
        chunk: &mut Vec<OffsetEdit>,
    ) -> bool {
        let (index, shift) = self.parts[self.part];
        let Some(splice) = group[index].get(self.left.wrapping_sub(1)) else {
            return self.next_part(group);
        };
        let start = splice.start.saturating_add_signed(shift);
        let new_len = splice.text.byte_len();
        let range = start..start + splice.len;
        // A whole splice, as `apply_marking` applies one.
        if self.piece.is_none() && new_len <= budget.piece {
            self.old_below = self.old_below.saturating_sub(splice.len);
            self.new_below = self.new_below.saturating_sub(new_len);
            let removed = (!range.is_empty()).then(|| text.slice(range.clone()));
            let edit = splice.text.replace_in(text, range);
            sync(text, display, &edit, wraps);
            let after = edit.new.end.saturating_sub(start + new_len);
            let work = SPLICE_WORK + new_len;
            budget.spend(work);
            *done += work;
            self.applied(
                start,
                (splice.len, new_len),
                removed,
                (edit.new.start, after),
                chunk,
            );
            return self.next_part(group);
        }
        let mut piece = match self.piece.take() {
            Some(piece) => piece,
            None => {
                // Its text goes once, and the pieces then go in after one another.
                self.old_below = self.old_below.saturating_sub(splice.len);
                self.new_below = self.new_below.saturating_sub(new_len);
                budget.spend(SPLICE_WORK);
                *done += SPLICE_WORK;
                let mut piece = Piece {
                    removed: None,
                    put: 0,
                    rebuilt: start,
                    after: 0,
                };
                if !range.is_empty() {
                    piece.removed = Some(text.slice(range.clone()));
                    let edit = text.replace(range, "");
                    sync(text, display, &edit, wraps);
                    self.piece = Some(Piece {
                        rebuilt: edit.new.start,
                        after: edit.new.end.saturating_sub(start),
                        ..piece
                    });
                    return false;
                }
                piece
            }
        };
        let Some(source) = &splice.text else {
            return self.next_part(group);
        };
        // A character is at most four bytes, so a piece always holds one.
        let end = source.clip_offset(piece.put + budget.piece.max(4), Bias::Left);
        let mut put = String::with_capacity(end.saturating_sub(piece.put));
        put.extend(source.chunks_in_range(piece.put..end));
        let at = start + piece.put;
        let edit = text.replace(at..at, &put);
        sync(text, display, &edit, wraps);
        piece.rebuilt = piece.rebuilt.min(edit.new.start);
        piece.after = piece.after.max(edit.new.end.saturating_sub(at + put.len()));
        piece.put += put.len();
        budget.spend(put.len());
        *done += put.len();
        if piece.put < new_len && !put.is_empty() {
            self.piece = Some(piece);
            return false;
        }
        let rebuilt = (piece.rebuilt, piece.after);
        self.applied(start, (splice.len, new_len), piece.removed, rebuilt, chunk);
        self.next_part(group)
    }

    /// Takes in a splice that applied whole: at `start` in the offsets before the run, it
    /// replaced `old_len` bytes with `new_len`, removed `removed`, and the chunks it rebuilt
    /// started at `rebuilt.0`, reaching `rebuilt.1` bytes past its text.
    fn applied(
        &mut self,
        start: usize,
        (old_len, new_len): (usize, usize),
        removed: Option<Rope>,
        (rebuilt, after): (usize, usize),
        chunk: &mut Vec<OffsetEdit>,
    ) {
        let post = (start + self.new_below).saturating_sub(self.old_below);
        // The chunks the splices above rebuilt from at or past this one's start: their start
        // now goes to its place after the run, or to this one's start if it is in this one,
        // which holds at least as much.
        while let Some(&(at, end)) = self.pending.peek()
            && at >= start
        {
            self.pending.pop();
            let moved = if start + old_len < at {
                (at + self.new_below + new_len).saturating_sub(self.old_below + old_len)
            } else {
                post
            };
            add_descending(&mut self.changed, moved..end);
        }
        self.pending.push((rebuilt, post + new_len + after));
        let splice = Splice {
            start: post,
            len: new_len,
            text: removed,
        };
        self.counts.add(&splice);
        self.undo.push(splice);
        let edit = OffsetEdit {
            start,
            old_len,
            new_len,
        };
        chunk.push(edit);
        self.edits.push(edit);
        self.last = Some((start, old_len));
        self.left = self.left.saturating_sub(1);
    }

    /// Moves on to the next part once the current one has applied. Returns whether the run
    /// ended.
    fn next_part(&mut self, group: &[Batch]) -> bool {
        while self.left == 0 {
            self.part += 1;
            let Some(&(index, _)) = self.parts.get(self.part) else {
                return true;
            };
            self.left = group[index].len();
        }
        false
    }
}

/// Adds `range` to `ranges`, descending and apart, joining those it meets or touches.
fn add_descending(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    let above = ranges.partition_point(|other| other.start > range.end);
    let met = ranges[above..].partition_point(|other| other.end >= range.start);
    let joined = ranges[above..above + met].iter().fold(range, |all, other| {
        all.start.min(other.start)..all.end.max(other.end)
    });
    ranges.splice(above..above + met, std::iter::once(joined));
}

#[cfg(test)]
mod tests;
