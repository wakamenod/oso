//! Jumps on the editor (ADR 0116): starting one, the string it reads, its candidates in every
//! tiled window, its labels' keys, and where it goes. What it holds is `crate::jump`'s
//! [`Jump`], and the cells it draws are in [`cells`].
//!
//! Must not depend on: `command` or `keymap`, or the host's window, threads or clock: the host
//! keeps the time a string is read for, and calls [`Editor::jump_show_labels`] when it passes.
//!
//! - Only the text in view is looked at: each window's rows in view, within [`WINDOW_BUDGET`]
//!   of text a window and [`JUMP_BUDGET`] in all, the selected window first, then the others
//!   in the tree's order, and each window's rows from its cursor's outward, so a budget that
//!   runs out leaves out the farthest. A key costs as much as the view, not the buffer, and
//!   works out each kept candidate's display points once, for every frame after it.
//! - The candidates nearest each window's primary cursor come first, by rows and then
//!   columns on the screen, and at most [`MAX_LABELLED`] are labelled: the nearest rows' are
//!   picked first, and only their columns are worked out.
//! - Only the primary cursor moves: the place it left goes on its mark ring with no message,
//!   unless a region shows, which the jump then extends.
//! - Nothing here logs the string, and `Debug` prints none of it (ADR 0044).

use std::ops::Range;

use super::Editor;
use crate::buffer::Buffer;
use crate::display_map::DisplayPoint;
use crate::jump::{Candidate, Jump, Keys, Kind, MAX_LABELLED, MAX_STRING, Stage};
use crate::search::{self, Needle, Query};
use crate::sum_tree::Bias;
use crate::window::{Screen, WindowId};

mod cells;

pub use cells::{JumpCells, LabelCells};

/// The most text of one window a key looks at, as isearch's highlights (ADR 0082).
const WINDOW_BUDGET: usize = 256 * 1024;

/// The most text of every window a key looks at, so that many windows cost no more than a
/// few.
const JUMP_BUDGET: usize = 1024 * 1024;

/// The most matches a key keeps before it picks the nearest to label.
const MAX_FOUND: usize = 64 * 1024;

/// What a key typed while labels show did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chose {
    /// It was a label's first keys: the labels that start with them stay.
    Narrowed,
    /// It ended a label: the cursor went to its candidate, and the jump ended.
    Jumped,
    /// No label goes on with it: the jump ended, and nothing moved.
    NoSuch,
}

/// A candidate found, before the nearest are picked.
#[derive(Debug)]
struct Near {
    /// The window's place among those looked at, and its row's distance from the cursor's.
    key: (usize, usize),
    /// Its column's distance from the cursor's, worked out only for those kept.
    column: usize,
    candidate: Candidate,
}

/// What finding candidates works in, kept from key to key so that a key allocates nothing
/// once they have grown.
#[derive(Debug, Default)]
pub(super) struct Scratch {
    found: Vec<Near>,
    ranges: Vec<Range<usize>>,
    windows: Vec<WindowId>,
    /// Each window looked at, by its place among them, and its primary cursor's point.
    cursors: Vec<(WindowId, DisplayPoint)>,
    screen: Screen,
}

impl Editor {
    /// The jump under way, if any.
    pub fn jump(&self) -> Option<&Jump> {
        self.jump.as_ref()
    }

    /// Sets the keys labels are typed with, as `[jump] keys` gives them. A jump under way
    /// keeps its own.
    pub fn set_jump_keys(&mut self, keys: Keys) {
        self.jump_keys = keys;
    }

    /// Starts a jump of `kind` in every tiled window, or only in the selected one if
    /// `selected_only`. A line jump and `avy-isearch` show their labels at once, and end at
    /// once, saying so, if there is nothing to label. Returns false in a prompt, where it does
    /// not work, and for [`Kind::Isearch`] with no isearch under way.
    pub fn jump_begin(&mut self, kind: Kind, selected_only: bool) -> bool {
        if !self.prompts.is_empty() || (kind == Kind::Isearch && self.isearch.is_none()) {
            return false;
        }
        let around_secret = self.shows_secret();
        let selected_only = selected_only || kind == Kind::Isearch;
        let keys = self.jump_keys.clone();
        self.jump = Some(Jump::new(kind, selected_only, keys, around_secret));
        if kind != Kind::Char {
            self.jump_show_labels();
        }
        true
    }

