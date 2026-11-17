//! Highlighting a review's sides on its view thread (ADR 0122, step 9): each side of a text
//! reading opened as a document no buffer owns (step 8), the files on screen first, and each
//! report's runs projected over the lines the review shows of that side.
//!
//! Must not depend on: `app`, `dispatch`, git's runs or the file system; the host's
//! [`Highlighter`] takes the documents, and the host hands their reports back.
//!
//! - A side is opened only from a text reading, of at most 2 MiB, in a language, and only when
//!   every path the reading names is judged ordinary by its name (ADR 0044), as the review's
//!   thread judged it before reading: no text of a file that may hold secrets is parsed.
//! - Each text opens under a key never used before in this view, and a report counts only
//!   while its side is open under that key: a report of a text replaced or closed is dropped.
//! - An old side is known by its blob: a new reading of the same blob keeps its runs and opens
//!   nothing, unless it shows a line they lack. A new side is known by its blob too, or by its
//!   reading when git gave none.
//! - At most [`MAX_OPEN`] documents are open at once, those of files on screen opened first,
//!   then the rest in the review's order; one is closed once it is done, fails or is lost.
//! - The runs held count toward [`MAX_RUNS`]: past it, files on screen take the room of those
//!   farthest from them, and a file off screen keeps no runs, so it is opened again only once
//!   it shows. A side that failed or was lost stays in its diff face while the review is open;
//!   one refused for a long line is tried again with its next text, as that costs one scan.
//! - Nothing here logs a file's text, path or highlights.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use oso_core::buffer::{Mode, Verdict};
use oso_core::display_map::TabWidth;
use oso_core::load::decode_all;
use oso_core::review::Side;
use oso_core::rope::Rope;
use oso_syntax::{Failure, Language, MAX_BYTES, Outcome, Refused, Report};

use super::project::{self, Projected};
use crate::git::{Id, os_path, path_verdict};
use crate::review::{Content, Reading};

/// The most documents a view keeps open at once: more than the 4 the worker parses at once,
/// so that one finishing has another ready.
pub(crate) const MAX_OPEN: usize = 8;

/// The most runs a review's sides hold over its lines: about what one 2 MiB buffer can hold.
pub(crate) const MAX_RUNS: usize = 2_000_000;

/// The lines of each side the windows show of each file on screen, as the first and last
/// number of each, old side then new, by path.
pub(super) type ShownLines = HashMap<Box<[u8]>, [Option<(u32, u32)>; 2]>;

/// How far a view's highlighting is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Progress {
    /// Every side of the files on screen that can be highlighted is.
    pub(crate) shown: bool,
    /// Every side that can be highlighted is.
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

/// What text a side is of.
#[derive(Clone, Debug)]
enum Source {
    Blob(Id),
    Reading(Arc<Reading>),
}

impl PartialEq for Source {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Source::Blob(one), Source::Blob(other)) => one == other,
            (Source::Reading(one), Source::Reading(other)) => Arc::ptr_eq(one, other),
            _ => false,
        }
    }
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
    /// Refused for a long line: tried again only with another text.
    refused: bool,
    /// Its runs were left out as past the budget while its file was off screen.
    over: bool,
    /// Its runs are whole and cover every line the reading shows of it.
    complete: bool,
}

/// A file's two sides, old then new, with the reading they are of.
#[derive(Default)]
struct Sides {
    reading: Option<Arc<Reading>>,
    sides: [SideState; 2],
}

fn index(side: Side) -> usize {
    match side {
        Side::Old => 0,
        Side::New => 1,
    }
}

