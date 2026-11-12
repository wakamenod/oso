//! Building a list, matching an input and adding a walk's paths to a list, on the UI thread up
//! to [`WORKER_PAST`] candidates and as a job for the worker past that (ADR 0096, ADR 0114):
//! what a job holds, running it, and the report it sends back.
//!
//! Must not depend on: `editor`, threads or a log. The host runs a job on its thread, and the
//! editor makes it and takes the report in. Neither a job nor a report says in `Debug` what
//! it lists or what is typed: file names can be private, and a prompt can be over a secret.
//!
//! A newer job supersedes the one before through a generation, which a job reads before it
//! starts, every [`STOP_EVERY`] candidates of each step, and every [`STOP_EVERY`] comparisons
//! of the sort, and stops once it is not the live one. A job that only frees what the prompt
//! no longer holds runs on the worker too, so that a list of 100,000 is never dropped on the UI
//! thread.

use std::cell::Cell;
use std::cmp::Ordering as Order;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::merge::{exact_in, merge_in};
use super::path::{self, Found, Name, Reached};
use super::{Candidate, MAX_INPUT_BYTES, Query, offset_u32};

/// The most candidates a list builds and matches on the UI thread; a longer one is a job for
/// the worker (ADR 0076, ADR 0096).
pub const WORKER_PAST: usize = 5000;

/// How many candidates a step goes through between two looks at whether to stop.
pub(crate) const STOP_EVERY: usize = 1024;

/// What matching an input came to: the candidates it matches, in their order, where an exact
/// match of it is, and the query and input it was.
#[derive(Clone, Default)]
pub(crate) struct Matched {
    pub(crate) matches: Arc<[u32]>,
    pub(crate) exact: Option<usize>,
    pub(crate) query: Query,
    /// The input matched, or `None` for one too long to match.
    pub(crate) input: Option<String>,
}

/// The part of `input` a list matches: what is typed after the last `/` when it matches a path's
/// last segment, or `None` for an input too long to match.
pub(crate) fn matched_part(input: &str, last_segment: bool) -> Option<&str> {
    let input = if last_segment {
        input.rfind('/').map_or(input, |slash| &input[slash + 1..])
    } else {
        input
    };
    (input.len() <= MAX_INPUT_BYTES).then_some(input)
}

/// Matches `input` over `items` in `order`, looking only at the matches of `before` where the
/// input narrows that one. `None` if `stop` says so first.
pub(crate) fn match_over(
    items: &[Candidate],
    order: &[u32],
    before: Option<(&[u32], &Query)>,
    input: Option<&str>,
    stop: &dyn Fn() -> bool,
) -> Option<Matched> {
    let Some(input) = input else {
        return Some(Matched {
            query: Query::too_long(),
            ..Matched::default()
        });
    };
    let empty = Query::default();
    let query = Query::parse_after(input, before.map_or(&empty, |(_, query)| query));
    // A candidate shown only while nothing is typed never matches an input (ADR 0091).
    let typed = !input.is_empty();
    let shows = |item: &Candidate| !(typed && item.hidden_by_input);
    let mut matches = Vec::new();
    match before {
        // Only what changed is checked again: the rest matched before.
        Some((matched, before)) if query.narrows(before) => {
            for (at, &index) in matched.iter().enumerate() {
                if at % STOP_EVERY == STOP_EVERY - 1 && stop() {
                    return None;
                }
                let item = &items[index as usize];
                if shows(item) && query.matches_after(item, before) {
                    matches.push(index);
                }
            }
        }
        _ => {
            for (at, &index) in order.iter().enumerate() {
                if at % STOP_EVERY == STOP_EVERY - 1 && stop() {
                    return None;
                }
                let item = &items[index as usize];
                if shows(item) && query.matches(item) {
                    matches.push(index);
                }
            }
        }
    }
    let exact = exact_in(items, &matches, Some(input));
    Some(Matched {
        matches: matches.into(),
        exact,
        query,
        input: Some(input.to_owned()),
    })
}

