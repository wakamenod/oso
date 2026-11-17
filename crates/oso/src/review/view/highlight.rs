//! Highlighting a review's sides on its view thread (ADR 0122, step 9): each side of a text
//! reading opened as a document no buffer owns (step 8), the files on screen first, and each
//! report's runs projected over the lines the review shows of that side.
//!
//! Must not depend on: `app`, `dispatch`, git's runner or the file system; the host's
//! [`Highlighter`] takes the documents, and the host hands their reports back.
//!
//! - A side is opened only from a text reading, of at most 2 MiB, in a language, and only when
//!   every path the reading names is judged ordinary by its name (ADR 0044), with the same
//!   check the review's thread made before reading: no text of a file that may hold secrets is
//!   parsed. A file the text cut short shows no line, so its sides are not opened.
//! - Each text opens under a key never used before in this view, and a report counts only
//!   while its side is open under that key: a report of a text replaced or closed is dropped.
//!   Reports that wait together count only the latest of each key.
//! - An old side is known by its blob: a new reading of the same blob keeps its runs and opens
//!   nothing, unless it shows a line they lack. A new side is known by its reading, as one read
//!   from the work tree may not be the blob git named.
//! - At most [`MAX_OPEN`] documents are open at once, those of files on screen opened first,
//!   then the rest in the review's order; one is closed once it is done, fails or is lost, when
//!   its text is replaced or its file leaves, and all of them when highlighting stops or the
//!   view ends. A full schedule stops the opening until a document closes.
//! - Until a side is highlighted whole, only its lines on screen are projected; once it is,
//!   every line the review shows of it.
//! - The runs and lines held count toward a budget ([`MAX_RUNS`], in `budget`).
//! - A side that failed or was lost stays in its diff face while the review is open; one
//!   refused for a long line is tried again with its next text, as that costs one scan.
//! - Nothing here logs a file's text, path or highlights.

mod budget;
mod sides;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::buffer::Verdict;
use oso_core::review::Side;
use oso_core::rope::Rope;
use oso_syntax::{Failure, Language, MAX_BYTES, Outcome, Refused, Report};

pub(crate) use budget::MAX_RUNS;
pub(super) use sides::{SideRuns, add_shown, index};
use sides::{Source, lines_of, on_screen, shown_ranges, source};

use super::project::{self, Projected};
use crate::review::{Content, Reading};

/// The most documents a view keeps open at once: more than the 4 the worker parses at once,
/// so that one finishing has another ready.
pub(crate) const MAX_OPEN: usize = 8;

/// The lines of each side the windows show of each file on screen, by path.
pub(super) type ShownLines = HashMap<Box<[u8]>, SideRuns>;

/// How far a view's highlighting is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Progress {
    /// The screen is known, some side on it is highlighted, and every side on it that can be.
    pub(crate) shown: bool,
    /// Some side is highlighted, and every side that can be.
    pub(crate) whole: bool,
}

/// What takes a view's documents: the app's syntax threads, or a test's stand-in.
pub(crate) trait Highlighter: Send {
    /// Opens `text` under `key`, as `oso_syntax::Scheduler::open` does.
    fn open(
        &self,
        key: u64,
        language: Language,
        verdict: Verdict,
        text: Rope,
        shown: Vec<Range<usize>>,
    ) -> Result<(), Refused>;
    /// What windows show of the document open under `key`.
    fn show(&self, key: u64, shown: Vec<Range<usize>>);
    /// Closes the document open under `key`.
    fn close(&self, key: u64);
}

/// A side's document while it is open.
struct Open {
    key: u64,
    text: Rope,
    bom: bool,
    shown: Vec<Range<usize>>,
}

/// One side of a file: what text it is of, its document while open, and its runs.
#[derive(Default)]
struct SideState {
    source: Option<Source>,
    open: Option<Open>,
    projected: Option<Projected>,
    /// It failed or was lost: it stays in its diff face while the review is open.
    stuck: bool,
    /// Refused, or for a long line: tried again only with another text.
    refused: bool,
    /// Its runs did not fit the budget, at this count of the screen's files: it is opened
    /// again only once other files show.
    over: Option<u64>,
    /// Its runs are whole and cover every line the reading shows of it.
    complete: bool,
}

impl SideState {
    /// Whether it wants its runs: it can have them and does not hold them whole yet, with
    /// `screen` the count of the screen's files now and `shown` whether its file shows.
    fn wants(&self, screen: u64, shown: bool) -> bool {
        !self.stuck
            && !self.refused
            && !self.complete
            && self.over.is_none_or(|at| shown && at != screen)
    }
}

