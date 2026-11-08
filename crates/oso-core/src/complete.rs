//! Completion: matching an input against candidates as orderless does, and keeping the
//! candidates that match in an order fixed when the list is made (ADR 0076).
//!
//! Must not depend on: `editor`, `minibuffer` or `display_map`; it works on strings alone,
//! so that a worker thread can run it on a snapshot of the list.
//!
//! An input is split at spaces into components, and a candidate matches when every
//! component matches it, in any order. A component matches as a literal substring or, if
//! it holds a character that means something in one, as a regular expression, or in the
//! style an affix at its start or end picks ([`AFFIXES`]). Expressions are the `regex`
//! crate's, which match in linear time, compiled with limits, and one that does not
//! compile matches as a literal. Case is ignored unless the
//! input holds an upper-case letter. The order is vertico's: the prompt's history first,
//! the most recent first, then shorter candidates, then alphabetical; filtering keeps it,
//! so typing never moves a candidate, and an exact match of the input shows first.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

pub(crate) use query::MAX_INPUT_BYTES;
pub use query::Query;

mod query;

/// The most candidate rows a prompt shows, until configuration sets it (ADR 0026).
pub const ROWS: usize = 18;

/// The characters that pick a component's style, at its start or its end, as orderless's
/// `orderless-affix-dispatch-alist`: `!` not, `,` initialism, `=` literal, `^` literal
/// prefix, `~` flex, `&` the annotation.
pub(crate) const AFFIXES: [char; 6] = ['!', ',', '=', '^', '~', '&'];

/// One thing a prompt may complete to.
pub struct Candidate {
    text: Box<str>,
    binding: Box<str>,
    annotation: Box<str>,
    folded: Folded,
    folded_annotation: Folded,
}

impl Candidate {
    /// A candidate that reads `text`, with `binding` after it and `annotation` at the end of
    /// its row, either of them empty. Folding its case is done here, once.
    pub fn new(
        text: impl Into<Box<str>>,
        binding: impl Into<Box<str>>,
        annotation: impl Into<Box<str>>,
    ) -> Self {
        let (text, annotation) = (text.into(), annotation.into());
        Self {
            folded: Folded::of(&text),
            folded_annotation: Folded::of(&annotation),
            text,
            binding: binding.into(),
            annotation,
        }
    }

    /// What it completes to.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// A key sequence shown after it, such as a command's binding. May be empty.
    pub fn binding(&self) -> &str {
        &self.binding
    }

    /// What is said of it at the end of its row, such as a docstring's first line. May be
    /// empty.
    pub fn annotation(&self) -> &str {
        &self.annotation
    }
}

impl fmt::Debug for Candidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Candidate")
            .field("text", &self.text)
            .field("binding", &self.binding)
            .finish_non_exhaustive()
    }
}

/// A string in lower case, for matching that ignores case, and where its bytes came from.
struct Folded {
    /// The folded string, or `None` where folding changes nothing.
    text: Option<Box<str>>,
    /// For each byte of `text` and its end, the offset in the original of the character it
    /// came from: `None` where every character folds to one of the same length, so that
    /// offsets agree.
    from: Option<Box<[u32]>>,
}

impl Folded {
    fn of(text: &str) -> Self {
        if !text.chars().any(changes_when_folded) {
            return Self {
                text: None,
                from: None,
            };
        }
        let mut folded = String::with_capacity(text.len());
        let mut from = Vec::with_capacity(text.len() + 1);
        let mut same = true;
        for (offset, c) in text.char_indices() {
            let start = folded.len();
            folded.extend(c.to_lowercase());
            same &= folded.len() - start == c.len_utf8();
            from.resize(folded.len(), offset_u32(offset));
        }
        from.push(offset_u32(text.len()));
        Self {
            text: Some(folded.into()),
            from: (!same).then(|| from.into()),
        }
    }

    /// The range of the original that `range` of the folded string covers, widened to whole
    /// characters.
    fn original(&self, range: Range<usize>) -> Range<usize> {
        let Some(from) = &self.from else {
            return range;
        };
        // Read with `get`: matching runs after a command's panic boundary, so a range it was
        // never given must give an empty range, not a panic.
        let start = from.get(range.start).map_or(0, |&offset| offset as usize);
        // The end is where the next original character starts, past the one `range` ends in.
        let last = from
            .get(range.end.saturating_sub(1).max(range.start))
            .copied()
            .unwrap_or(0);
        let end = from
            .get(range.end..)
            .and_then(|rest| rest.iter().find(|&&offset| offset > last))
            .map_or(start, |&offset| offset as usize);
        start..end.max(start)
    }
}

