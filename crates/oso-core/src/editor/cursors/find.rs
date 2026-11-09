//! Finding the occurrences the keys that add cursors look for (ADR 0091): isearch's literal
//! matcher, over whole display clusters only, and for a string taken from a word, only
//! matches that are a whole word.
//!
//! Must not depend on: `command`, `keymap` or the editor's state; these are functions of a
//! snapshot of a buffer, which also run on the worker's thread.
//!
//! What a search reads is taken from one budget, `left`, in bytes: the text the matcher
//! reads, and for each match it finds, the cost of the checks that decide whether it is
//! an occurrence. On the UI thread that is [`KEY_BUDGET`](crate::search::KEY_BUDGET) for a
//! whole command, also across a count; on the worker it has no bound, and `stop` is asked
//! before each match.

use std::ops::Range;

use crate::decode::InvalidBytes;
use crate::display_map::{DisplaySnapshot, MAX_CLUSTER_BYTES};
use crate::motion::{BACK_WINDOW, SEEK_COST, WalkBudget};
use crate::rope::Rope;
use crate::search::{Found, Matched, Query, find_backward, find_forward};
use crate::selection::MAX_CURSORS;
use crate::sum_tree::Bias;
use crate::word;

/// What checking that a match starts and ends on clusters costs: two seeks.
const CLUSTER_CHECK: usize = 2 * SEEK_COST;

/// What `M-f` may read past a match to find that the word ends there: its seeks, and the
/// clusters after it that decide a break.
const AFTER_WORD: usize = 4 * SEEK_COST + 4 * MAX_CLUSTER_BYTES;

/// What `M-b` may read past a match to find that the word starts there: its seeks, and a
/// backward window of the widest clusters.
const BEFORE_WORD: usize = 4 * SEEK_COST + 2 * BACK_WINDOW * MAX_CLUSTER_BYTES;

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

/// What `check` decides with a budget of `cap`, taking what it read from `left`.
fn charged<T>(left: &mut usize, cap: usize, check: impl FnOnce(&mut WalkBudget) -> T) -> T {
    let mut budget = WalkBudget::with_limit(cap);
    let decided = check(&mut budget);
    *left = left.saturating_sub(cap - budget.left());
    decided
}

/// How a search for every occurrence ended.
pub(crate) enum All {
    /// It read the whole text.
    Done(Matched),
    /// Its budget or `stop` ended it first.
    Stopped,
}

impl Text {
    /// Whether `range`, a match, is an occurrence: it starts and ends on display clusters,
    /// as a cursor can only be there, and is one word as `M-f` and `M-b` find words if
    /// `wanted` asks. What the checks read is taken from `left`.
    fn accepts(&self, wanted: &Wanted, range: &Range<usize>, left: &mut usize) -> bool {
        *left = left.saturating_sub(CLUSTER_CHECK);
        let on_clusters = self.display.cluster_at(range.start).0 == range.start
            && self.display.cluster_at(range.end).0 == range.end;
        if !on_clusters || !wanted.whole_word {
            return on_clusters;
        }
        let display = &self.display;
        // Inside a run of ASCII letters and digits, three bytes tell.
        if self.ascii_word_goes_on(range.start) || self.ascii_word_goes_on(range.end) {
            return false;
        }
        // Its edges next, from the clusters around them, so that a match inside a long run
        // of letters costs a seek or two. Then the motions, each of which may read the word
        // and what decides its edge, and stops short in a longer one, which is not this word.
        word::word_at(display, range.start)
            && charged(left, BEFORE_WORD, |budget| {
                word::may_start_word(display, range.start, budget)
            })
            && charged(left, BEFORE_WORD, |budget| {
                word::may_end_word(display, range.end, budget)
            })
            && charged(left, range.len() + AFTER_WORD, |budget| {
                word::forward_word(display, range.start, budget) == range.end
            })
            && charged(left, range.len() + BEFORE_WORD, |budget| {
                word::backward_word(display, range.end, budget) == range.start
            })
    }

    /// Whether a word surely goes on across `at`: an ASCII letter or digit before it and one
    /// at it, with no break between them as `word.rs` breaks words, which for these is only
    /// a lower-case letter or digit before a capital, or the last of a run of capitals before
    /// a lower-case letter or digit. The byte after decides that, and must be ASCII too, as
    /// a mark or an emoji selector would join what is at `at`.
    fn ascii_word_goes_on(&self, at: usize) -> bool {
        let Some(start) = at.checked_sub(1) else {
            return false;
        };
        let end = (at + 2).min(self.rope.len());
        // Before a character of several bytes the range starts at its first, which is no
        // ASCII and decides nothing.
        let mut bytes = [0u8; 3];
        let mut len = 0;
        let read = self.rope.chunks_in_range(start..end).flat_map(str::bytes);
        for (slot, byte) in bytes.iter_mut().zip(read) {
            *slot = byte;
            len += 1;
        }
        let (before, here) = (bytes[0], bytes[1]);
        let after = (len == 3).then_some(bytes[2]);
        let lower = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
        if len < 2 || !before.is_ascii_alphanumeric() || !here.is_ascii_alphanumeric() {
            return false;
        }
        if after.is_some_and(|after| !after.is_ascii()) {
            return false;
        }
        let splits = (lower(before) && here.is_ascii_uppercase())
            || (before.is_ascii_uppercase()
                && here.is_ascii_uppercase()
                && after.is_some_and(lower));
        !splits
    }

