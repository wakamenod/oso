//! Jumps by label, as avy's (ADR 0116): what a jump under way holds, the keys its labels are
//! typed with, and the tree that gives each candidate its label.
//!
//! Must not depend on: `editor`, `command` or `keymap`; the editor finds the candidates and
//! moves the cursor, and this holds what it found.
//!
//! A jump reads a string first (`avy-goto-char-timer`), or goes straight to its labels (the
//! line jumps and `avy-isearch`). Labels come from [`Labels`], avy's tree: as many candidates
//! as can get one key do, nearest first, and the rest share the other keys as prefixes, so no
//! label starts another. `Debug` prints none of the string, which can say something of a
//! buffer that may hold secrets (ADR 0044).

use std::fmt::{self, Write};
use std::ops::Range;

use crate::buffer::BufferId;
use crate::display_map::DisplayPoint;
use crate::window::WindowId;

/// The fewest keys labels can be typed with.
pub const MIN_KEYS: usize = 2;

/// The most keys labels can be typed with.
pub const MAX_KEYS: usize = 32;

/// The most candidates a jump labels: eight keys label them with four keys or fewer. Past
/// that, the farthest are not labelled.
pub const MAX_LABELLED: usize = 4096;

/// The most characters a jump's string holds.
pub const MAX_STRING: usize = 64;

/// The keys of the actions on a target (ADR 0116 step 6), which no label may use.
const ACTION_KEYS: &str = "WKYTc";

/// The label keys when `[jump] keys` gives none: the home row without `k`, which an action
/// may take.
const DEFAULT_KEYS: &str = "asdfghjl";

/// What a jump labels, by the command that began it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The matches in view of a string it reads first (`avy-goto-char-timer`).
    Char,
    /// The start of each line in view (`avy-goto-line`).
    LineStart,
    /// The end of each line in view (`avy-goto-end-of-line`).
    LineEnd,
    /// isearch's matches in view (`avy-isearch`).
    Isearch,
}

/// Where a jump has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// It reads its string, and shows the string's matches.
    Reading,
    /// It shows its labels, and reads one.
    Labels,
}

/// The keys labels are typed with, as `[jump] keys` gives them: 2 to 32 distinct printable
/// ASCII characters, none of them an action's key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys(String);

/// Why a string cannot be label keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeysError {
    /// It holds this many characters, not [`MIN_KEYS`] to [`MAX_KEYS`].
    Count(usize),
    /// A character that is not printable ASCII, a space among them.
    NotPrintable(char),
    /// A character given twice.
    Twice(char),
    /// An action's key.
    Action(char),
}

impl fmt::Display for KeysError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Count(count) => {
                write!(f, "takes {MIN_KEYS} to {MAX_KEYS} characters, not {count}")
            }
            Self::NotPrintable(c) => write!(f, "takes printable ASCII characters, not {c:?}"),
            Self::Twice(c) => write!(f, "gives {c:?} twice"),
            Self::Action(c) => write!(f, "cannot take {c:?}, an action's key"),
        }
    }
}

impl std::error::Error for KeysError {}

impl Keys {
    /// The keys of `keys`, each character one.
    ///
    /// # Errors
    ///
    /// A [`KeysError`] for the first thing wrong with it.
    pub fn new(keys: &str) -> Result<Self, KeysError> {
        let count = keys.chars().count();
        if !(MIN_KEYS..=MAX_KEYS).contains(&count) {
            return Err(KeysError::Count(count));
        }
        for (index, c) in keys.char_indices() {
            if !c.is_ascii_graphic() {
                return Err(KeysError::NotPrintable(c));
            }
            if ACTION_KEYS.contains(c) {
                return Err(KeysError::Action(c));
            }
            if keys[..index].contains(c) {
                return Err(KeysError::Twice(c));
            }
        }
        Ok(Self(keys.to_owned()))
    }

    /// The keys, in the order labels take them.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The key at `index`, as a byte: every key is ASCII.
    pub(crate) fn byte(&self, index: u8) -> u8 {
        self.0
            .as_bytes()
            .get(usize::from(index))
            .copied()
            .unwrap_or(b'?')
    }

