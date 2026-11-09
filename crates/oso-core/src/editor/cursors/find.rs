//! Finding the occurrences the keys that add cursors look for (ADR 0091): isearch's literal
//! matcher, and for a string taken from a word, only matches that are a whole word.
//!
//! Must not depend on: `command`, `keymap` or the editor's state; these are functions of a
//! snapshot of a buffer, which also run on the worker's thread.

use std::ops::Range;

use crate::decode::InvalidBytes;
use crate::display_map::DisplaySnapshot;
use crate::motion::WalkBudget;
use crate::rope::Rope;
use crate::search::{Found, Matched, Query, find_backward, find_forward};
use crate::selection::MAX_CURSORS;
use crate::sum_tree::Bias;
use crate::word;

/// Which way a search for cursors goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toward {
    /// After the last cursor, as `C->`.
    Next,
    /// Before the first cursor, as `C-<`.
    Previous,
}

/// What occurrences are looked for: the query, and whether a match must be a whole word,
/// as when the string was taken from one. Its `Debug` prints no text (ADR 0044).
#[derive(Clone, Debug)]
pub(crate) struct Wanted {
    pub(crate) query: Query,
    pub(crate) whole_word: bool,
}

/// A buffer's text as a search for cursors reads it: O(1) clones, so that a worker can
/// take them.
#[derive(Clone)]
pub(crate) struct Text {
    pub(crate) rope: Rope,
    pub(crate) invalid: InvalidBytes,
    pub(crate) display: DisplaySnapshot,
}

impl Text {
    /// Whether `range` is one word, as `M-f` and `M-b` find words.
    fn is_whole_word(&self, range: &Range<usize>) -> bool {
        let mut budget = WalkBudget::new();
        word::word_at(&self.display, range.start)
            && word::forward_word(&self.display, range.start, &mut budget) == range.end
            && word::backward_word(&self.display, range.end, &mut budget) == range.start
    }

    /// The next occurrence of `wanted` that starts at `from` or after, reading about
    /// `limit` bytes, which it takes from.
    fn next(
        &self,
        wanted: &Wanted,
        mut from: usize,
        limit: &mut usize,
        stop: &dyn Fn() -> bool,
    ) -> Found {
        loop {
            let found = find_forward(&self.rope, &self.invalid, &wanted.query, from, *limit, stop);
            let Found::Match(range) = found else {
                *limit = 0;
                return found;
            };
            *limit = limit.saturating_sub(range.end - from);
            if !wanted.whole_word || self.is_whole_word(&range) {
                return Found::Match(range);
            }
            from = self.rope.clip_offset(range.start + 1, Bias::Right);
            if *limit == 0 {
                return Found::Stopped { resume: from };
            }
        }
    }

    /// The last occurrence of `wanted` that ends at `before` or earlier, reading about
    /// `limit` bytes, which it takes from.
    fn previous(
        &self,
        wanted: &Wanted,
        before: usize,
        limit: &mut usize,
        stop: &dyn Fn() -> bool,
    ) -> Found {
        let mut start_at_most = before;
        loop {
            let found = find_backward(
                &self.rope,
                &self.invalid,
                &wanted.query,
                before,
                start_at_most,
                *limit,
                stop,
            );
            let Found::Match(range) = found else {
                *limit = 0;
                return found;
            };
            *limit = limit.saturating_sub(start_at_most.saturating_sub(range.start));
            if !wanted.whole_word || self.is_whole_word(&range) {
                return Found::Match(range);
            }
            let Some(earlier) = range.start.checked_sub(1) else {
                return Found::None;
            };
            start_at_most = self.rope.clip_offset(earlier, Bias::Left);
            if *limit == 0 {
                return Found::Stopped {
                    resume: start_at_most,
                };
            }
        }
    }

    /// Up to `most` occurrences `toward` either way from `from`, reading about `limit`
    /// bytes, into `out`. Returns where to go on from if the budget or `stop` ended it
    /// first.
    pub(crate) fn several(
        &self,
        wanted: &Wanted,
        (toward, from): (Toward, usize),
        (most, limit): (usize, &mut usize),
        stop: &dyn Fn() -> bool,
        out: &mut Vec<Range<usize>>,
    ) -> Option<usize> {
        let mut at = from;
        while out.len() < most {
            let found = match toward {
                Toward::Next => self.next(wanted, at, limit, stop),
                Toward::Previous => self.previous(wanted, at, limit, stop),
            };
            match found {
                Found::Match(range) => {
                    at = match toward {
                        Toward::Next => range.end.max(at + 1).min(self.rope.len()),
                        Toward::Previous => range.start,
                    };
                    out.push(range);
                    if toward == Toward::Previous && at == 0 {
                        return None;
                    }
                }
                Found::None => return None,
                Found::Stopped { resume } => return Some(resume),
            }
        }
        None
    }

    /// Every occurrence in the text, in order and apart, as a scan from the start finds
    /// them: the first [`MAX_CURSORS`] and how many there are. `None` if `stop` said so.
    pub(crate) fn all(&self, wanted: &Wanted, stop: &dyn Fn() -> bool) -> Option<Matched> {
        let mut matched = Matched {
            ranges: Vec::new(),
            total: 0,
        };
        let mut from = 0;
        loop {
            let mut limit = usize::MAX;
            match self.next(wanted, from, &mut limit, stop) {
                Found::Match(range) => {
                    from = range.end.max(from + 1);
                    if matched.ranges.len() < MAX_CURSORS {
                        matched.ranges.push(range);
                    }
                    matched.total += 1;
                    if from >= self.rope.len() {
                        return Some(matched);
                    }
                }
                Found::None => return Some(matched),
                Found::Stopped { .. } => return None,
            }
        }
    }
}