    /// Adds `text` to the string a jump reads, up to [`MAX_STRING`] characters, and finds its
    /// matches in view again.
    pub fn jump_add(&mut self, text: &str) {
        self.jump_follow_text();
        let Some(jump) = self
            .jump
            .as_mut()
            .filter(|jump| jump.stage == Stage::Reading)
        else {
            return;
        };
        let room = MAX_STRING.saturating_sub(jump.string.chars().count());
        jump.string.extend(text.chars().take(room));
        self.find_candidates();
    }

    /// Takes the last character off the string a jump reads, and finds its matches in view
    /// again. Returns whether there was one.
    pub fn jump_delete(&mut self) -> bool {
        self.jump_follow_text();
        let Some(jump) = self
            .jump
            .as_mut()
            .filter(|jump| jump.stage == Stage::Reading)
        else {
            return false;
        };
        if jump.string.pop().is_none() {
            return false;
        }
        self.find_candidates();
        true
    }

    /// Shows a jump's labels: when the time after the last key passed, or at `RET`. A string
    /// not begun waits for a key. With nothing to label, the jump ends saying `No candidates`.
    pub fn jump_show_labels(&mut self) {
        if !self.jump.as_ref().is_some_and(|jump| {
            jump.stage == Stage::Reading && !(jump.kind == Kind::Char && jump.string.is_empty())
        }) {
            return;
        }
        self.find_candidates();
        let Some(jump) = &mut self.jump else {
            return;
        };
        if jump.candidates.is_empty() {
            self.jump = None;
            self.echo("No candidates");
            return;
        }
        jump.label();
    }

    /// A key typed while labels show: narrows them to those that go on with it, or, at a
    /// label's last key, goes to its candidate. A key no label goes on with ends the jump
    /// saying `No such candidate`.
    pub fn jump_choose(&mut self, key: char) -> Chose {
        if self.jump_follow_text() && self.jump.is_none() {
            return Chose::NoSuch;
        }
        let Some(jump) = self
            .jump
            .as_mut()
            .filter(|jump| jump.stage == Stage::Labels)
        else {
            return Chose::NoSuch;
        };
        let chosen = jump.keys.index_of(key).and_then(|index| {
            let (at, label) = jump.labels_after(index).next()?;
            Some((at, label.len(), index))
        });
        let Some((at, len, index)) = chosen else {
            self.jump = None;
            self.echo("No such candidate");
            return Chose::NoSuch;
        };
        if len > jump.typed.len() + 1 {
            jump.typed.push(index);
            return Chose::Narrowed;
        }
        let candidate = jump.candidates[at].clone();
        let kind = jump.kind;
        self.jump = None;
        self.jump_to(kind, &candidate);
        Chose::Jumped
    }

    /// Ends a jump with nothing moved and nothing said, as a key it does not bind does.
    /// Returns whether one was under way.
    pub fn jump_end(&mut self) -> bool {
        self.jump.take().is_some()
    }

    /// `C-g` in a jump: ends it with nothing moved, first of `C-g`'s steps (ADR 0116).
    pub(super) fn jump_quit(&mut self) -> bool {
        self.jump_end()
    }

    /// Keeps a jump true to the windows and the text it looks at: a prompt over it ends it,
    /// a change while its string is read finds the matches again, and a change under its
    /// labels ends it, saying so. Returns whether what shows changed.
    pub(super) fn jump_follow_text(&mut self) -> bool {
        let Some(jump) = &self.jump else {
            return false;
        };
        if !self.prompts.is_empty() {
            self.jump = None;
            return true;
        }
        let same = |&(window, buffer, revision): &(WindowId, _, u64)| {
            self.window_by_id(window).is_some_and(|shown| {
                shown.buffer() == buffer
                    && self
                        .buffer(buffer)
                        .is_some_and(|b| b.revision() == revision)
            })
        };
        if jump.shown.iter().all(same) {
            return false;
        }
        match jump.stage {
            Stage::Reading => self.find_candidates(),
            Stage::Labels => {
                self.jump = None;
                self.echo("Jump ended: the text changed");
            }
        }
        true
    }