/// A view's highlighting of its sides.
#[derive(Default)]
pub(super) struct Highlighting {
    highlighter: Option<Box<dyn Highlighter>>,
    /// The work tree's top, which every path a reading names is judged under.
    top: PathBuf,
    next_key: u64,
    files: HashMap<Box<[u8]>, Sides>,
    keys: HashMap<u64, (Box<[u8]>, Side)>,
    runs: usize,
    /// The files on screen, and the lines of each side shown, by path.
    shown: ShownLines,
    /// Files whose runs changed since the last text took them.
    changed: HashSet<Box<[u8]>>,
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
                *side = SideState::default();
            }
        }
        self.runs = 0;
        self.highlighter = highlighter;
        self.top = top;
    }

    /// Whether it has a highlighter to open documents with.
    pub(super) fn is_on(&self) -> bool {
        self.highlighter.is_some()
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

    /// The file at `path` shows `reading` now: a side whose text changed drops its runs and
    /// closes its document.
    pub(super) fn reading(&mut self, path: &[u8], reading: &Arc<Reading>) {
        let file = self.files.entry(path.into()).or_default();
        file.reading = Some(Arc::clone(reading));
        for side in [Side::Old, Side::New] {
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
            if let Some(open) = state.open.take() {
                self.keys.remove(&open.key);
                if let Some(highlighter) = &self.highlighter {
                    highlighter.close(open.key);
                }
            }
            if let Some(projected) = state.projected.take() {
                self.runs = self.runs.saturating_sub(projected.runs());
                self.changed.insert(path.into());
            }
            state.refused = false;
            state.over = false;
        }
    }

    /// The file at `path` left the review.
    pub(super) fn forget(&mut self, path: &[u8]) {
        let Some(file) = self.files.remove(path) else {
            return;
        };
        for state in file.sides {
            if let Some(open) = state.open {
                self.keys.remove(&open.key);
                if let Some(highlighter) = &self.highlighter {
                    highlighter.close(open.key);
                }
            }
            if let Some(projected) = state.projected {
                self.runs = self.runs.saturating_sub(projected.runs());
            }
        }
        self.shown.remove(path);
    }

    /// What windows show: the lines of each side of each file on screen, as the first and last
    /// number of each. Tells each open document what it shows, if that changed.
    pub(super) fn show(&mut self, shown: ShownLines) {
        self.shown = shown;
        let Some(highlighter) = &self.highlighter else {
            return;
        };
        for (path, file) in &mut self.files {
            let lines = self.shown.get(path).copied().unwrap_or([None, None]);
            for (state, lines) in file.sides.iter_mut().zip(lines) {
                let Some(open) = &mut state.open else {
                    continue;
                };
                let ranges = shown_ranges(&open.text, lines);
                if ranges != open.shown {
                    open.shown.clone_from(&ranges);
                    highlighter.show(open.key, ranges);
                }
            }
        }
    }

    /// Opens documents for the sides that want one, while fewer than [`MAX_OPEN`] are open:
    /// those of files on screen first, then the others in `order`, the review's.
    pub(super) fn schedule(&mut self, order: &[Box<[u8]>]) {
        if self.highlighter.is_none() || self.keys.len() >= MAX_OPEN {
            return;
        }
        let on_screen = |at: &usize| self.shown.contains_key(&order[*at]);
        let shown = (0..order.len()).filter(on_screen);
        let rest = (0..order.len()).filter(|at| !on_screen(at));
        let picks: Vec<usize> = shown.chain(rest).collect();
        for path in picks.into_iter().map(|at| &order[at]) {
            if self.keys.len() >= MAX_OPEN {
                return;
            }
            for side in [Side::Old, Side::New] {
                if self.keys.len() < MAX_OPEN {
                    self.open(path, side);
                }
            }
        }
    }

    /// How far highlighting is, while it is on: whether every side of the files on screen is
    /// done, and whether every side is, of those that can be highlighted.
    pub(super) fn progress(&self) -> Option<Progress> {
        self.highlighter.as_ref()?;
        let mut progress = Progress {
            shown: true,
            whole: true,
        };
        for (path, file) in &self.files {
            let Some(reading) = &file.reading else {
                continue;
            };
            let Content::Text { shape, .. } = &reading.content else {
                continue;
            };
            let on_screen = self.shown.contains_key(path);
            for side in [Side::Old, Side::New] {
                let state = &file.sides[index(side)];
                let left = !state.stuck
                    && !state.refused
                    && !state.complete
                    && (!state.over || on_screen)
                    && lines_of(shape.hunks(), side).next().is_some();
                if left {
                    progress.whole = false;
                    progress.shown &= !on_screen;
                }
            }
        }
        Some(progress)
    }

    /// Opens a document for `path`'s `side`, if it wants one and may have one.
    fn open(&mut self, path: &[u8], side: Side) {
        let on_screen = self.shown.get(path).copied();
        let Some(file) = self.files.get_mut(path) else {
            return;
        };
        let Some(reading) = &file.reading else {
            return;
        };
        let Content::Text { shape, old, new } = &reading.content else {
            return;
        };
        let state = &mut file.sides[index(side)];
        let numbers = || lines_of(shape.hunks(), side);
        let wanted = state.open.is_none()
            && !state.stuck
            && !state.refused
            && !state.complete
            && (!state.over || on_screen.is_some())
            && numbers().next().is_some();
        if !wanted {
            return;
        }
        let bytes = match side {
            Side::Old => old,
            Side::New => new,
        };
        let named = match side {
            Side::Old => reading
                .entry
                .old_path
                .as_deref()
                .unwrap_or(&reading.entry.path),
            Side::New => &reading.entry.path,
        };
        let language = os_path(named)
            .ok()
            .and_then(|name| Language::of(Mode::for_path(std::path::Path::new(name))));
        let Some(language) = language else {
            state.refused = true;
            return;
        };
        if bytes.len() > MAX_BYTES {
            state.refused = true;
            return;
        }
        let verdict = verdict_of(&self.top, reading);
        let Some(highlighter) = &self.highlighter else {
            return;
        };
        if verdict.is_withheld() {
            state.refused = true;
            return;
        }
        // O(n) of at most 2 MiB, here on the view thread, never on the UI's.
        let text = decode_all(bytes, TabWidth::default()).text().clone();
        let bom = project::has_bom(bytes);
        let lines = on_screen.and_then(|lines| lines[index(side)]);
        let shown = shown_ranges(&text, lines);
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
            }
            // Tried again once another closes.
            Err(Refused::Full) => {}
            Err(Refused::MayHoldSecrets | Refused::TooLarge) => state.refused = true,
        }
    }

    /// Takes a report of a document this view opened: its runs over the lines shown, or the end
    /// of that side's highlighting. A report of a key no longer open is dropped.
    pub(super) fn report(&mut self, report: Report<u64>, order: &[Box<[u8]>]) {
        let Some((path, side)) = self.keys.get(&report.buffer).cloned() else {
            return;
        };
        let Some(file) = self.files.get_mut(&path) else {
            return;
        };
        let state = &mut file.sides[index(side)];
        let ended = report.done || !matches!(report.outcome, Outcome::Highlights(_));
        let projected = match (&report.outcome, &state.open, &file.reading) {
            (Outcome::Highlights(highlights), Some(open), Some(reading)) => {
                let Content::Text { shape, .. } = &reading.content else {
                    return;
                };
                let numbers = lines_of(shape.hunks(), side);
                Some(project::project(
                    &open.text,
                    open.bom,
                    highlights,
                    numbers,
                    report.done,
                ))
            }
            _ => None,
        };
        match report.outcome {
            Outcome::Failed(Failure::LongLine) => state.refused = true,
            Outcome::Failed(_) | Outcome::Lost => state.stuck = true,
            Outcome::Highlights(_) => {}
        }
        if ended {
            self.keys.remove(&report.buffer);
            if let (Some(open), Some(highlighter)) = (state.open.take(), &self.highlighter) {
                highlighter.close(open.key);
            }
        }
        if let Some(projected) = projected {
            self.keep(&path, side, projected, order);
        }
    }

    /// Keeps `projected` as `path`'s `side`'s runs, within the budget.
    fn keep(&mut self, path: &[u8], side: Side, projected: Projected, order: &[Box<[u8]>]) {
        let before = self.files.get(path).and_then(|file| {
            let state = &file.sides[index(side)];
            state.projected.as_ref().map(Projected::runs)
        });
        let after = self.runs - before.unwrap_or(0) + projected.runs();
        if after > MAX_RUNS && self.shown.contains_key(path) {
            self.make_room(after - MAX_RUNS, path, order);
        }
        let Some(file) = self.files.get_mut(path) else {
            return;
        };
        let state = &mut file.sides[index(side)];
        let held = self.runs - before.unwrap_or(0);
        if held + projected.runs() > MAX_RUNS {
            // Off screen, or more than the room a screen's files can make.
            if let Some(dropped) = state.projected.take() {
                self.runs = self.runs.saturating_sub(dropped.runs());
                self.changed.insert(path.into());
            }
            state.complete = false;
            state.over = true;
            if let Some(open) = state.open.take() {
                self.keys.remove(&open.key);
                if let Some(highlighter) = &self.highlighter {
                    highlighter.close(open.key);
                }
            }
            return;
        }
        self.runs = held + projected.runs();
        // Projected over every line the reading shows of the side.
        state.complete = projected.done;
        state.projected = Some(projected);
        self.changed.insert(path.into());
    }

    /// Drops the runs of files off screen, those farthest in `order` from `near` first, until
    /// `needed` runs are free or none is left to drop.
    fn make_room(&mut self, mut needed: usize, near: &[u8], order: &[Box<[u8]>]) {
        let at = order.iter().position(|path| **path == *near).unwrap_or(0);
        let mut far: Vec<(usize, &Box<[u8]>)> = order
            .iter()
            .enumerate()
            .filter(|(_, path)| !self.shown.contains_key(*path))
            .map(|(index, path)| (index.abs_diff(at), path))
            .collect();
        far.sort_unstable_by_key(|(distance, _)| std::cmp::Reverse(*distance));
        for (_, path) in far {
            if needed == 0 {
                return;
            }
            let Some(file) = self.files.get_mut(path) else {
                continue;
            };
            for state in &mut file.sides {
                if let Some(dropped) = state.projected.take() {
                    needed = needed.saturating_sub(dropped.runs());
                    self.runs = self.runs.saturating_sub(dropped.runs());
                    state.complete = false;
                    state.over = true;
                    self.changed.insert(path.clone());
                }
            }
        }
    }
}

