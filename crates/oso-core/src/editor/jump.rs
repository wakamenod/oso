//! Jumps on the editor (ADR 0116): starting one, the string it reads, its candidates in every
//! tiled window, its labels' keys, and where it goes. What it holds is `crate::jump`'s
//! [`Jump`], the cells it draws are in [`cells`], and finding its candidates in a window
//! in [`find`].
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

use super::{Editor, Isearch};
use crate::display_map::DisplayPoint;
use crate::jump::{Candidate, Jump, Keys, Kind, MAX_LABELLED, MAX_STRING, Shown, Stage};
use crate::window::{Screen, WindowId};

mod cells;
mod find;

#[cfg(test)]
pub(super) use find::outward;
use find::{Place, candidates_in};

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
    /// What each window looked at shows now.
    views: Vec<Shown>,
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
        // `avy-isearch` labels what the isearch found, so it is around what that was around.
        let around_secret = self.shows_secret()
            || (kind == Kind::Isearch && self.isearch.as_ref().is_some_and(Isearch::around_secret));
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
        self.jump_follow(false);
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
        let moved = self.jump_follow(false);
        let Some(jump) = self.jump.as_mut() else {
            return false;
        };
        if jump.stage == Stage::Labels {
            self.jump_refuse();
            return false;
        }
        let deleted = jump.string.pop().is_some();
        if deleted || moved {
            self.find_candidates();
        }
        deleted
    }

    /// Shows a jump's labels: when the time after the last key passed, or at `RET`. A string
    /// not begun waits for a key. With nothing to label, the jump ends saying `No candidates`.
    pub fn jump_show_labels(&mut self) {
        let moved = self.jump_follow(false);
        let Some(jump) = &self.jump else {
            return;
        };
        if jump.stage == Stage::Labels {
            self.jump_refuse();
            return;
        }
        if jump.kind == Kind::Char && jump.string.is_empty() {
            return;
        }
        // What it found at the last key stands, unless what it looks at moved since.
        if moved || jump.shown.is_empty() {
            self.find_candidates();
        }
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
        let Some(jump) = (self.jump.as_mut()).filter(|jump| jump.stage == Stage::Labels) else {
            return Chose::NoSuch;
        };
        if jump.ended.is_some() {
            self.jump_refuse();
            return Chose::NoSuch;
        }
        let chosen = jump.keys.index_of(key).and_then(|index| {
            let (at, label) = jump.labels_after(index).next()?;
            Some((at, label.len(), index))
        });
        let Some((at, len, index)) = chosen else {
            self.jump_refuse();
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

    /// Ends a jump as a key it does not take does: with nothing moved, saying `No such
    /// candidate` if its labels showed. Returns whether one was under way.
    pub fn jump_refuse(&mut self) -> bool {
        let Some(jump) = self.jump.take() else {
            return false;
        };
        if let Some(why) = jump.ended {
            self.echo(why);
        } else if jump.stage == Stage::Labels {
            self.echo("No such candidate");
        }
        true
    }

    /// Keeps a jump true to the windows and the text it looks at: a prompt over it ends it,
    /// a change while its string is read finds the matches again, and a change under its
    /// labels ends it, saying so. Returns whether what shows changed.
    pub(super) fn jump_follow_text(&mut self) -> bool {
        self.jump_follow(true)
    }

    /// [`Editor::jump_follow_text`], which leaves a jump reading its string to find its
    /// matches again itself unless `find`. A change is any to what [`Shown`] holds, such as a
    /// wheel's scroll.
    fn jump_follow(&mut self, find: bool) -> bool {
        if self.jump.is_some() && !self.prompts.is_empty() {
            self.jump = None;
            return true;
        }
        let Some(jump) =
            (self.jump.as_ref()).filter(|jump| !jump.shown.is_empty() && jump.ended.is_none())
        else {
            return false;
        };
        let mut scratch = std::mem::take(&mut self.jump_scratch);
        self.jump_views(jump.selected_only, &mut scratch.views);
        let views = &scratch.views;
        let moved = *views != jump.shown;
        let edited = jump.shown.iter().any(|was| {
            views
                .iter()
                .find(|now| now.window == was.window)
                .is_some_and(|now| (now.buffer, now.revision) != (was.buffer, was.revision))
        });
        let stage = jump.stage;
        self.jump_scratch = scratch;
        if !moved {
            return false;
        }
        match stage {
            Stage::Reading if find => self.find_candidates(),
            Stage::Reading => {}
            Stage::Labels => {
                // The person did not end it, so the key they type next was meant for a label.
                let why = if edited {
                    "Jump ended: the text changed"
                } else {
                    "Jump ended: the view changed"
                };
                if let Some(jump) = &mut self.jump {
                    jump.ended = Some(why);
                }
                self.echo(why);
            }
        }
        true
    }

    /// What each window a jump looks at shows, into `views`: the selected window, then, unless
    /// `selected_only`, the others with cells, in the tree's order.
    fn jump_views(&self, selected_only: bool, views: &mut Vec<Shown>) {
        views.clear();
        let selected = self.tiled_id();
        let others = self.frame().windows().iter().filter_map(|(id, regions)| {
            (!selected_only && regions.has_cells() && *id != selected).then_some(*id)
        });
        for id in std::iter::once(selected).chain(others) {
            let Some((window, buffer, regions)) = self.window_in_tree(id) else {
                continue;
            };
            views.push(Shown {
                window: id,
                buffer: window.buffer(),
                revision: buffer.revision(),
                scroll: window.scroll(),
                text: regions.text,
                cursor: window.selections.primary().cursor,
            });
        }
    }

    /// Writes what the echo row says of the jump into `out`, if one is under way and no change
    /// ended its labels.
    pub(super) fn write_jump_line(&self, out: &mut String) {
        if let Some(jump) = self.jump.as_ref().filter(|jump| jump.ended.is_none()) {
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
        let (kind, selected_only) = (jump.kind, jump.selected_only);
        let needle = match kind {
            // Migemo by the setting, as isearch starts (ADR 0116), kept out of the cache as
            // isearch keeps a string typed around a secret out of its history.
            Kind::Char => {
                let (string, around_secret) = (jump.string.clone(), jump.around_secret);
                self.needle_for(&string, around_secret)
            }
            Kind::Isearch => self.isearch_needle(),
            Kind::LineStart | Kind::LineEnd => None,
        };
        let mut scratch = std::mem::take(&mut self.jump_scratch);
        let Scratch {
            found,
            ranges,
            views,
            cursors,
            screen,
        } = &mut scratch;
        found.clear();
        cursors.clear();
        self.jump_views(selected_only, views);
        let mut left = JUMP_BUDGET;
        for view in views.iter() {
            let (id, buffer) = (view.window, self.buffer(view.buffer));
            let Some(buffer) = buffer else {
                continue;
            };
            let display = buffer.display();
            let cursor = display.offset_to_display_point(view.cursor);
            let rank = cursors.len();
            cursors.push((id, cursor));
            self.screen_of(id, screen);
            let rows = screen.rows().len().min(view.text.rows);
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
            left -= candidates_in(&place, (kind, needle.as_ref()), within, found, ranges);
            if left == 0 || found.len() >= MAX_FOUND {
                break;
            }
        }
        let total = found.len();
        if total > MAX_LABELLED {
            // On the row the cut falls in, the text's order picks, as no column is known yet.
            found.select_nth_unstable_by_key(MAX_LABELLED, |near| {
                (near.key, near.candidate.range.start)
            });
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
        jump.shown.clone_from(views);
        jump.around_secret |= around_secret;
        self.jump_scratch = scratch;
    }
}

impl Editor {
    /// Works out the display points of the candidates `found` kept, and their columns' distance
    /// from their window's primary cursor's, from `cursors`, each window's by its place. Each
    /// window's display is taken once: there are few windows, and many candidates.
    fn place_kept(&self, found: &mut [Near], cursors: &[(WindowId, DisplayPoint)]) {
        for (rank, &(id, cursor)) in cursors.iter().enumerate() {
            let Some(window) = self.window_by_id(id) else {
                continue;
            };
            let display = self.shown(window).display();
            for near in found.iter_mut().filter(|near| near.key.0 == rank) {
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
}