    /// Where `key` is among the keys.
    pub(crate) fn index_of(&self, key: char) -> Option<u8> {
        let at = self.0.find(key)?;
        u8::try_from(at).ok()
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

impl Default for Keys {
    fn default() -> Self {
        Self(DEFAULT_KEYS.to_owned())
    }
}

/// Each candidate's label, as indices into the keys, from avy's tree (`avy-tree`): earlier
/// candidates get labels no longer than later ones, and no label starts another.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Labels {
    keys: Vec<u8>,
    /// Where each label ends in `keys`.
    ends: Vec<u32>,
}

impl Labels {
    /// The labels of `count` candidates, at most [`MAX_LABELLED`], typed with `keys` keys,
    /// [`MIN_KEYS`] to [`MAX_KEYS`].
    pub fn new(keys: usize, count: usize) -> Self {
        let keys = keys.clamp(MIN_KEYS, MAX_KEYS);
        let count = count.min(MAX_LABELLED);
        let mut labels = Self {
            keys: Vec::new(),
            ends: Vec::with_capacity(count),
        };
        let mut prefix = Vec::new();
        labels.tree(keys, count, &mut prefix);
        labels
    }

    /// How many candidates have a label.
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    /// Whether no candidate has one.
    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// The label of the `index`th candidate, as indices into the keys.
    pub fn get(&self, index: usize) -> Option<&[u8]> {
        let end = usize::try_from(*self.ends.get(index)?).ok()?;
        let start = match index.checked_sub(1) {
            Some(before) => usize::try_from(self.ends[before]).ok()?,
            None => 0,
        };
        self.keys.get(start..end)
    }

    /// Labels `count` more candidates under `prefix`, as `avy-tree` does: one key each while
    /// they are fewer than the keys, and otherwise a share of them under each key, as
    /// [`subdivide`] splits them.
    fn tree(&mut self, keys: usize, count: usize, prefix: &mut Vec<u8>) {
        if count < keys {
            for key in 0..count {
                self.push(prefix, key);
            }
            return;
        }
        for (key, share) in subdivide(count, keys).into_iter().enumerate() {
            if share == 1 {
                self.push(prefix, key);
            } else {
                prefix.push(u8::try_from(key).unwrap_or(u8::MAX));
                self.tree(keys, share, prefix);
                prefix.pop();
            }
        }
    }

    fn push(&mut self, prefix: &[u8], key: usize) {
        self.keys.extend_from_slice(prefix);
        self.keys.push(u8::try_from(key).unwrap_or(u8::MAX));
        self.ends
            .push(u32::try_from(self.keys.len()).unwrap_or(u32::MAX));
    }
}

/// `count` candidates split among `keys` keys in a balanced way, as `avy-subdiv` does, for
/// `count` of at least `keys`: the first keys take the smallest shares, so earlier candidates
/// get shorter labels.
fn subdivide(count: usize, keys: usize) -> Vec<usize> {
    // The largest power of `keys` that is at most `count`, and the one below it.
    let mut big = keys;
    while let Some(next) = big.checked_mul(keys).filter(|&next| next <= count) {
        big = next;
    }
    let small = big / keys;
    let delta = count - big;
    let bigs = delta / (big - small);
    let smalls = keys - bigs - 1;
    let middle = count - smalls * small - bigs * big;
    let mut shares = vec![small; smalls];
    shares.push(middle);
    shares.extend(std::iter::repeat_n(big, bigs));
    shares
}

/// A place a jump can go: in a window, a range of its buffer's text, whose start is the
/// target, and its ends' display points, worked out once when it was found, so that a frame
/// converts none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) window: WindowId,
    pub(crate) range: Range<usize>,
    pub(crate) start: DisplayPoint,
    pub(crate) end: DisplayPoint,
}