/// An offset as kept in [`Folded::from`]; a candidate is never 4 GiB long.
fn offset_u32(offset: usize) -> u32 {
    u32::try_from(offset).unwrap_or(u32::MAX)
}

/// Whether ignoring case reads `c` as something else.
fn changes_when_folded(c: char) -> bool {
    let mut lower = c.to_lowercase();
    lower.next() != Some(c) || lower.next().is_some()
}

/// The candidates of a prompt, the ones its input matches, and which of them is selected.
pub struct Candidates {
    items: Arc<[Candidate]>,
    /// Every candidate, by its index in `items`, in the order they show.
    order: Box<[u32]>,
    /// The candidates the input matches, in that order.
    matches: Vec<u32>,
    /// Where in `matches` an exact match of the input is, which shows first.
    exact: Option<usize>,
    query: Query,
    /// The input matched last, or `None` for one too long to match.
    input: Option<String>,
    /// The selected candidate, as it shows, while any matches.
    selected: usize,
    /// The first row shown.
    top: usize,
    /// A candidate was moved to since the input last changed.
    chosen: bool,
    /// The prompt takes any text, not only a candidate's: `C-p` from the first candidate
    /// selects what is typed, and `RET` with no match gives it (ADR 0087, ADR 0088).
    takes_input: bool,
    /// What is typed is selected, not a candidate.
    on_input: bool,
    /// The float shows the selected candidate beside the list (ADR 0088).
    previews: bool,
}

