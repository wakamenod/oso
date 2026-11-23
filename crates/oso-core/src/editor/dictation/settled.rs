//! Settled spans (ADR 0175, step 3): a dictation's span once its fix ended, or a plain
//! dictation's, kept undrawn so that the host can learn from the person's rewrites of it; and
//! how every span, drawn or settled, follows an edit, noting whose it was.
//!
//! Must not depend on: the host's clock or threads; the host says when a watch ends.

use super::{Dictation, MAX_SETTLED};
use crate::buffer::BufferId;
use crate::editor::{DictationSpan, Editor};
use crate::selection::{OffsetEdit, follow_sorted, follow_span_starts};
use crate::sum_tree::Bias;

impl Editor {
    /// Settles `span`: its face goes, but it stays, undrawn, so that the host can learn from
    /// the person's rewrites of it.
    pub fn settle_dictation_span(&mut self, span: DictationSpan) {
        if let Some(held) = self
            .dictation
            .spans
            .iter_mut()
            .find(|held| held.id == span.0)
        {
            held.drawn = false;
        }
        self.keep_settled();
    }

    /// Ends the oldest settled spans past [`MAX_SETTLED`].
    pub(super) fn keep_settled(&mut self) {
        let settled = self.dictation.spans.iter().filter(|held| !held.drawn);
        let extra = settled.count().saturating_sub(MAX_SETTLED);
        if extra == 0 {
            return;
        }
        let mut oldest: Vec<u64> = (self.dictation.spans.iter())
            .filter(|held| !held.drawn)
            .map(|held| held.id)
            .collect();
        oldest.sort_unstable();
        oldest.truncate(extra);
        self.dictation
            .spans
            .retain(|held| !oldest.contains(&held.id));
    }

    /// How many of the person's commands changed `span`'s text since it was made. `None`
    /// once it is gone: it ends here, and is gone then, if anything else changed its text or
    /// its buffer may hold secrets now.
    pub fn dictation_span_edits(&mut self, span: DictationSpan) -> Option<u64> {
        let at = (self.dictation.spans.iter()).position(|held| held.id == span.0)?;
        let held = &self.dictation.spans[at];
        // A change by a command never seen to be the person's is not theirs: this is asked
        // between commands.
        let holds = !held.foreign
            && !held.pending
            && self
                .buffer(held.buffer)
                .is_some_and(|buffer| !buffer.is_secret());
        if !holds {
            self.dictation.spans.remove(at);
            return None;
        }
        Some(held.edits)
    }

    /// The text of `span`'s first range, if it is at most `max` bytes. `None` once the span is
    /// gone, or its buffer may hold secrets.
    pub fn dictation_span_text(&self, span: DictationSpan, max: usize) -> Option<String> {
        let held = self.dictation_span_at(span)?;
        let buffer = self
            .buffer(held.buffer)
            .filter(|buffer| !buffer.is_secret())?;
        let range = held.ranges.first()?.clone();
        (range.len() <= max && range.end <= buffer.text().len())
            .then(|| buffer.text().slice(range).to_string())
    }

    /// A command starts: a change to a span by the one before, never seen to be the person's,
    /// was not.
    pub(in crate::editor) fn dictation_command_begins(&mut self) {
        for held in &mut self.dictation.spans {
            if std::mem::take(&mut held.pending) {
                held.foreign = true;
            }
        }
    }

    /// The command that ended was the person's: the spans it changed count it.
    pub(in crate::editor) fn dictation_persons_command(&mut self) {
        for held in &mut self.dictation.spans {
            if std::mem::take(&mut held.pending) {
                held.edits += 1;
            }
        }
    }

    /// Moves the spans in `buffer` through an edit's `steps`. A start goes after text typed at
    /// it and an end stays before it, so that what the person types at either end is not the
    /// dictation's; a replacement of what begins at a start stays in. A span whose text an
    /// edit changed notes whose edit it was: a command's, until it is seen to be the person's,
    /// or another's. O(ranges + edits) a step.
    pub(in crate::editor) fn dictation_spans_follow(
        &mut self,
        buffer: BufferId,
        steps: &[Vec<OffsetEdit>],
    ) {
        let persons = self.in_command() && !self.runs_for_agent();
        let Dictation {
            spans,
            starts,
            ends,
            ..
        } = &mut self.dictation;
        for held in spans.iter_mut().filter(|held| held.buffer == buffer) {
            starts.clear();
            ends.clear();
            starts.extend(held.ranges.iter().map(|range| range.start));
            ends.extend(held.ranges.iter().map(|range| range.end));
            for edits in steps {
                if touches(starts, ends, edits) {
                    if persons {
                        held.pending = true;
                    } else {
                        held.foreign = true;
                    }
                }
                follow_span_starts(starts, edits);
                follow_sorted(ends, edits, Bias::Left);
            }
            for ((range, &start), &end) in held.ranges.iter_mut().zip(&*starts).zip(&*ends) {
                *range = start..end.max(start);
            }
        }
    }

    /// Whether `buffer` has spans, which an undo's step keeps its edits for.
    pub(in crate::editor) fn has_dictation_spans(&self, buffer: BufferId) -> bool {
        self.dictation
            .spans
            .iter()
            .any(|held| held.buffer == buffer)
    }

    /// `buffer` was read again or went: its spans go with what they marked.
    pub(in crate::editor) fn forget_dictation_spans(&mut self, buffer: BufferId) {
        self.dictation.spans.retain(|held| held.buffer != buffer);
    }
}

/// Whether one of `edits`, ascending, changes text inside a range of `starts` and `ends`,
/// ascending and apart: replaces some of it, or inserts strictly inside it. O(ranges + edits).
fn touches(starts: &[usize], ends: &[usize], edits: &[OffsetEdit]) -> bool {
    let mut edits = edits.iter().peekable();
    for (&start, &end) in starts.iter().zip(ends) {
        while let Some(edit) = edits.peek() {
            let edit_end = edit.start + edit.old_len;
            if edit_end < start || (edit.old_len > 0 && edit_end == start) {
                edits.next();
                continue;
            }
            if edit.start >= end {
                break;
            }
            let inside = if edit.old_len == 0 {
                edit.start > start
            } else {
                edit_end > start
            };
            if inside {
                return true;
            }
            edits.next();
        }
    }
    false
}

#[cfg(test)]
mod tests;