/// What text `reading`'s `side` is of: its blob, or the reading for a work tree file git did
/// not hash; none for a reading that is not text.
fn source(reading: &Arc<Reading>, side: Side) -> Option<Source> {
    if !matches!(reading.content, Content::Text { .. }) {
        return None;
    }
    let id = match side {
        Side::Old => &reading.entry.old_id,
        Side::New => &reading.entry.new_id,
    };
    Some(if id.is_zero() {
        Source::Reading(Arc::clone(reading))
    } else {
        Source::Blob(id.clone())
    })
}

/// The verdict on every path `reading` names, under `top`: ordinary only if each is.
fn verdict_of(top: &std::path::Path, reading: &Reading) -> Verdict {
    let entry = &reading.entry;
    let paths = std::iter::once(&*entry.path).chain(entry.old_path.as_deref());
    paths
        .map(|path| path_verdict(top, path))
        .find(|verdict| verdict.is_withheld())
        .unwrap_or(Verdict::Ordinary)
}

/// The numbers of the lines of `side` that `hunks` show, rising.
pub(super) fn lines_of(
    hunks: &[oso_core::review::Hunk],
    side: Side,
) -> impl Iterator<Item = u32> + '_ {
    hunks.iter().flat_map(move |hunk| {
        let (start, lines) = match side {
            Side::Old => (hunk.old_start, hunk.removed.len()),
            Side::New => (hunk.new_start, hunk.added.len()),
        };
        (0..lines as u32).map(move |offset| start.saturating_add(offset))
    })
}

/// The byte ranges of `text` that `lines`, a first and last line number, cover.
fn shown_ranges(text: &Rope, lines: Option<(u32, u32)>) -> Vec<Range<usize>> {
    let Some((first, last)) = lines else {
        return Vec::new();
    };
    let first = (first as usize).saturating_sub(1);
    let last = (last as usize).saturating_sub(1);
    std::iter::once(text.line_start(first)..text.line_end(last)).collect()
}