/// A file's two sides, old then new, with the reading they are of.
#[derive(Default)]
struct Sides {
    reading: Option<Arc<Reading>>,
    sides: [SideState; 2],
}

/// A view's highlighting of its sides.
pub(super) struct Highlighting {
    highlighter: Option<Box<dyn Highlighter>>,
    /// The work tree's top, which every path a reading names is judged under.
    top: PathBuf,
    next_key: u64,
    files: HashMap<Box<[u8]>, Sides>,
    keys: HashMap<u64, (Box<[u8]>, Side)>,
    /// What the runs held cost, and the most they may.
    held: usize,
    budget: usize,
    /// The files on screen, and the lines of each side shown, by path.
    shown: ShownLines,
    /// Counts each change of which files are on screen.
    screen: u64,
    /// The files whose sections in the last text show lines.
    laid: HashSet<Box<[u8]>>,
    /// Files whose runs changed since the last text took them.
    changed: HashSet<Box<[u8]>>,
    /// Reports waiting to be taken, the latest of each key.
    reports: HashMap<u64, Report<u64>>,
    /// Nothing more to open until something changes.
    settled: bool,
}

impl Default for Highlighting {
    fn default() -> Self {
        Highlighting {
            highlighter: None,
            top: PathBuf::new(),
            next_key: 0,
            files: HashMap::new(),
            keys: HashMap::new(),
            held: 0,
            budget: MAX_RUNS,
            shown: ShownLines::new(),
            screen: 0,
            laid: HashSet::new(),
            changed: HashSet::new(),
            reports: HashMap::new(),
            settled: false,
        }
    }
}

impl Drop for Highlighting {
    /// The view ended, as its review closed: nothing it opened goes on running or holds a slot.
    fn drop(&mut self) {
        self.close_all();
    }
}

impl Highlighting {
    /// Takes the host's highlighter, or none to stop: every document closes and every file
    /// goes back to its diff faces.
    pub(super) fn set(&mut self, highlighter: Option<Box<dyn Highlighter>>, top: PathBuf) {
        self.close_all();
        for (path, file) in &mut self.files {
            for side in &mut file.sides {
                if side.projected.take().is_some() {
                    self.changed.insert(path.clone());
                }
                // The same text: only what was opened and found goes.
                *side = SideState {
                    source: side.source.take(),
                    ..SideState::default()
                };
            }
        }
        self.held = 0;
        self.reports.clear();
        self.settled = false;
        self.highlighter = highlighter;
        self.top = top;
    }

    /// The work tree's top, which every path a reading names is judged under.
    pub(super) fn top(&self) -> &Path {
        &self.top
    }

    /// Whether it has a highlighter to open documents with.
    pub(super) fn is_on(&self) -> bool {
        self.highlighter.is_some()
    }

