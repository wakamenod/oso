//! Completion: matching an input against candidates as orderless does, and keeping the
//! candidates that match in an order fixed when the list is made, into which a list that grows,
//! as a project's files do while it is walked, puts each one it gains (ADR 0076, ADR 0114).
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
use std::sync::Arc;

use crate::migemo;
use folded::Folded;

pub(crate) use job::{
    Build, Built, Matched, Matching, Merge, Report, WORKER_PAST, listed, matched_part,
};
pub use job::{CompletionJob, CompletionReport};
pub use path::{Name, NameKind, typed_form};
pub(crate) use query::MAX_INPUT_BYTES;
pub use query::Query;

use job::{match_over, order_of};

mod folded;
mod job;
mod merge;
pub(crate) mod path;
mod query;

/// The most candidate rows a prompt shows, until configuration sets it (ADR 0026).
pub const ROWS: usize = 18;

/// The characters that pick a component's style, at its start or its end, as orderless's
/// `orderless-affix-dispatch-alist`: `!` not, `,` initialism, `=` literal, `^` literal
/// prefix, `~` flex, `&` the annotation, and `%` migemo's pattern (ADR 0116).
pub(crate) const AFFIXES: [char; 7] = ['!', ',', '=', '^', '~', '&', '%'];

/// One thing a prompt may complete to.
pub struct Candidate {
    text: Box<str>,
    binding: Box<str>,
    annotation: Box<str>,
    folded: Folded,
    folded_annotation: Folded,
    /// Its text with each kana and combining sound mark composed, for a migemo component,
    /// only where it holds such a mark, as names of files on macOS may (ADR 0116).
    composed: Option<Box<Folded>>,
    /// It shows only while nothing is typed, as a secret kill does (ADR 0091).
    hidden_by_input: bool,
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
            composed: Folded::composed(&text).map(Box::new),
            text,
            binding: binding.into(),
            annotation,
            hidden_by_input: false,
        }
    }

    /// A candidate that reads `text`, with `annotation` at the end of its row, which no input is
    /// ever matched against, as a project's search's rows are not (ADR 0114): its case is not
    /// folded, which would cost each row as it comes.
    pub(crate) fn unmatched(text: impl Into<Box<str>>, annotation: impl Into<Box<str>>) -> Self {
        let unfolded = || Folded {
            text: None,
            from: None,
        };
        Self {
            text: text.into(),
            binding: "".into(),
            annotation: annotation.into(),
            folded: unfolded(),
            folded_annotation: unfolded(),
            composed: None,
            hidden_by_input: false,
        }
    }

    /// The same, shown only while nothing is typed: any input hides it, whatever it asks,
    /// so that what it stands for is never matched against (ADR 0091).
    pub fn hidden_by_input(self) -> Self {
        Self {
            hidden_by_input: true,
            ..self
        }
    }

    /// A copy of it, folding nothing again, for a list that grows while another holds this one
    /// (ADR 0114).
    pub(crate) fn duplicate(&self) -> Self {
        Self {
            text: self.text.clone(),
            binding: self.binding.clone(),
            annotation: self.annotation.clone(),
            folded: self.folded.clone(),
            folded_annotation: self.folded_annotation.clone(),
            composed: self.composed.clone(),
            hidden_by_input: self.hidden_by_input,
        }
    }

    /// Moves it out, leaving an empty candidate, which allocates nothing, in its place.
    pub(crate) fn take(&mut self) -> Self {
        std::mem::replace(self, Self::new("", "", ""))
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

/// An offset as kept in [`Folded::from`]; a candidate is never 4 GiB long.
fn offset_u32(offset: usize) -> u32 {
    u32::try_from(offset).unwrap_or(u32::MAX)
}

/// The candidates of a prompt, the ones its input matches, and which of them is selected.
pub struct Candidates {
    items: Arc<[Candidate]>,
    /// Every candidate, by its index in `items`, in the order they show.
    order: Arc<[u32]>,
    /// Rows a look found more of since the list was made, by their index in `items`: they
    /// show in place of those items, and match as they did (ADR 0096).
    patched: HashMap<u32, Candidate>,
    /// A worker is matching a newer input or building a newer list: the rows shown stay, and
    /// the count says so (ADR 0076, ADR 0096).
    pending: bool,
    /// The candidates the input matches, in that order, shared with a worker's job.
    matches: Arc<[u32]>,
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
    /// The candidates keep the order given, not one from a history (ADR 0088).
    given_order: bool,
    /// Only what is typed after the last `/` is matched, as a path's last segment (ADR 0087).
    last_segment: bool,
}

impl Candidates {
    /// `items` in the order they show, every one matching: first those in `history`, newest
    /// first, then the shorter, then by their text.
    pub(crate) fn new<'a>(
        items: Arc<[Candidate]>,
        history: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let order = order_of(&items, history, &|| false);
        let order = order.unwrap_or_else(|| (0..items.len()).map(offset_u32).collect());
        Self::with_order(items, order)
    }

    /// `items` in the order given, every one matching, as buffers show most recent first
    /// (ADR 0088).
    pub(crate) fn in_given_order(items: Arc<[Candidate]>) -> Self {
        let order: Arc<[u32]> = (0..items.len()).map(offset_u32).collect();
        let mut candidates = Self::with_order(items, order);
        candidates.given_order = true;
        candidates
    }

    /// Whether the candidates keep the order given, as buffers do, most recent first, so the
    /// first is a choice of its own and not one a history made.
    pub fn in_order_given(&self) -> bool {
        self.given_order
    }

    fn with_order(items: Arc<[Candidate]>, order: Arc<[u32]>) -> Self {
        Self {
            matches: Arc::clone(&order),
            order,
            items,
            patched: HashMap::new(),
            pending: false,
            exact: None,
            query: Query::default(),
            input: Some(String::new()),
            selected: 0,
            top: 0,
            chosen: false,
            takes_input: false,
            on_input: false,
            previews: false,
            given_order: false,
            last_segment: false,
        }
    }

    /// Matches only what is typed after the last `/`.
    pub(crate) fn match_last_segment(&mut self) {
        self.last_segment = true;
    }

    /// The same candidates' settings over a list built anew, with the input it matched.
    pub(crate) fn rebuilt(
        &self,
        items: Arc<[Candidate]>,
        order: Arc<[u32]>,
        matched: Matched,
    ) -> Self {
        let mut next = Self::with_order(items, order);
        next.takes_input = self.takes_input;
        next.previews = self.previews;
        next.last_segment = self.last_segment;
        next.given_order = self.given_order;
        next.apply(matched, false);
        next
    }

    /// The candidates, matching or not, in the order of their indexes.
    pub(crate) fn items(&self) -> &Arc<[Candidate]> {
        &self.items
    }

    /// Whether a worker is matching a newer input or building a newer list, so that the rows
    /// shown are those of an older one.
    pub fn pending(&self) -> bool {
        self.pending
    }

    pub(crate) fn set_pending(&mut self, pending: bool) {
        self.pending = pending;
    }

    /// Shows `candidate` in place of the item at `index`, as a look found more of its row. What
    /// matches and the order stay.
    pub(crate) fn patch(&mut self, index: usize, candidate: Candidate) {
        self.patched.insert(offset_u32(index), candidate);
    }

    /// Whether the item at `index` shows a patch.
    pub(crate) fn is_patched(&self, index: usize) -> bool {
        self.patched.contains_key(&offset_u32(index))
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
    /// selects the first match, with no dictionary. Nothing changes for the input matched
    /// last.
    #[cfg(test)]
    pub(crate) fn set_input(&mut self, input: &str) {
        self.set_input_with(input, None);
    }

    /// [`Candidates::set_input`], a migemo component taking `table`'s words (ADR 0116).
    pub(crate) fn set_input_with(&mut self, input: &str, table: Option<&migemo::Table>) {
        let Some(part) = matched_part(input, self.last_segment) else {
            self.set_too_long();
            return;
        };
        if self.input.as_deref() == Some(part) {
            return;
        }
        let before = (&self.matches[..], &self.query);
        if let Some(matched) = match_over(
            &self.items,
            &self.order,
            Some(before),
            (Some(part), table),
            &|| false,
        ) {
            self.apply(matched, false);
        }
    }

    /// What a worker needs to match `input`, unless it is the input matched last, with no
    /// dictionary.
    #[cfg(test)]
    pub(crate) fn matching(&self, input: &str) -> Option<Matching> {
        self.matching_with(input, None)
    }

    /// [`Candidates::matching`], a migemo component taking `table`'s words (ADR 0116).
    pub(crate) fn matching_with(
        &self,
        input: &str,
        table: Option<&migemo::Table>,
    ) -> Option<Matching> {
        let part = matched_part(input, self.last_segment);
        if part.is_some() && self.input.as_deref() == part {
            return None;
        }
        Some(Matching {
            items: Arc::clone(&self.items),
            order: Arc::clone(&self.order),
            before: Some((Arc::clone(&self.matches), self.query.clone())),
            input: part.map(str::to_owned),
            table: table.cloned(),
        })
    }

    /// Takes in what matching an input came to, and selects the first match. With `keep`, a
    /// candidate moved to while the match was on its way stays selected if it still matches,
    /// as it was chosen after the input was typed (ADR 0096).
    pub(crate) fn apply(&mut self, matched: Matched, keep: bool) {
        let kept = (keep && self.chosen && !self.on_input)
            .then(|| self.item_at(self.selected))
            .flatten();
        self.matches = matched.matches;
        self.exact = matched.exact;
        self.query = matched.query;
        self.input = matched.input;
        self.selected = 0;
        self.top = 0;
        self.chosen = false;
        self.on_input = false;
        if let Some(place) = kept.and_then(|index| self.place_of(index)) {
            self.select(place);
        }
    }

    /// Whether the input matched last must be matched anew, as a migemo component took its
    /// words from another dictionary than that of `generation` (ADR 0116): it is then
    /// forgotten, and what it matched stays until the next match.
    pub(crate) fn stale_for(&mut self, generation: Option<u64>) -> bool {
        let stale = self.query.compiled_for_another(generation);
        if stale {
            self.input = None;
        }
        stale
    }

    /// Forgets that a candidate was moved to, as an input was typed after it.
    pub(crate) fn forget_choice(&mut self) {
        self.chosen = false;
    }

    /// Matches an input longer than [`MAX_INPUT_BYTES`], which nothing matches, without
    /// reading it.
    pub(crate) fn set_too_long(&mut self) {
        self.matches = Arc::from([]);
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
        let index = self.item_at(place)?;
        match self.patched.get(&offset_u32(index)) {
            Some(patched) => Some(patched),
            None => self.items.get(index),
        }
    }

    /// The index in the items given of the match shown at `place`, for a caller that keeps
    /// more of each item beside it.
    pub fn item_at(&self, place: usize) -> Option<usize> {
        let at = match self.exact {
            Some(exact) if place == 0 => exact,
            Some(exact) if place <= exact => place - 1,
            _ => place,
        };
        Some(*self.matches.get(at)? as usize)
    }

    /// The place where the item at `index` shows, if it matches.
    pub(crate) fn place_of(&self, index: usize) -> Option<usize> {
        let index = u32::try_from(index).ok()?;
        let at = self.matches.iter().position(|&other| other == index)?;
        // The inverse of `item_at`: an exact match shows first, and those before it one later.
        Some(match self.exact {
            Some(exact) if at == exact => 0,
            Some(exact) if at < exact => at + 1,
            _ => at,
        })
    }

    /// Where the selected candidate shows, while any matches and what is typed is not selected.
    pub fn selected(&self) -> Option<usize> {
        (!self.matches.is_empty() && !self.on_input).then_some(self.selected)
    }

    /// The selected candidate, while any matches.
    pub fn selected_candidate(&self) -> Option<&Candidate> {
        self.get(self.selected()?)
    }

    /// Where the selected candidate is in the items given, while any matches, for a list
    /// whose rows may read alike.
    pub(crate) fn selected_item(&self) -> Option<usize> {
        self.item_at(self.selected()?)
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
mod migemo_tests;
#[cfg(test)]
mod tests;