    /// Writes what the echo row says of the jump into `out`, if one is under way.
    pub(super) fn write_jump_line(&self, out: &mut String) {
        if let Some(jump) = &self.jump {
            jump.write_line(out);
        }
    }

    /// Moves the primary cursor of `candidate`'s window to it, selecting that window first.
    /// `avy-isearch` then ends the search there, as `RET` does, which saves where it began.
    fn jump_to(&mut self, kind: Kind, candidate: &Candidate) {
        if candidate.window != self.tiled_id() {
            self.select_window(candidate.window);
        }
        let Some(index) = self.window_index(candidate.window) else {
            return;
        };
        let (window, buffer) = self.window_mut(index);
        let offset = crate::motion::cluster_start(&buffer.display(), candidate.range.start);
        let selections = &mut window.selections;
        let from = selections.primary().cursor;
        let region = selections.mark_active().is_active();
        if kind != Kind::Isearch && !region && from != offset {
            selections.push_primary_mark(from);
        }
        selections.move_primary(offset);
        self.follow_cursor();
        if kind == Kind::Isearch {
            self.isearch_exit();
        }
    }

    /// Finds a jump's candidates in view again, nearest first, keeping the first
    /// [`MAX_LABELLED`], and notes what each window shows.
    fn find_candidates(&mut self) {
        let Some(jump) = &self.jump else {
            return;
        };
        let needle = match jump.kind {
            Kind::Char => string_needle(&jump.string),
            Kind::Isearch => self.isearch_needle(),
            Kind::LineStart | Kind::LineEnd => None,
        };
        let mut scratch = std::mem::take(&mut self.jump_scratch);
        let Scratch {
            found,
            ranges,
            windows,
            cursors,
            screen,
        } = &mut scratch;
        found.clear();
        windows.clear();
        cursors.clear();
        let selected = self.tiled_id();
        windows.push(selected);
        if !jump.selected_only {
            let others = self.frame().windows().iter().filter_map(|(id, regions)| {
                (regions.has_cells() && *id != selected).then_some(*id)
            });
            windows.extend(others);
        }
        let mut shown = Vec::with_capacity(windows.len());
        let mut left = JUMP_BUDGET;
        for &id in windows.iter() {
            let Some((window, buffer, regions)) = self.window_in_tree(id) else {
                continue;
            };
            shown.push((id, window.buffer(), buffer.revision()));
            let display = buffer.display();
            let cursor = display.offset_to_display_point(window.selections.primary().cursor);
            let rank = cursors.len();
            cursors.push((id, cursor));
            self.screen_of(id, screen);
            let rows = screen.rows().len().min(regions.text.rows);
            let within = (
                WINDOW_BUDGET.min(left),
                MAX_FOUND.saturating_sub(found.len()),
            );
            let place = Place {
                rank,
                id,
                buffer,
                screen,
                rows,
                cursor,
            };
            left -= candidates_in(&place, (jump.kind, needle.as_ref()), within, found, ranges);
            if left == 0 || found.len() >= MAX_FOUND {
                break;
            }
        }
        let total = found.len();
        if total > MAX_LABELLED {
            found.select_nth_unstable_by_key(MAX_LABELLED, |near| near.key);
            found.truncate(MAX_LABELLED);
        }
        self.place_kept(found, cursors);
        found.sort_unstable_by_key(|near| (near.key, near.column, near.candidate.range.start));
        let around_secret = self.shows_secret();
        let Some(jump) = &mut self.jump else {
            return;
        };
        jump.candidates.clear();
        jump.candidates
            .extend(found.drain(..).map(|near| near.candidate));
        jump.found = total;
        jump.shown = shown;
        jump.around_secret |= around_secret;
        self.jump_scratch = scratch;
    }
}

impl Editor {
    /// Works out the display points of the candidates `found` kept, and their columns' distance
    /// from their window's primary cursor's, from `cursors`, each window's by its place.
    fn place_kept(&self, found: &mut [Near], cursors: &[(WindowId, DisplayPoint)]) {
        for near in found {
            let (id, cursor) = cursors[near.key.0];
            let Some(window) = self.window_by_id(id) else {
                continue;
            };
            let display = self.shown(window).display();
            let candidate = &mut near.candidate;
            candidate.start = display.offset_to_display_point(candidate.range.start);
            candidate.end = if candidate.range.is_empty() {
                candidate.start
            } else {
                display.offset_to_display_point(candidate.range.end)
            };
            near.column = candidate.start.column.abs_diff(cursor.column);
        }
    }
}

