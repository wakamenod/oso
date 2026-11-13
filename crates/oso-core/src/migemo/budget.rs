//! What a migemo pattern's dictionary words, and the lookup that finds them, may spend
//! (ADR 0116, Q7): one pattern's caps, what a word costs against them, and how one prompt's
//! input shares them, each migemo component spending from them in turn. What a component
//! spent tells whether a later budget gives it the same words.
//!
//! Must not depend on: any module of this crate but its parent's.

/// The most steps a pattern's lookup may take, which one input's components share (ADR 0116,
/// Q7): narrowing a range of entries by a kana takes one step and one more for each time its
/// length halves, as its binary search does, and each entry found takes one as they are
/// ordered. Past them, a lookup stops and gives no words.
/// The most a common dictionary takes for one input is a small share of them, and a crafted
/// one cannot make a key's lookups slow.
pub(crate) const MAX_LOOKUP_STEPS: usize = 64 * 1024;

/// The most words of a pattern: the exact readings' first, then the shorter readings'.
pub(crate) const MAX_PATTERN_WORDS: usize = 4096;

/// The most bytes of a pattern's words, by [`word_cost`], about twice those of the largest
/// pattern a common dictionary gives, so a pattern compiles within a key's time whatever the
/// file holds.
pub(crate) const MAX_PATTERN_WORD_BYTES: usize = 32 * 1024;

/// How many times its bytes a character with a case counts against
/// [`MAX_PATTERN_WORD_BYTES`]: a search that ignores case compiles it as a class of every
/// character it folds to, which costs that many times a character with none.
const CASE_WEIGHT: usize = 4;

/// What `word` counts against [`MAX_PATTERN_WORD_BYTES`]: its bytes, each character with a
/// case [`CASE_WEIGHT`] times over.
pub(crate) fn word_cost(word: &str) -> usize {
    word.chars()
        .map(|c| c.len_utf8() * if has_case(c) { CASE_WEIGHT } else { 1 })
        .sum()
}

/// Whether `c` has another case a search that ignores case also matches: a letter that is
/// lowercase or uppercase, or that a case mapping changes, as a titlecase one.
fn has_case(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphabetic();
    }
    c.is_lowercase() || c.is_uppercase() || !c.to_lowercase().eq([c]) || !c.to_uppercase().eq([c])
}

/// What a pattern's words may spend: how many, how many bytes by their cost, which counts a
/// character with a case several times (see [`word_cost`]), and how many steps the
/// lookup that finds them may take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Budget {
    pub(crate) words: usize,
    pub(crate) bytes: usize,
    pub(crate) steps: usize,
}

impl Budget {
    /// One pattern's: the most words and bytes of words it takes, and the most steps its
    /// lookup does.
    pub(crate) const PATTERN: Self = Self {
        words: MAX_PATTERN_WORDS,
        bytes: MAX_PATTERN_WORD_BYTES,
        steps: MAX_LOOKUP_STEPS,
    };

    /// Whether `cost` fits within it.
    pub(crate) fn holds(self, cost: Self) -> bool {
        cost.words <= self.words && cost.bytes <= self.bytes && cost.steps <= self.steps
    }

    /// What is left of it once `cost` is spent.
    pub(crate) fn spend(&mut self, cost: Self) {
        self.words = self.words.saturating_sub(cost.words);
        self.bytes = self.bytes.saturating_sub(cost.bytes);
        self.steps = self.steps.saturating_sub(cost.steps);
    }

    /// The smaller of it and `other` in each.
    pub(super) fn within(self, other: Self) -> Self {
        Self {
            words: self.words.min(other.words),
            bytes: self.bytes.min(other.bytes),
            steps: self.steps.min(other.steps),
        }
    }
}

/// What a pattern's words used of the budget they were given, and, if the budget left a word
/// out or stopped the lookup short, what they would have used with it. The words a budget gives depend on these alone,
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