/// A jump under way. It reads its keys in place of every other keymap, and ends at a label,
/// at `C-g`, or at any other key.
pub struct Jump {
    pub(crate) kind: Kind,
    pub(crate) stage: Stage,
    /// `C-u` kept it to the selected window.
    pub(crate) selected_only: bool,
    pub(crate) string: String,
    /// The keys its labels are typed with, as they were when it began.
    pub(crate) keys: Keys,
    /// What it found, nearest first: every match while it reads, the labelled ones after.
    pub(crate) candidates: Vec<Candidate>,
    /// How many it found in all, of which the first [`MAX_LABELLED`] are kept.
    pub(crate) found: usize,
    pub(crate) labels: Labels,
    /// The keys of a label typed so far, as indices into `keys`.
    pub(crate) typed: Vec<u8>,
    /// Each window it looks at, the buffer it shows and that buffer's revision, so that a
    /// change under its labels ends it.
    pub(crate) shown: Vec<(WindowId, BufferId, u64)>,
    /// It began while a buffer that may hold secrets was shown.
    pub(crate) around_secret: bool,
}

impl fmt::Debug for Jump {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The string and the places stay out of it (ADR 0044).
        f.debug_struct("Jump")
            .field("kind", &self.kind)
            .field("stage", &self.stage)
            .field("found", &self.found)
            .finish_non_exhaustive()
    }
}

impl Jump {
    /// A jump of `kind`, before it has found anything.
    pub(crate) fn new(kind: Kind, selected_only: bool, keys: Keys, around_secret: bool) -> Self {
        Self {
            kind,
            stage: Stage::Reading,
            selected_only,
            string: String::new(),
            keys,
            candidates: Vec::new(),
            found: 0,
            labels: Labels::default(),
            typed: Vec::new(),
            shown: Vec::new(),
            around_secret,
        }
    }

    /// What it labels.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Where it has got to.
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// How many candidates it found: the string's matches in view while it reads, and the
    /// candidates its labels were given for after.
    pub fn count(&self) -> usize {
        self.found
    }

    /// How many candidates have a label: at most [`MAX_LABELLED`], none while it reads.
    pub fn labelled(&self) -> usize {
        self.labels.len()
    }

    /// Whether it began, or went on, while a buffer that may hold secrets was shown.
    pub fn around_secret(&self) -> bool {
        self.around_secret
    }

    /// Whether `key` continues a label now: a label's key after what is typed of it.
    pub fn takes(&self, key: char) -> bool {
        self.stage == Stage::Labels
            && self
                .keys
                .index_of(key)
                .is_some_and(|index| self.labels_after(index).next().is_some())
    }

    /// The candidates whose labels start with what is typed and then the key `index`.
    pub(crate) fn labels_after(&self, index: u8) -> impl Iterator<Item = (usize, &[u8])> {
        let typed = &self.typed[..];
        (0..self.labels.len()).filter_map(move |at| {
            let label = self.labels.get(at)?;
            let (head, rest) = label.split_at_checked(typed.len())?;
            (head == typed && rest.first() == Some(&index)).then_some((at, label))
        })
    }

    /// The labelled candidates whose labels start with what is typed, with what is left of
    /// each label to type.
    pub(crate) fn shown_labels(&self) -> impl Iterator<Item = (&Candidate, &[u8])> {
        let typed = &self.typed[..];
        self.candidates
            .iter()
            .enumerate()
            .filter_map(move |(at, candidate)| {
                let label = self.labels.get(at)?;
                let rest = label.strip_prefix(typed)?;
                Some((candidate, rest))
            })
    }

    /// Gives every candidate kept a label, and shows them.
    pub(crate) fn label(&mut self) {
        self.candidates.truncate(MAX_LABELLED);
        self.labels = Labels::new(self.keys.len(), self.candidates.len());
        self.typed.clear();
        self.stage = Stage::Labels;
    }