/// What a jump's string matches: the literal string, with isearch's smart case. Migemo's
/// pattern takes its place for a romaji string once ADR 0116's step 2 is in.
fn string_needle(string: &str) -> Option<Needle> {
    Query::new(string, search::smart_case(string)).map(Needle::Literal)
}

/// Where [`candidates_in`] looks: the window's place among those looked at, the window, its
/// buffer, its rows in view, how many of them its text area shows, and its primary cursor's
/// point.
struct Place<'a> {
    rank: usize,
    id: WindowId,
    buffer: &'a Buffer,
    screen: &'a Screen,
    rows: usize,
    cursor: DisplayPoint,
}

/// The rows of `count` from the cursor's row `at` outward: it, then one below and one above,
/// and so on, so that a budget that runs out leaves out the farthest rows.
pub(super) fn outward(at: usize, count: usize) -> impl Iterator<Item = usize> {
    let first = (count > 0).then_some(at.min(count.saturating_sub(1)));
    let at = first.unwrap_or(0);
    let rest = (1..count).flat_map(move |step| {
        let below = Some(at + step).filter(|&row| row < count);
        [below, at.checked_sub(step)]
    });
    first.into_iter().chain(rest.flatten())
}

/// Adds the candidates of `kind` in a window's rows in view to `found`, reading at most
/// `budget` bytes and adding at most `most`, from the cursor's row outward. Returns about how
/// many bytes it read. `ranges` is room to work in.
fn candidates_in(
    place: &Place<'_>,
    (kind, needle): (Kind, Option<&Needle>),
    (budget, most): (usize, usize),
    found: &mut Vec<Near>,
    ranges: &mut Vec<Range<usize>>,
) -> usize {
    let display = place.buffer.display();
    let (screen, text) = (place.screen, place.buffer.text());
    let shown = &screen.rows()[..place.rows];
    let cursor_row = screen.row_of(place.cursor).unwrap_or(0);
    let limit = found.len().saturating_add(most);
    let push = |found: &mut Vec<Near>, range: Range<usize>, index: usize| {
        found.push(Near {
            key: (place.rank, index.abs_diff(cursor_row)),
            column: 0,
            candidate: Candidate {
                window: place.id,
                range,
                start: DisplayPoint::default(),
                end: DisplayPoint::default(),
            },
        });
    };
    match (kind, needle) {
        (Kind::LineStart, _) => {
            for (index, row) in shown.iter().enumerate().take(most) {
                // A line's first row, with its first column in view.
                if row.starts_line() && screen.origin(row) == 0 {
                    push(found, row.start..row.start, index);
                }
            }
            0
        }
        (Kind::LineEnd, _) => {
            for (index, row) in shown.iter().enumerate().take(most) {
                // A line's last row, with its end, or the cell after its text, in view.
                if row.end_column != usize::MAX {
                    continue;
                }
                let end = text.line_end(row.line);
                let column = display.offset_to_display_point(end).column;
                if screen.drawn(row).contains(&column) {
                    push(found, end..end, index);
                }
            }
            0
        }
        (_, Some(needle)) => {
            let (reach, invalid) = (needle.reach(), place.buffer.invalid_bytes());
            let mut left = budget;
            for index in outward(cursor_row, shown.len()) {
                if left == 0 || found.len() >= limit {
                    break;
                }
                let row = &shown[index];
                let offset = |column| {
                    display.display_point_to_offset(DisplayPoint::new(row.line, column), Bias::Left)
                };
                let drawn = screen.drawn(row);
                // A match must start within the row's columns in view, though it may reach
                // past them.
                let (start, end) = (offset(drawn.start), offset(drawn.end));
                let span = start..end.saturating_add(reach).min(text.len());
                let len = span.len();
                ranges.clear();
                let most = limit - found.len();
                needle.matches_in(
                    (text, invalid),
                    std::slice::from_ref(&span),
                    (most, left),
                    ranges,
                );
                for range in ranges.drain(..) {
                    if range.start < end {
                        push(found, range, index);
                    }
                }
                left = left.saturating_sub(len.max(1));
            }
            budget - left
        }
        (_, None) => 0,
    }
}
