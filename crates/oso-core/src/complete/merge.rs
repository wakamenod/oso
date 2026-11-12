//! Candidates added to a list that is shown, as a project's files are while it is walked
//! (ADR 0114): each batch is put in the list's order and matched against the input on its own,
//! so the rest of the list is neither ordered nor matched again.
//!
//! Must not depend on: `editor`, threads or a log. Up to [`WORKER_PAST`](super::WORKER_PAST)
//! candidates the editor merges a batch on the UI thread; past that a job does on the worker.
//!
//! The order is ADR 0076's, as [`order_of`](super::job::order_of) makes it: the history first,
//! the newest first, then the shorter, then by text. A batch is sorted by it, and each of its
//! candidates goes in before the first of the list that comes after it, found by a binary
//! search. So a batch of `k` over a list of `n` costs `k log n` comparisons and one copy of the
//! list's indexes.

use std::collections::HashMap;
use std::sync::Arc;

use super::job::{Matched, STOP_EVERY, key, recent_of};
use super::{Candidate, Candidates, Query};

/// A list's order and the matches of its input, each a list of its items' indexes.
type Ordered = (Arc<[u32]>, Arc<[u32]>);

/// What a list held before it grew: its items, its order and its matches.
type Before = (Arc<[Candidate]>, Arc<[u32]>, Arc<[u32]>);

/// The order and the matches of `items` once those from `start` on are added to a list whose
/// first `start` items were in `order`, with `matches` the ones that matched `query`. Only the
/// added items are matched. A row shown only while nothing is typed (ADR 0091) is never one a
/// walk adds, so none is looked for. `None` if `stop` says so first.
pub(crate) fn merge_in<'a>(
    items: &[Candidate],
    start: usize,
    (order, matches): (&[u32], &[u32]),
    query: &Query,
    history: impl IntoIterator<Item = &'a str>,
    stop: &dyn Fn() -> bool,
) -> Option<Ordered> {
    let recent = recent_of(history);
    let mut added: Vec<u32> = (start..items.len()).map(super::offset_u32).collect();
    added.sort_by_cached_key(|&index| key(&recent, &items[index as usize]));
    if stop() {
        return None;
    }
    let matching: Vec<u32> = added
        .iter()
        .copied()
        .filter(|&index| query.matches(&items[index as usize]))
        .collect();
    let order = insert(items, &recent, order, &added, stop)?;
    let matches = insert(items, &recent, matches, &matching, stop)?;
    Some((order.into(), matches.into()))
}

/// `into`, in its order, with each of `added`, sorted in that order, put in its place: after
/// those of `into` that come before it or tie with it.
fn insert(
    items: &[Candidate],
    recent: &HashMap<&str, usize>,
    into: &[u32],
    added: &[u32],
    stop: &dyn Fn() -> bool,
) -> Option<Vec<u32>> {
    let mut out = Vec::with_capacity(into.len() + added.len());
    let mut from = 0;
    for (at, &index) in added.iter().enumerate() {
        if at % STOP_EVERY == STOP_EVERY - 1 && stop() {
            return None;
        }
        let new = key(recent, &items[index as usize]);
        let place =
            from + into[from..].partition_point(|&old| key(recent, &items[old as usize]) <= new);
        out.extend_from_slice(&into[from..place]);
        out.push(index);
        from = place;
    }
    out.extend_from_slice(&into[from..]);
    Some(out)
}

/// Where in `matches` an exact match of `input` is, which shows first.
pub(crate) fn exact_in(items: &[Candidate], matches: &[u32], input: Option<&str>) -> Option<usize> {
    let input = input?;
    matches
        .iter()
        .position(|&index| items[index as usize].text() == input)
}

impl Candidates {
    /// Every candidate's index, in the order they show.
    pub(crate) fn order(&self) -> &Arc<[u32]> {
        &self.order
    }

    /// What the input matched last came to, for a job that adds to the list.
    pub(crate) fn matched(&self) -> Matched {
        Matched {
            matches: Arc::clone(&self.matches),
            exact: self.exact,
            query: self.query.clone(),
            input: self.input.clone(),
        }
    }

    /// Adds `added` to the list where its order puts them, matching only them against the
    /// input matched last, as a project's files come in while it is walked (ADR 0114). The
    /// list is moved, not copied, when nothing else holds it. The selected row stays selected.
    pub(crate) fn merge<'a>(
        &mut self,
        added: impl IntoIterator<Item = Candidate>,
        history: impl IntoIterator<Item = &'a str>,
    ) {
        let start = self.items.len();
        let mut items: Vec<Candidate> = match Arc::get_mut(&mut self.items) {
            Some(items) => items.iter_mut().map(Candidate::take).collect(),
            None => self.items.iter().map(Candidate::duplicate).collect(),
        };
        items.extend(added);
        let items: Arc<[Candidate]> = items.into();
        let merged = merge_in(
            &items,
            start,
            (&self.order, &self.matches),
            &self.query,
            history,
            &|| false,
        );
        let kept = self.selected_item();
        self.items = items;
        if let Some((order, matches)) = merged {
            self.exact = exact_in(&self.items, &matches, self.input.as_deref());
            self.order = order;
            self.matches = matches;
        }
        self.reselect(kept);
    }

    /// Takes in a list that grew on a worker: `items` hold the ones before at the same
    /// indexes, then those added, in `order`, and `matched` is what an input came to over
    /// them. For the input matched last, the selected row stays selected; for another, a row
    /// moved to stays as [`Candidates::apply`] keeps it. Gives back the list before, for the
    /// caller to free.
    pub(crate) fn grown(
        &mut self,
        items: Arc<[Candidate]>,
        order: Arc<[u32]>,
        matched: Matched,
    ) -> Before {
        let kept = self.selected_item();
        let same = matched.input == self.input;
        let items = std::mem::replace(&mut self.items, items);
        let order = std::mem::replace(&mut self.order, order);
        let matches = Arc::clone(&self.matches);
        if same {
            self.matches = matched.matches;
            self.exact = matched.exact;
            self.reselect(kept);
        } else {
            self.apply(matched, true);
        }
        (items, order, matches)
    }

    /// Selects the item at `kept` again where it shows now, or the first if it is gone or was
    /// none, leaving whether one was moved to as it was. An exact match of the input that came
    /// shows first, and is selected unless the person moved to a row, as typing a whole name
    /// selects it (ADR 0076).
    fn reselect(&mut self, kept: Option<usize>) {
        if self.exact.is_some() && !self.chosen {
            self.selected = 0;
            return;
        }
        self.selected = kept.and_then(|index| self.place_of(index)).unwrap_or(0);
    }
}
