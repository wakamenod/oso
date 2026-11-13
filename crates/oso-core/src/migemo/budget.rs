//! What a migemo pattern's dictionary words may spend (ADR 0116, Q7): one prompt's input
//! shares one pattern's caps, each migemo component spends from them in turn, and what a
//! component spent tells whether a later budget gives it the same words.
//!
//! Must not depend on: any module of this crate but its parent's.

use super::dictionary::{MAX_PATTERN_WORD_BYTES, MAX_PATTERN_WORDS};

/// What a pattern's words may spend: how many, and how many bytes by their cost, which counts
/// a character with a case several times (see [`super::Dictionary`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Budget {
    pub(crate) words: usize,
    pub(crate) bytes: usize,
}

impl Budget {
    /// One pattern's: the most words and bytes of words it takes.
    pub(crate) const PATTERN: Self = Self {
        words: MAX_PATTERN_WORDS,
        bytes: MAX_PATTERN_WORD_BYTES,
    };

    /// Whether `cost` fits within it.
    pub(crate) fn holds(self, cost: Self) -> bool {
        cost.words <= self.words && cost.bytes <= self.bytes
    }

    /// What is left of it once `cost` is spent.
    pub(crate) fn spend(&mut self, cost: Self) {
        self.words = self.words.saturating_sub(cost.words);
        self.bytes = self.bytes.saturating_sub(cost.bytes);
    }

    /// The smaller of it and `other` in each.
    pub(super) fn within(self, other: Self) -> Self {
        Self {
            words: self.words.min(other.words),
            bytes: self.bytes.min(other.bytes),
        }
    }
}

/// What a pattern's words used of the budget they were given, and, if the budget left a word
/// out, what they would have used with it. The words a budget gives depend on these alone,
/// so a pattern compiled with one budget is the one another gives when [`Spent::keep`] says
/// so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Spent {
    pub(crate) used: Budget,
    pub(crate) next: Option<Budget>,
}

impl Spent {
    /// Whether `budget` gives the same words: they fit within it, and the next one does not.
    pub(crate) fn same_within(&self, budget: Budget) -> bool {
        budget.holds(self.used) && self.next.is_none_or(|next| !budget.holds(next))
    }

    /// Whether a pattern that spent this may be kept with what is left of `budget`, which it
    /// then spends, as compiling it again would.
    pub(crate) fn keep(&self, budget: &mut Budget) -> bool {
        let same = self.same_within(*budget);
        if same {
            budget.spend(self.used);
        }
        same
    }
}