    /// Writes what the echo row says of it into `out`: `Jump: ka`, or what it labels, and how
    /// many of its candidates have labels when not all do.
    pub(crate) fn write_line(&self, out: &mut String) {
        match self.kind {
            Kind::Char => {
                out.push_str("Jump: ");
                out.push_str(&self.string);
            }
            Kind::LineStart => out.push_str("Jump to a line"),
            Kind::LineEnd => out.push_str("Jump to a line's end"),
            Kind::Isearch => out.push_str("Jump to a match"),
        }
        if self.stage == Stage::Labels && self.found > self.labelled() {
            // Writing to a `String` cannot fail.
            let _ = write!(out, "  [{} of {} labelled]", self.labelled(), self.found);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Keys, KeysError, Labels, MAX_LABELLED, subdivide};
    use proptest::prelude::*;

    #[test]
    fn subdivide_gives_avys_shares() {
        // `(avy-subdiv 9 8)` and `(avy-subdiv 100 8)` and `(avy-subdiv 4096 8)`.
        assert_eq!(subdivide(9, 8), [1, 1, 1, 1, 1, 1, 1, 2]);
        assert_eq!(subdivide(8, 8), [1; 8]);
        assert_eq!(subdivide(100, 8), [8, 8, 8, 8, 8, 8, 8, 44]);
        assert_eq!(subdivide(4096, 8), [512; 8]);
        assert_eq!(subdivide(3, 2), [1, 2]);
    }

    #[test]
    fn fewer_candidates_than_keys_take_one_key_each() {
        let labels = Labels::new(8, 5);
        let all: Vec<_> = (0..5).map(|at| labels.get(at).unwrap().to_vec()).collect();
        assert_eq!(all, [[0], [1], [2], [3], [4]]);
        assert_eq!(labels.get(5), None);
    }

    #[test]
    fn the_last_key_leads_the_rest_past_the_keys() {
        let labels = Labels::new(8, 9);
        assert_eq!(labels.get(6), Some(&[6][..]));
        assert_eq!(labels.get(7), Some(&[7, 0][..]));
        assert_eq!(labels.get(8), Some(&[7, 1][..]));
    }

    #[test]
    fn eight_keys_label_the_most_with_four_keys_or_fewer() {
        let labels = Labels::new(8, MAX_LABELLED);
        assert_eq!(labels.len(), MAX_LABELLED);
        assert!((0..labels.len()).all(|at| labels.get(at).unwrap().len() <= 4));
        assert_eq!(Labels::new(8, MAX_LABELLED * 2).len(), MAX_LABELLED);
    }

    proptest! {
        #[test]
        fn labels_are_prefix_free_and_never_shorter_later(keys in 2usize..=32, count in 0usize..600) {
            let labels = Labels::new(keys, count);
            prop_assert_eq!(labels.len(), count);
            let all: Vec<&[u8]> = (0..count).map(|at| labels.get(at).unwrap()).collect();
            for (at, label) in all.iter().enumerate() {
                prop_assert!(!label.is_empty());
                prop_assert!(label.iter().all(|&key| usize::from(key) < keys));
                if at > 0 {
                    prop_assert!(all[at - 1].len() <= label.len());
                }
            }
            let mut sorted = all.clone();
            sorted.sort();
            for pair in sorted.windows(2) {
                prop_assert!(!pair[1].starts_with(pair[0]), "{:?} starts {:?}", pair[0], pair[1]);
            }
        }
    }

    #[test]
    fn keys_are_checked() {
        assert_eq!(Keys::default().as_str(), "asdfghjl");
        assert_eq!(Keys::new("ab").unwrap().as_str(), "ab");
        assert_eq!(Keys::new("a"), Err(KeysError::Count(1)));
        assert_eq!(Keys::new(&"x".repeat(33)), Err(KeysError::Count(33)));
        assert_eq!(Keys::new("a b"), Err(KeysError::NotPrintable(' ')));
        assert_eq!(Keys::new("aあ"), Err(KeysError::NotPrintable('あ')));
        assert_eq!(Keys::new("aba"), Err(KeysError::Twice('a')));
        assert_eq!(Keys::new("asdK"), Err(KeysError::Action('K')));
        assert_eq!(Keys::new("asdc"), Err(KeysError::Action('c')));
        assert!(Keys::new("1234").is_ok(), "digits may be keys");
    }
}