/// `items` in the order they show: first those in `history`, newest first, then the shorter,
/// then by their text (ADR 0076). `None` if `stop` says so first.
pub(crate) fn order_of<'a>(
    items: &[Candidate],
    history: impl IntoIterator<Item = &'a str>,
    stop: &dyn Fn() -> bool,
) -> Option<Arc<[u32]>> {
    let recent = recent_of(history);
    let mut keyed: Vec<(usize, usize, &str, u32)> = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if index % STOP_EVERY == STOP_EVERY - 1 && stop() {
            return None;
        }
        let (rank, length, text) = key(&recent, item);
        keyed.push((rank, length, text, offset_u32(index)));
    }
    // A sort cannot be left midway, so once a newer job has come its comparisons end at once.
    let (compared, stopped) = (Cell::new(0_usize), Cell::new(false));
    keyed.sort_by(|a, b| {
        if stopped.get() {
            return Order::Equal;
        }
        compared.set(compared.get() + 1);
        if compared.get().is_multiple_of(STOP_EVERY) && stop() {
            stopped.set(true);
            return Order::Equal;
        }
        (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2))
    });
    if stopped.get() {
        return None;
    }
    Some(keyed.into_iter().map(|(_, _, _, index)| index).collect())
}

/// What orders a candidate among others (ADR 0076): its place in the history, its length in
/// characters, its text.
pub(crate) type Key<'a> = (usize, usize, &'a str);

/// The key of `item`, with `recent` from [`recent_of`].
pub(crate) fn key<'a>(recent: &HashMap<&str, usize>, item: &'a Candidate) -> Key<'a> {
    let text = item.text();
    (
        recent.get(text).copied().unwrap_or(usize::MAX),
        text.chars().count(),
        text,
    )
}

/// Where each answer of `history`, newest first, stands in it: the newest of an answer given
/// twice decides.
pub(crate) fn recent_of<'a>(history: impl IntoIterator<Item = &'a str>) -> HashMap<&'a str, usize> {
    let mut recent: HashMap<&str, usize> = HashMap::new();
    for (place, text) in history.into_iter().enumerate() {
        recent.entry(text).or_insert(place);
    }
    recent
}

/// The list a prompt that reads a path is to have: the directories its segments reached, each
/// with its listing, and what to order and match it by.
pub(crate) struct Build {
    pub(crate) reached: Vec<(Reached, Arc<[Name]>)>,
    /// Names starting with `.` are kept.
    pub(crate) hidden: bool,
    /// The most names kept.
    pub(crate) most: usize,
    /// The path of the row selected, which stays selected if the new list has it.
    pub(crate) keep: Option<PathBuf>,
    /// The prompt's history, newest first, which orders the list.
    pub(crate) history: Vec<String>,
    /// The part of what is typed that the list matches, or `None` for one too long.
    pub(crate) input: Option<String>,
}

/// A list built: the candidates, the path each stands for, their order, and the input matched.
pub(crate) struct Built {
    pub(crate) found: Vec<Found>,
    /// More names were found than are kept.
    pub(crate) more: bool,
    /// Where in `found` the path to keep selected is.
    pub(crate) kept: Option<usize>,
    pub(crate) items: Arc<[Candidate]>,
    pub(crate) order: Arc<[u32]>,
    pub(crate) matched: Matched,
}

impl Build {
    /// How many names the listings reached hold, before any is left out.
    pub(crate) fn size(&self) -> usize {
        self.reached.iter().map(|(_, names)| names.len()).sum()
    }

    /// Builds the list and matches the input. `None` if `stop` says so first.
    pub(crate) fn run(&self, stop: &dyn Fn() -> bool) -> Option<Built> {
        let listed = self
            .reached
            .iter()
            .map(|((directory, shown), names)| (directory.as_path(), shown.as_str(), &names[..]));
        // One past the most, to tell some were left out, and never a name more (ADR 0096).
        let mut found = path::found_in(listed, self.hidden, self.most + 1, stop)?;
        let more = found.len() > self.most;
        found.truncate(self.most);
        // What a look found of a row is kept beside the list and patches it (ADR 0096).
        let mut items = Vec::with_capacity(found.len());
        for (index, found) in found.iter().enumerate() {
            if index % STOP_EVERY == STOP_EVERY - 1 && stop() {
                return None;
            }
            items.push(Candidate::new(
                found.name.as_str(),
                "",
                found.context.as_str(),
            ));
        }
        let items: Arc<[Candidate]> = items.into();
        let order = order_of(&items, self.history.iter().map(String::as_str), stop)?;
        let matched = match_over(&items, &order, None, self.input.as_deref(), stop)?;
        // Compared as bytes: a path's own comparison reads its components.
        let kept = self.keep.as_ref().and_then(|keep| {
            found
                .iter()
                .position(|found| found.path.as_os_str() == keep.as_os_str())
        });
        Some(Built {
            found,
            more,
            kept,
            items,
            order,
            matched,
        })
    }
}