    #[cfg(test)]
    pub(super) fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
    }

    fn close_all(&mut self) {
        if let Some(highlighter) = &self.highlighter {
            for key in self.keys.keys() {
                highlighter.close(*key);
            }
        }
        self.keys.clear();
        for file in self.files.values_mut() {
            for side in &mut file.sides {
                side.open = None;
            }
        }
    }

    /// Closes `side`'s document, if it has one open.
    fn close(&mut self, path: &[u8], side: Side) {
        let open = self
            .files
            .get_mut(path)
            .and_then(|file| file.sides[index(side)].open.take());
        if let Some(open) = open {
            self.keys.remove(&open.key);
            if let Some(highlighter) = &self.highlighter {
                highlighter.close(open.key);
            }
            self.settled = false;
        }
    }

    /// The runs of the file at `path`, old side then new.
    pub(super) fn sides(&self, path: &[u8]) -> [Option<&Projected>; 2] {
        let Some(file) = self.files.get(path) else {
            return [None, None];
        };
        let [old, new] = &file.sides;
        [old.projected.as_ref(), new.projected.as_ref()]
    }

    /// The files whose runs changed since the last call.
    pub(super) fn take_changed(&mut self) -> HashSet<Box<[u8]>> {
        std::mem::take(&mut self.changed)
    }

    /// The files whose sections in the text just built show lines: only their sides open.
    pub(super) fn laid(&mut self, laid: HashSet<Box<[u8]>>) {
        if laid != self.laid {
            self.laid = laid;
            self.settled = false;
        }
    }

    /// The file at `path` shows `reading` now: a side whose text changed drops its runs and
    /// closes its document.
    pub(super) fn reading(&mut self, path: &[u8], reading: &Arc<Reading>) {
        let file = match self.files.get_mut(path) {
            Some(file) => file,
            None => self.files.entry(path.into()).or_default(),
        };
        if file
            .reading
            .as_ref()
            .is_some_and(|now| Arc::ptr_eq(now, reading))
        {
            return;
        }
        file.reading = Some(Arc::clone(reading));
        self.settled = false;
        for side in [Side::Old, Side::New] {
            let Some(file) = self.files.get_mut(path) else {
                return;
            };
            let source = source(reading, side);
            let state = &mut file.sides[index(side)];
            if state.source == source {
                // The same text: its runs may lack a line this reading shows.
                if let Content::Text { shape, .. } = &reading.content {
                    state.complete = state.projected.as_ref().is_some_and(|projected| {
                        projected.done && projected.holds_all(lines_of(shape.hunks(), side))
                    });
                }
                continue;
            }
            state.source = source;
            state.complete = false;
            state.refused = false;
            state.over = None;
            if let Some(projected) = state.projected.take() {
                self.held = self.held.saturating_sub(budget::cost(&projected));
                self.changed.insert(path.into());
            }
            self.close(path, side);
        }
    }

    /// The file at `path` left the review.
    pub(super) fn forget(&mut self, path: &[u8]) {
        self.close(path, Side::Old);
        self.close(path, Side::New);
        let Some(file) = self.files.remove(path) else {
            return;
        };
        for state in file.sides {
            if let Some(projected) = state.projected {
                self.held = self.held.saturating_sub(budget::cost(&projected));
            }
        }
        self.shown.remove(path);
    }

    /// What windows show: the lines of each side of each file on screen. Tells each open
    /// document what it shows, if that changed.
    pub(super) fn show(&mut self, shown: ShownLines) {
        let same_files = shown.len() == self.shown.len()
            && shown.keys().all(|path| self.shown.contains_key(path));
        if !same_files {
            self.screen += 1;
            self.settled = false;
        }
        self.shown = shown;
        let Some(highlighter) = &self.highlighter else {
            return;
        };
        for (key, (path, side)) in &self.keys {
            let Some(open) =
                (self.files.get_mut(path)).and_then(|file| file.sides[index(*side)].open.as_mut())
            else {
                continue;
            };
            let runs = self
                .shown
                .get(path)
                .map_or(&[][..], |runs| &runs[index(*side)]);
            let ranges = shown_ranges(&open.text, runs);
            if ranges != open.shown {
                open.shown.clone_from(&ranges);
                highlighter.show(*key, ranges);
            }
        }
    }

    /// Opens documents for the sides that want one, while fewer than [`MAX_OPEN`] are open:
    /// those of files on screen first, then the others in `order`, the review's. Nothing is
    /// looked at again until something changes.
    pub(super) fn schedule(&mut self, order: &[Box<[u8]>]) {
        if self.settled || self.highlighter.is_none() || self.keys.len() >= MAX_OPEN {
            return;
        }
        let on_screen = |at: &usize| self.shown.contains_key(&order[*at]);
        let shown = (0..order.len()).filter(on_screen);
        let rest = (0..order.len()).filter(|at| !on_screen(at));
        let picks: Vec<usize> = shown.chain(rest).collect();
        for path in picks.into_iter().map(|at| &order[at]) {
            for side in [Side::Old, Side::New] {
                if self.keys.len() >= MAX_OPEN || !self.open(path, side) {
                    // Tried again once a document closes.
                    return;
                }
            }
        }
        self.settled = true;
    }

    /// Opens a document for `path`'s `side`, if it wants one and may have one; false if the
    /// schedule is full.
    fn open(&mut self, path: &[u8], side: Side) -> bool {
        if !self.laid.contains(path) {
            return true;
        }
        let on_screen = self.shown.get(path).map(|runs| &runs[index(side)]);
        let screen = self.screen;
        let Some(file) = self.files.get_mut(path) else {
            return true;
        };
        let Some(reading) = &file.reading else {
            return true;
        };
        let Content::Text { shape, old, new } = &reading.content else {
            return true;
        };
        let state = &mut file.sides[index(side)];
        let wanted = state.open.is_none()
            && state.wants(screen, on_screen.is_some())
            && lines_of(shape.hunks(), side).next().is_some();
        if !wanted {
            return true;
        }
        let bytes = match side {
            Side::Old => old,
            Side::New => new,
        };
        let language = sides::language(reading, side);
        let verdict = reading.entry.verdict(&self.top);
        let (Some(language), Some(highlighter)) = (language, &self.highlighter) else {
            state.refused = true;
            return true;
        };
        if bytes.len() > MAX_BYTES || verdict.is_withheld() {
            state.refused = true;
            return true;
        }
        // O(n) of at most 2 MiB, here on the view thread, never on the UI's.
        let (text, bom) = sides::decode(bytes);
        let shown = on_screen.map_or_else(Vec::new, |runs| shown_ranges(&text, runs));
        self.next_key += 1;
        let key = self.next_key;
        match highlighter.open(key, language, verdict, text.clone(), shown.clone()) {
            Ok(()) => {
                state.open = Some(Open {
                    key,
                    text,
                    bom,
                    shown,
                });
                self.keys.insert(key, (path.into(), side));
                true
            }
            Err(Refused::Full) => false,
            Err(Refused::MayHoldSecrets | Refused::TooLarge) => {
                state.refused = true;
                true
            }
        }
    }

    /// Keeps a report to take with the others that came with it: only the latest of a key
    /// counts, each being a whole snapshot, and the last of a document comes last.
    pub(super) fn told(&mut self, report: Report<u64>) {
        match self.reports.get(&report.buffer) {
            Some(kept) if kept.stamp > report.stamp => {}
            _ => {
                self.reports.insert(report.buffer, report);
            }
        }
    }

    /// Takes the reports that waited; whether any changed what the text shows or how far
    /// highlighting is.
    pub(super) fn take_reports(&mut self, order: &[Box<[u8]>]) -> bool {
        let reports: Vec<Report<u64>> = self.reports.drain().map(|(_, report)| report).collect();
        let mut changed = false;
        for report in reports {
            changed |= self.report(report, order);
        }
        changed
    }

    /// Takes a report of a document this view opened: its runs over the lines it projects, or
    /// the end of that side's highlighting. A report of a key no longer open is dropped.
    fn report(&mut self, report: Report<u64>, order: &[Box<[u8]>]) -> bool {
        let Some((path, side)) = self.keys.get(&report.buffer).cloned() else {
            return false;
        };
        let shown = self.shown.get(&path).map(|runs| runs[index(side)].clone());
        let Some(file) = self.files.get_mut(&path) else {
            return false;
        };
        let state = &mut file.sides[index(side)];
        let ended = report.done || !matches!(report.outcome, Outcome::Highlights(_));
        let projected = match (&report.outcome, &state.open, &file.reading) {
            (Outcome::Highlights(highlights), Some(open), Some(reading)) => {
                let Content::Text { shape, .. } = &reading.content else {
                    return false;
                };
                let numbers = lines_of(shape.hunks(), side);
                if report.done {
                    Some(project::project(
                        &open.text, open.bom, highlights, numbers, true,
                    ))
                } else {
                    // Partly done: the lines on screen only, the rest once it is whole.
                    let shown = shown.unwrap_or_default();
                    let numbers = numbers.filter(|number| on_screen(&shown, *number));
                    let projected =
                        project::project(&open.text, open.bom, highlights, numbers, false);
                    (projected.lines() > 0).then_some(projected)
                }
            }
            _ => None,
        };
        match report.outcome {
            Outcome::Failed(Failure::LongLine) => state.refused = true,
            Outcome::Failed(_) | Outcome::Lost => state.stuck = true,
            Outcome::Highlights(_) => {}
        }
        if ended {
            self.close(&path, side);
        }
        let kept = projected.is_some();
        if let Some(projected) = projected {
            self.keep(&path, side, projected, order);
        }
        ended || kept
    }

    /// How far highlighting is, while it is on.
    pub(super) fn progress(&self) -> Option<Progress> {
        self.highlighter.as_ref()?;
        let (mut left, mut left_shown) = (false, false);
        let (mut done, mut done_shown) = (false, false);
        for (path, file) in &self.files {
            let Some(reading) = &file.reading else {
                continue;
            };
            let Content::Text { shape, .. } = &reading.content else {
                continue;
            };
            if !self.laid.contains(path) {
                continue;
            }
            let shown = self.shown.contains_key(path);
            for side in [Side::Old, Side::New] {
                let state = &file.sides[index(side)];
                if state.complete {
                    done = true;
                    done_shown |= shown;
                } else if state.wants(self.screen, shown)
                    && lines_of(shape.hunks(), side).next().is_some()
                {
                    left = true;
                    left_shown |= shown;
                }
            }
        }
        Some(Progress {
            shown: !self.shown.is_empty() && done_shown && !left_shown,
            whole: done && !left,
        })
    }
}