impl Candidates {
    /// `items` in the order they show, every one matching: first those in `history`, newest
    /// first, then the shorter, then by their text.
    pub(crate) fn new<'a>(
        items: Arc<[Candidate]>,
        history: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let mut recent: HashMap<&str, usize> = HashMap::new();
        for (place, text) in history.into_iter().enumerate() {
            // The newest of an answer given twice decides.
            recent.entry(text).or_insert(place);
        }
        let mut keyed: Vec<(usize, usize, u32)> = (0..items.len())
            .map(|index| {
                let text = items[index].text();
                (
                    recent.get(text).copied().unwrap_or(usize::MAX),
                    text.chars().count(),
                    offset_u32(index),
                )
            })
            .collect();
        keyed.sort_by(|a, b| {
            (a.0, a.1)
                .cmp(&(b.0, b.1))
                .then_with(|| items[a.2 as usize].text().cmp(items[b.2 as usize].text()))
        });
        let order: Box<[u32]> = keyed.into_iter().map(|(_, _, index)| index).collect();
        Self::with_order(items, order)
    }

    /// `items` in the order given, every one matching, as buffers show most recent first
    /// (ADR 0088).
    pub(crate) fn in_given_order(items: Arc<[Candidate]>) -> Self {
        let order: Box<[u32]> = (0..items.len()).map(offset_u32).collect();
        Self::with_order(items, order)
    }

    fn with_order(items: Arc<[Candidate]>, order: Box<[u32]>) -> Self {
        Self {
            matches: order.to_vec(),
            order,
            items,
            exact: None,
            query: Query::default(),
            input: Some(String::new()),
            selected: 0,
            top: 0,
            chosen: false,
            takes_input: false,
            on_input: false,
            previews: false,
        }
    }

    /// Shows the selected candidate beside the list.
    pub(crate) fn show_preview(&mut self) {
        self.previews = true;
    }

    /// Whether the float shows the selected candidate beside the list.
    pub fn previews(&self) -> bool {
        self.previews
    }

    /// Lets what is typed be selected and given, as a prompt for a file or a buffer that need
    /// not exist does.
    pub(crate) fn take_input(&mut self) {
        self.takes_input = true;
    }

    /// Whether the prompt takes any text, not only a candidate's.
    pub fn takes_input(&self) -> bool {
        self.takes_input
    }

    /// Whether what is typed is selected, not a candidate.
    pub fn on_input(&self) -> bool {
        self.on_input
    }

    /// Matches `input`, looking only at what matched before where it narrows that, and
    /// selects the first match. Nothing changes for the input matched last.
    pub(crate) fn set_input(&mut self, input: &str) {
        if input.len() > MAX_INPUT_BYTES {
            self.set_too_long();
            return;
        }
        if self.input.as_deref() == Some(input) {
            return;
        }
        let query = Query::parse_after(input, &self.query);
        let items = &self.items;
        if query.narrows(&self.query) {
            // Only what changed is checked again: the rest matched before.
            let before = &self.query;
            self.matches
                .retain(|&index| query.matches_after(&items[index as usize], before));
        } else {
            self.matches.clear();
            self.matches.extend(
                self.order
                    .iter()
                    .copied()
                    .filter(|&index| query.matches(&items[index as usize])),
            );
        }
        self.exact = self
            .matches
            .iter()
            .position(|&index| items[index as usize].text() == input);
        self.query = query;
        match &mut self.input {
            Some(kept) => input.clone_into(kept),
            None => self.input = Some(input.to_owned()),
        }
        self.selected = 0;
        self.top = 0;
        self.chosen = false;
        self.on_input = false;
    }

    /// Matches an input longer than [`MAX_INPUT_BYTES`], which nothing matches, without
    /// reading it.
    pub(crate) fn set_too_long(&mut self) {
        self.matches.clear();
        self.exact = None;
        self.query = Query::too_long();
        self.input = None;
        self.selected = 0;
        self.top = 0;
        self.chosen = false;
        self.on_input = false;
    }

    /// Whether the selected candidate is only the first of an empty input's list: nothing was
    /// typed, and no candidate moved to.
    pub(crate) fn defaulted(&self) -> bool {
        self.input.as_deref() == Some("") && !self.chosen
    }

    /// The query matched last, for highlighting what matched.
    pub fn query(&self) -> &Query {
        &self.query
    }

    /// How many candidates the input matches.
    pub fn len(&self) -> usize {
        self.matches.len()
    }

    /// Whether the input matches none.
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// How many candidates there are, matching or not.
    pub fn total(&self) -> usize {
        self.items.len()
    }

    /// The match shown at `place`, from 0: an exact match of the input first, then the rest
    /// in their order.
    pub fn get(&self, place: usize) -> Option<&Candidate> {
        let at = match self.exact {
            Some(exact) if place == 0 => exact,
            Some(exact) if place <= exact => place - 1,
            _ => place,
        };
        let index = *self.matches.get(at)?;
        self.items.get(index as usize)
    }

    /// Where the selected candidate shows, while any matches and what is typed is not selected.
    pub fn selected(&self) -> Option<usize> {
        (!self.matches.is_empty() && !self.on_input).then_some(self.selected)
    }

    /// The selected candidate, while any matches.
    pub fn selected_candidate(&self) -> Option<&Candidate> {
        self.get(self.selected()?)
    }

    /// Selects the match at `place`, or the last if there are fewer.
    pub(crate) fn select(&mut self, place: usize) {
        self.chosen = true;
        self.on_input = false;
        self.selected = place.min(self.matches.len().saturating_sub(1));
    }

    /// Selects the match `by` places after the selected one, or before for a negative
    /// `by`, stopping at the first and the last, as vertico does with `vertico-cycle` off. In a
    /// prompt that takes any text, going back from the first selects what is typed, and going on
    /// from there selects the first again.
    pub(crate) fn move_by(&mut self, by: isize) {
        if self.on_input {
            if by > 0 {
                self.select(usize::try_from(by - 1).unwrap_or(usize::MAX));
            }
            return;
        }
        if self.takes_input && by < 0 && self.selected.checked_add_signed(by).is_none() {
            self.chosen = true;
            self.on_input = true;
            return;
        }
        let place = self.selected.saturating_add_signed(by);
        self.select(place);
    }

    /// The first row to show of `rows` so that the selected one is in view, moving the rows
    /// shown no more than it must. The rows shown are `top..top + rows`.
    pub(crate) fn keep_in_view(&mut self, rows: usize) -> usize {
        if rows == 0 {
            return self.top;
        }
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + rows {
            self.top = self.selected + 1 - rows;
        }
        // Rows past the last match show the ones before them instead.
        self.top = self.top.min(self.matches.len().saturating_sub(rows));
        self.top
    }

    /// The matches in the `rows` rows shown, from the first, with where each shows.
    pub fn shown(&self, rows: usize) -> impl Iterator<Item = (usize, &Candidate)> {
        (self.top..self.len())
            .take(rows)
            .filter_map(|place| Some((place, self.get(place)?)))
    }

    /// The first row shown, as [`Candidates::keep_in_view`] left it.
    pub fn top(&self) -> usize {
        self.top
    }
}

impl fmt::Debug for Candidates {
    /// The counts, and never the input, which may be about a secret buffer.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Candidates")
            .field("total", &self.total())
            .field("matches", &self.len())
            .field("selected", &self.selected())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
