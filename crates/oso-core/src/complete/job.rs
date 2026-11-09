//! Building a list and matching an input, on the UI thread up to [`WORKER_PAST`] candidates
//! and as a job for the worker past that (ADR 0096): what a job holds, running it, and the
//! report it sends back.
//!
//! Must not depend on: `editor`, threads or a log. The host runs a job on its thread, and the
//! editor makes it and takes the report in. Neither a job nor a report says in `Debug` what
//! it lists or what is typed: file names can be private, and a prompt can be over a secret.
//!
//! A newer job supersedes the one before through a generation, which a job reads every
//! [`STOP_EVERY`] candidates and between the steps of a build, and stops at once it is not
//! the live one.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::path::{self, Found, Name, Reached};
use super::{Candidate, MAX_INPUT_BYTES, Query, offset_u32};

/// The most candidates a list builds and matches on the UI thread; a longer one is a job for
/// the worker (ADR 0076, ADR 0096).
pub const WORKER_PAST: usize = 5000;

/// How many candidates matching goes through between two looks at whether to stop.
const STOP_EVERY: usize = 1024;

/// What matching an input came to: the candidates it matches, in their order, where an exact
/// match of it is, and the query and input it was.
#[derive(Clone, Default)]
pub(crate) struct Matched {
    pub(crate) matches: Vec<u32>,
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
    let mut matches = Vec::new();
    match before {
        // Only what changed is checked again: the rest matched before.
        Some((matched, before)) if query.narrows(before) => {
            for (at, &index) in matched.iter().enumerate() {
                if at % STOP_EVERY == STOP_EVERY - 1 && stop() {
                    return None;
                }
                if query.matches_after(&items[index as usize], before) {
                    matches.push(index);
                }
            }
        }
        _ => {
            for (at, &index) in order.iter().enumerate() {
                if at % STOP_EVERY == STOP_EVERY - 1 && stop() {
                    return None;
                }
                if query.matches(&items[index as usize]) {
                    matches.push(index);
                }
            }
        }
    }
    let exact = matches
        .iter()
        .position(|&index| items[index as usize].text() == input);
    Some(Matched {
        matches,
        exact,
        query,
        input: Some(input.to_owned()),
    })
}

/// `items` in the order they show: first those in `history`, newest first, then the shorter,
/// then by their text (ADR 0076).
pub(crate) fn order_of<'a>(
    items: &[Candidate],
    history: impl IntoIterator<Item = &'a str>,
) -> Arc<[u32]> {
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
    keyed.into_iter().map(|(_, _, index)| index).collect()
}

/// The list a prompt that reads a path is to have: the directories its segments reached, each
/// with its listing, and what to order and match it by.
pub(crate) struct Build {
    pub(crate) reached: Vec<(Reached, Arc<[Name]>)>,
    /// Names starting with `.` are kept.
    pub(crate) hidden: bool,
    /// The most names kept.
    pub(crate) most: usize,
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
        let mut found = path::found_in(listed, self.hidden, stop)?;
        let more = found.len() > self.most;
        found.truncate(self.most);
        // What a look found of a row is kept beside the list and patches it (ADR 0096).
        let items: Arc<[Candidate]> = found
            .iter()
            .map(|found| Candidate::new(found.name.as_str(), "", found.context.as_str()))
            .collect();
        if stop() {
            return None;
        }
        let order = order_of(&items, self.history.iter().map(String::as_str));
        if stop() {
            return None;
        }
        let matched = match_over(&items, &order, None, self.input.as_deref(), stop)?;
        Some(Built {
            found,
            more,
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
    pub(crate) before: Option<(Vec<u32>, Query)>,
    pub(crate) input: Option<String>,
}

/// What a job does.
enum Work {
    Build(Box<Build>),
    Match(Box<Matching>),
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

    /// Runs the job, on the worker's thread. `None` once a newer job has taken its place,
    /// which it notices within [`STOP_EVERY`] candidates.
    pub fn run(self) -> Option<CompletionReport> {
        let (generation, live) = (self.generation, &self.live);
        let stop = || live.load(Ordering::Relaxed) != generation;
        let report = match &self.work {
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
        };
        Some(CompletionReport(report))
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
}

impl Report {
    pub(crate) fn generation(&self) -> u64 {
        match self {
            Self::Built { generation, .. } | Self::Matched { generation, .. } => *generation,
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