    /// The next occurrence of `wanted` that starts at `from` or after, taking what it reads
    /// from `left`.
    fn next(
        &self,
        wanted: &Wanted,
        mut from: usize,
        left: &mut usize,
        stop: &dyn Fn() -> bool,
    ) -> Found {
        loop {
            if *left == 0 || stop() {
                return Found::Stopped { resume: from };
            }
            let found = find_forward(&self.rope, &self.invalid, &wanted.query, from, *left, stop);
            let Found::Match(range) = found else {
                return found;
            };
            *left = left.saturating_sub(range.end - from);
            if self.accepts(wanted, &range, left) {
                return Found::Match(range);
            }
            from = self.rope.clip_offset(range.start + 1, Bias::Right);
        }
    }

    /// The last occurrence of `wanted` that ends at `before` or earlier, taking what it
    /// reads from `left`. After a match it does not take, it reads again only as far back
    /// as a match that starts before that one can end.
    fn previous(
        &self,
        wanted: &Wanted,
        mut before: usize,
        left: &mut usize,
        stop: &dyn Fn() -> bool,
    ) -> Found {
        let mut start_at_most = before;
        loop {
            if *left == 0 || stop() {
                return Found::Stopped { resume: before };
            }
            let found = find_backward(
                &self.rope,
                &self.invalid,
                &wanted.query,
                before,
                start_at_most,
                *left,
                stop,
            );
            let Found::Match(range) = found else {
                return found;
            };
            *left = left.saturating_sub(before - range.start);
            if self.accepts(wanted, &range, left) {
                return Found::Match(range);
            }
            let Some(earlier) = range.start.checked_sub(1) else {
                return Found::None;
            };
            start_at_most = self.rope.clip_offset(earlier, Bias::Left);
            let reach = start_at_most.saturating_add(wanted.query.max_bytes());
            before = before.min(self.rope.clip_offset(reach, Bias::Right));
        }
    }

    /// Up to `most` occurrences `toward` either way from `from`, taking what it reads from
    /// `left`, into `out`. Returns where to go on from if `left` or `stop` ended it first.
    pub(crate) fn several(
        &self,
        wanted: &Wanted,
        (toward, from): (Toward, usize),
        (most, left): (usize, &mut usize),
        stop: &dyn Fn() -> bool,
        out: &mut Vec<Range<usize>>,
    ) -> Option<usize> {
        let mut at = from;
        while out.len() < most {
            let found = match toward {
                Toward::Next => self.next(wanted, at, left, stop),
                Toward::Previous => self.previous(wanted, at, left, stop),
            };
            match found {
                Found::Match(range) => {
                    at = match toward {
                        Toward::Next => range.end,
                        Toward::Previous => range.start,
                    };
                    out.push(range);
                }
                Found::None => return None,
                Found::Stopped { resume } => return Some(resume),
            }
        }
        None
    }

    /// Every occurrence in the text, in order and apart, as a scan from the start finds
    /// them: the first [`MAX_CURSORS`] and how many there are, taking what it reads from
    /// `left`.
    pub(crate) fn all(&self, wanted: &Wanted, left: &mut usize, stop: &dyn Fn() -> bool) -> All {
        let mut matched = Matched {
            ranges: Vec::new(),
            total: 0,
        };
        let mut from = 0;
        loop {
            match self.next(wanted, from, left, stop) {
                Found::Match(range) => {
                    from = range.end;
                    if matched.ranges.len() < MAX_CURSORS {
                        matched.ranges.push(range);
                    }
                    matched.total += 1;
                }
                Found::None => return All::Done(matched),
                Found::Stopped { .. } => return All::Stopped,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::display_map::{DisplayMap, TabWidth};
    use crate::rope::tests::ATOMS;
    use crate::search::Found;

    fn text(string: &str) -> Text {
        let rope = Rope::from(string);
        let display = DisplayMap::new(&rope, TabWidth::new(4)).snapshot();
        Text {
            invalid: InvalidBytes::default(),
            rope,
            display,
        }
    }

    proptest! {
        /// The checks with their caps take what checks with no bound take: every match on
        /// clusters, and of those wanted as a word, those `M-f` and `M-b` find whole.
        #[test]
        fn capped_checks_decide_as_unbounded_ones(
            atoms in prop::collection::vec(
                prop_oneof![
                    prop::sample::select(ATOMS),
                    prop::sample::select(&[
                        "ab", "ab", "a", "b", "abab", "Ab", " ", "\u{301}", "\u{301}\u{301}", "日本",
                        "の", "HTTP", "Server", "aB", "カー", "x\u{301}", "_", "1",
                    ][..]),
                ],
                0..40,
            ),
            query in prop::sample::select(&["ab", "a", "b", "日本", "の", "\u{301}", "HTTP", "Server", "B"][..]),
            whole_word in any::<bool>(),
        ) {
            let string = atoms.concat();
            let text = text(&string);
            let wanted = Wanted { query: Query::new(query, true).unwrap(), whole_word };
            let mut from = 0;
            while let Found::Match(range) =
                find_forward(&text.rope, &text.invalid, &wanted.query, from, usize::MAX, &|| false)
            {
                let display = &text.display;
                let on_clusters = display.cluster_at(range.start).0 == range.start
                    && display.cluster_at(range.end).0 == range.end;
                let mut budget = WalkBudget::new();
                let whole = word::word_at(display, range.start)
                    && word::forward_word(display, range.start, &mut budget) == range.end
                    && word::backward_word(display, range.end, &mut budget) == range.start;
                let expected = on_clusters && (!whole_word || whole);
                let mut left = usize::MAX;
                prop_assert_eq!(text.accepts(&wanted, &range, &mut left), expected);
                from = text.rope.clip_offset(range.start + 1, Bias::Right);
            }
        }
    }
}