/// An input to match over a list, from what was matched last where the input narrows that.
pub(crate) struct Matching {
    pub(crate) items: Arc<[Candidate]>,
    pub(crate) order: Arc<[u32]>,
    pub(crate) before: Option<(Arc<[u32]>, Query)>,
    pub(crate) input: Option<String>,
}

/// Paths a walk found, to add to a list past [`WORKER_PAST`], and the input to match then
/// (ADR 0114).
pub(crate) struct Merge {
    pub(crate) items: Arc<[Candidate]>,
    pub(crate) order: Arc<[u32]>,
    /// What the list's input matched last came to.
    pub(crate) matched: Matched,
    /// The batches of paths to add, the oldest first.
    pub(crate) batches: Vec<Arc<[Box<str>]>>,
    /// The prompt's history, newest first, which orders the list, shared with every job.
    pub(crate) history: Arc<[String]>,
    /// The part of what is typed that the list matches now, or `None` for one too long.
    pub(crate) input: Option<String>,
}

impl Merge {
    /// The list with the paths added. Only they are matched, against the input now; when that
    /// is not the input matched last, the list before is matched again first, narrowing what
    /// it matched where the input narrows that one. `None` if `stop` says so first.
    fn run(&self, stop: &dyn Fn() -> bool) -> Option<Merged> {
        let start = self.items.len();
        let added: usize = self.batches.iter().map(|batch| batch.len()).sum();
        let mut items = Vec::with_capacity(start + added);
        for (index, item) in self.items.iter().enumerate() {
            if index % STOP_EVERY == STOP_EVERY - 1 && stop() {
                return None;
            }
            items.push(item.duplicate());
        }
        for (index, path) in self
            .batches
            .iter()
            .flat_map(|batch| batch.iter())
            .enumerate()
        {
            if index % STOP_EVERY == STOP_EVERY - 1 && stop() {
                return None;
            }
            items.push(Candidate::new(&**path, "", ""));
        }
        let items: Arc<[Candidate]> = items.into();
        // The list before, over the input now: its indexes are the same in the list grown.
        let before = if self.input == self.matched.input {
            self.matched.clone()
        } else {
            let matched = (&self.matched.matches[..], &self.matched.query);
            match_over(
                &items,
                &self.order,
                Some(matched),
                self.input.as_deref(),
                stop,
            )?
        };
        let history = self.history.iter().map(String::as_str);
        let (order, matches) = merge_in(
            &items,
            start,
            (&self.order, &before.matches),
            &before.query,
            history,
            stop,
        )?;
        let matched = Matched {
            exact: exact_in(&items, &matches, self.input.as_deref()),
            matches,
            query: before.query,
            input: self.input.clone(),
        };
        Some(Merged {
            base: Arc::clone(&self.items),
            items,
            order,
            matched,
            carried: self.batches.len(),
        })
    }
}

/// A list a walk added to: the list it was added to, to tell it is still the one shown, the
/// list now, its order, what the input came to, and how many batches it took in.
pub(crate) struct Merged {
    pub(crate) base: Arc<[Candidate]>,
    pub(crate) items: Arc<[Candidate]>,
    pub(crate) order: Arc<[u32]>,
    pub(crate) matched: Matched,
    pub(crate) carried: usize,
}

/// What a job does.
enum Work {
    Build(Box<Build>),
    Match(Box<Matching>),
    Merge(Box<Merge>),
    /// What the prompt no longer holds, to be dropped off the UI thread.
    Free(#[expect(dead_code, reason = "held only to be dropped where the job runs")] Box<dyn Send>),
}

/// A list to build or an input to match, for the host to run on the worker's thread
/// (ADR 0096). `Debug` gives its generation only.
pub struct CompletionJob {
    generation: u64,
    live: Arc<AtomicU64>,
    work: Work,
}

impl fmt::Debug for CompletionJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompletionJob")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl CompletionJob {
    pub(crate) fn build(build: Build, generation: u64, live: Arc<AtomicU64>) -> Self {
        Self {
            generation,
            live,
            work: Work::Build(Box::new(build)),
        }
    }

    pub(crate) fn matching(matching: Matching, generation: u64, live: Arc<AtomicU64>) -> Self {
        Self {
            generation,
            live,
            work: Work::Match(Box::new(matching)),
        }
    }

    pub(crate) fn merge(merge: Merge, generation: u64, live: Arc<AtomicU64>) -> Self {
        Self {
            generation,
            live,
            work: Work::Merge(Box::new(merge)),
        }
    }

    /// Whether it only frees, so that a newer job leaves it to run.
    pub(crate) fn frees_only(&self) -> bool {
        matches!(self.work, Work::Free(_))
    }

    /// A job that drops `garbage` where it runs, and reports nothing.
    pub(crate) fn free(garbage: Box<dyn Send>) -> Self {
        Self {
            generation: 0,
            live: Arc::default(),
            work: Work::Free(garbage),
        }
    }

    /// Runs the job, on the worker's thread. `None` once a newer job has taken its place,
    /// which it notices within [`STOP_EVERY`] candidates, and for a job that only frees.
    pub fn run(self) -> Option<CompletionReport> {
        let (generation, live) = (self.generation, &self.live);
        let stop = || live.load(Ordering::Relaxed) != generation;
        if matches!(self.work, Work::Free(_)) || stop() {
            return None;
        }
        let report = match &self.work {
            Work::Free(_) => return None,
            Work::Build(build) => Report::Built {
                generation,
                built: Box::new(build.run(&stop)?),
            },
            Work::Match(matching) => {
                let before = matching
                    .before
                    .as_ref()
                    .map(|(matches, query)| (&matches[..], query));
                let matched = match_over(
                    &matching.items,
                    &matching.order,
                    before,
                    matching.input.as_deref(),
                    &stop,
                )?;
                Report::Matched {
                    generation,
                    items: Arc::clone(&matching.items),
                    matched,
                }
            }
            Work::Merge(merge) => Report::Merged {
                generation,
                merged: Box::new(merge.run(&stop)?),
            },
        };
        // Superseded at the end: the newer job's report is the one to send.
        (!stop()).then_some(CompletionReport(report))
    }
}

/// What a job came to, for the editor to take in: the host only hands it back. `Debug` gives
/// its generation only.
pub struct CompletionReport(pub(crate) Report);

impl fmt::Debug for CompletionReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompletionReport")
            .field("generation", &self.0.generation())
            .finish_non_exhaustive()
    }
}

/// What a [`CompletionReport`] says.
pub(crate) enum Report {
    Built {
        generation: u64,
        built: Box<Built>,
    },
    /// The list it matched over, to tell it is still the one shown.
    Matched {
        generation: u64,
        items: Arc<[Candidate]>,
        matched: Matched,
    },
    Merged {
        generation: u64,
        merged: Box<Merged>,
    },
}

impl Report {
    pub(crate) fn generation(&self) -> u64 {
        match self {
            Self::Built { generation, .. }
            | Self::Matched { generation, .. }
            | Self::Merged { generation, .. } => *generation,
        }
    }
}

/// The directories a build reached, as they and their listings are kept by the prompt.
pub(crate) fn listed(
    reached: Vec<Reached>,
    listing: impl Fn(&std::path::Path) -> Option<Arc<[Name]>>,
) -> Vec<(Reached, Arc<[Name]>)> {
    reached
        .into_iter()
        .map(|reached| {
            let names = listing(&reached.0).unwrap_or_else(|| Arc::from([]));
            (reached, names)
        })
        .collect()
}
