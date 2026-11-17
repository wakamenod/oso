//! A review's two panes (ADR 0122, step 11): its old side and its new side as two generated
//! buffers in a pair of windows that scroll together (step 10), lined up row for row by the
//! filler rows the host lays out, so that their map is every line the same line.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads. The host builds both
//! panes' texts on the review's view thread, only while some area shows them or is about to,
//! and judges each area's width.
//!
//! - The panes are made by the first texts the host puts in, and dropped after a command once
//!   no window shows either.
//! - A switch carries the cursor and each window's top by place: a line of a side, or the same
//!   row of a file's header, a hunk's header, a note or a card. A filler row is placed by the
//!   other pane's row beside it.
//! - On a switch to the panes, the pane of the cursor's side holds it, and is selected when the
//!   window was; a header, a note or a card's row of both sides goes to the new pane.
//! - After each command, a cursor of the selected pane on a filler row moves on the way the
//!   primary cursor went, or back when nothing lies that way.

use crate::buffer::{BufferId, Mode};
use crate::display_map::DisplayPoint;
use crate::editor::{Editor, NoPair, ReviewId, ReviewLeftover};
use crate::frame::{TooSmall, Way};
use crate::load::Reloaded;
use crate::motion;
use crate::review::{Place, RowKind, Rows, Side};
use crate::selection::Goal;
use crate::sum_tree::Bias;
use crate::syntax::Highlights;
use crate::window::{RowMap, WindowId};

/// Which of `buffers` and `rows` is the old side's.
const OLD: usize = 0;
/// Which is the new side's.
const NEW: usize = 1;

/// A review's two panes.
#[derive(Debug)]
pub(super) struct Panes {
    /// The old pane's buffer, then the new one's.
    pub(super) buffers: [BufferId; 2],
    pub(super) rows: [Rows; 2],
    /// The selected pane's window and its primary cursor's line after the last command, so that
    /// a cursor on a filler row moves on the way it went.
    last: Option<(WindowId, usize)>,
}

/// How a review shows in an area of the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewLayout {
    /// Its one stream, in one window; a pane shown alone, as when its pair broke, counts too.
    Unified,
    /// Its two panes, side by side with [`Way::Across`] or stacked with [`Way::Down`].
    Panes(Way),
}

/// An area of the frame a review shows in: a window showing it unified or a pane of it alone,
/// or a pair of its panes, named by its first window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReviewArea {
    pub window: WindowId,
    pub review: ReviewId,
    /// Its whole width in cells, its gutters and dividers too.
    pub columns: usize,
    pub layout: ReviewLayout,
}

/// Where a window shows its review: its primary cursor's place and its top's, each with its
/// file, and where the cursor aims in a line.
struct Places {
    cursor: Option<(Box<[u8]>, Place)>,
    top: Option<(Box<[u8]>, Place)>,
    goal: Goal,
    left: usize,
}

fn side_index(side: Side) -> usize {
    match side {
        Side::Old => OLD,
        Side::New => NEW,
    }
}

impl Editor {
    /// The old pane's buffer and the new one's of the review `id`, while it has them.
    pub fn review_panes(&self, id: ReviewId) -> Option<(BufferId, BufferId)> {
        let panes = self.reviews.get(id)?.panes.as_ref()?;
        Some((panes.buffers[OLD], panes.buffers[NEW]))
    }

    /// Puts in the review `id`'s two panes' whole new texts, with what each line is and their
    /// highlights, as [`Editor::set_review_text`] puts in its buffer's, making the panes'
    /// buffers the first time. Their windows in a pair move in step again. The host calls it
    /// only while [`Editor::review_waits`] is false, and frees what this returns off the UI
    /// thread: all it was given when the two do not have as many lines, or the review is gone.
    pub fn set_review_panes(
        &mut self,
        id: ReviewId,
        old: (Reloaded, Rows, Highlights),
        new: (Reloaded, Rows, Highlights),
    ) -> ReviewLeftover {
        if old.1.lines() != new.1.lines() || self.reviews.get(id).is_none() {
            let refused = [old, new]
                .map(|(text, rows, highlights)| ReviewLeftover::of(rows, highlights, Some(text)));
            return ReviewLeftover {
                panes: refused.into(),
                ..ReviewLeftover::none()
            };
        }
        let buffers = match self.review_panes(id) {
            Some((old, new)) => [old, new],
            None => self.make_review_panes(id),
        };
        let mut left = ReviewLeftover::none();
        for (side, (text, rows, highlights)) in [old, new].into_iter().enumerate() {
            let mut pane = self.put_generated_text(buffers[side], text, highlights);
            pane.rows = match self
                .reviews
                .get_mut(id)
                .and_then(|open| open.panes.as_mut())
            {
                Some(panes) => Some(std::mem::replace(&mut panes.rows[side], rows)),
                None => Some(rows),
            };
            left.panes.push(pane);
        }
        let paired: Vec<WindowId> = (self.windows.iter())
            .filter(|window| window.buffer() == buffers[OLD])
            .map(|window| window.id())
            .filter(|&window| self.pair_of(window).is_some())
            .collect();
        for window in paired {
            self.set_row_map(window, RowMap::same());
        }
        left
    }

    /// Makes the review `id`'s two panes' buffers, empty, named after its review buffer.
    fn make_review_panes(&mut self, id: ReviewId) -> [BufferId; 2] {
        let name = (self.review_buffer(id))
            .and_then(|buffer| self.buffer(buffer))
            .map(|buffer| buffer.name().to_owned())
            .unwrap_or_default();
        let base = name.strip_suffix('*').unwrap_or(&name).to_owned();
        let buffers = ["old", "new"].map(|side| {
            let buffer = self.add_internal_buffer(format!("{base} \u{b7} {side}*"), Mode::Review);
            if let Some(shown) = self.buffer_mut(buffer) {
                shown.set_wraps(false);
            }
            buffer
        });
        if let Some(open) = self.reviews.get_mut(id) {
            open.panes = Some(Panes {
                buffers,
                rows: [Rows::default(), Rows::default()],
                last: None,
            });
        }
        buffers
    }

    /// Shows the review in the tiled window `window` as its two panes, the way `way`: the old
    /// pane in `window` and the new one paired beside it, or below, each at the place the
    /// window showed. A pair of its panes turns to `way`. The pane of the cursor's side is
    /// selected when `window` was. Ends isearch.
    ///
    /// # Errors
    ///
    /// [`NoPair`]: [`NoPair::NoBuffer`] for a window showing no review or one with no panes
    /// yet, [`NoPair::Paired`] for one paired with another buffer, [`NoPair::TooSmall`] where
    /// a split would be. Nothing changes.
    pub fn show_review_panes(&mut self, window: WindowId, way: Way) -> Result<(), NoPair> {
        let index = self.window_index(window).ok_or(NoPair::NoBuffer)?;
        let buffer = self.windows[index].buffer();
        let open = self.reviews.holding(buffer).ok_or(NoPair::NoBuffer)?;
        let buffers = open.panes.as_ref().ok_or(NoPair::NoBuffer)?.buffers;
        if let Some(other) = self.pair_of(window) {
            let shown = self.window_by_id(other).map(|other| other.buffer());
            if buffers.contains(&buffer) && shown.is_some_and(|shown| buffers.contains(&shown)) {
                return (self.turn_pair(window, way).map(drop))
                    .map_err(|TooSmall| NoPair::TooSmall);
            }
            return Err(NoPair::Paired);
        }
        self.paired_splits(window, way)?;
        let places = self.review_places(index);
        let (side, line, top) = {
            let open = self.reviews.holding(buffer);
            let Some(panes) = open.and_then(|open| open.panes.as_ref()) else {
                return Err(NoPair::NoBuffer);
            };
            let (side, line) = places
                .cursor
                .as_ref()
                .and_then(|(path, place)| pane_line(panes, path, *place))
                .unwrap_or((NEW, 0));
            let top = (places.top.as_ref())
                .and_then(|(path, place)| pane_line(panes, path, *place))
                .map_or(0, |(_, line)| line);
            (side, line, top)
        };
        let selected = self.tiled_id() == window;
        self.window_shows(index, buffers[OLD]);
        self.follow_lead(index, (top, places.left), line, Some(places.goal));
        let new = self.pair_window_of(window, way, buffers[NEW], RowMap::same())?;
        if selected && side == NEW {
            self.select_window(new);
        }
        Ok(())
    }

    /// Shows the review in `window`, a pane of it alone or in a pair of its panes, unified
    /// again, at the place the pair's selected window showed, else `window`'s: the pair's other
    /// window closes, `window` taking its cells. Returns whether it did: not for a window that
    /// shows no pane, nor beneath a prompt over the pair (ADR 0072). Ends isearch.
    pub fn show_review_unified(&mut self, window: WindowId) -> bool {
        let Some(index) = self.window_index(window) else {
            return false;
        };
        let buffer = self.windows[index].buffer();
        let Some(open) = self.reviews.holding(buffer) else {
            return false;
        };
        let unified = open.buffer;
        let Some(buffers) = open.panes.as_ref().map(|panes| panes.buffers) else {
            return false;
        };
        if !buffers.contains(&buffer) {
            return false;
        }
        let other = self.pair_of(window).filter(|&other| {
            (self.window_by_id(other)).is_some_and(|other| buffers.contains(&other.buffer()))
        });
        let lead = match other {
            Some(other) if other == self.tiled_id() => self.window_index(other).unwrap_or(index),
            _ => index,
        };
        let places = self.review_places(lead);
        if other.is_some() && !self.unpair(window) {
            return false;
        }
        let Some(index) = self.window_index(window) else {
            return false;
        };
        let line_of = |editor: &Editor, at: Option<&(Box<[u8]>, Place)>| {
            let rows = editor.review_rows_of(unified)?;
            let (path, place) = at?;
            rows.find(path, *place).or_else(|| rows.file_start(path))
        };
        let line = line_of(self, places.cursor.as_ref()).unwrap_or(0);
        let top = line_of(self, places.top.as_ref()).unwrap_or(0);
        self.window_shows(index, unified);
        self.follow_lead(index, (top, places.left), line, Some(places.goal));
        true
    }

    /// Each area of the frame a review shows in, into `out`, emptied first, in the windows'
    /// order: a window showing a review unified or a pane alone, and a pair of its panes once,
    /// by its first window. Windows the frame has no room for, and the side window, are left
    /// out.
    pub fn review_areas(&self, out: &mut Vec<ReviewArea>) {
        out.clear();
        let side = self.side_window();
        for window in &self.windows {
            let id = window.id();
            let Some(open) = self.reviews.holding(window.buffer()) else {
                continue;
            };
            let Some(outer) = (self.regions(id))
                .filter(|regions| regions.has_cells() && Some(id) != side)
                .map(|regions| regions.outer())
            else {
                continue;
            };
            let buffers = open.panes.as_ref().map(|panes| panes.buffers);
            let shows_pane = |id: WindowId| {
                let shown = self.window_by_id(id).map(|window| window.buffer());
                buffers.is_some_and(|buffers| shown.is_some_and(|shown| buffers.contains(&shown)))
            };
            let paired = self
                .pair_of(id)
                .filter(|&other| shows_pane(id) && shows_pane(other))
                .and_then(|other| self.regions(other))
                .map(|regions| regions.outer());
            let (columns, layout) = match paired {
                // The second of a pair is counted with the first.
                Some(theirs) if (theirs.row, theirs.column) < (outer.row, outer.column) => {
                    continue;
                }
                Some(theirs) if theirs.column > outer.column => (
                    theirs.column + theirs.columns - outer.column,
                    ReviewLayout::Panes(Way::Across),
                ),
                Some(_) => (outer.columns, ReviewLayout::Panes(Way::Down)),
                None => (outer.columns, ReviewLayout::Unified),
            };
            out.push(ReviewArea {
                window: id,
                review: open.id,
                columns,
                layout,
            });
        }
    }

    /// After a command: drops the panes no window shows, and moves the selected pane's cursors
    /// off filler rows.
    pub(in crate::editor) fn sync_review_panes(&mut self) {
        let unshown: Vec<(ReviewId, [BufferId; 2])> = (self.reviews.open.iter())
            .filter_map(|open| Some((open.id, open.panes.as_ref()?.buffers)))
            .filter(|(_, buffers)| {
                !(self.windows.iter()).any(|window| buffers.contains(&window.buffer()))
            })
            .collect();
        for (id, buffers) in unshown {
            if let Some(open) = self.reviews.get_mut(id) {
                open.panes = None;
            }
            for buffer in buffers {
                self.drop_internal_buffer(buffer);
            }
        }
        self.skip_review_fillers();
    }

    /// Moves each cursor of the selected window, if it shows a pane, off a filler row: on the
    /// way the primary cursor went since the last command, else back.
    fn skip_review_fillers(&mut self) {
        let index = self.selected;
        let id = self.windows[index].id();
        let buffer = self.windows[index].buffer();
        let Some((review, rows, last)) = self.reviews.holding(buffer).and_then(|open| {
            let panes = open.panes.as_ref()?;
            let side = panes.buffers.iter().position(|pane| *pane == buffer)?;
            Some((open.id, panes.rows[side].clone(), panes.last))
        }) else {
            return;
        };
        let (window, shown) = self.window_mut(index);
        let (text, display) = (shown.text(), shown.display());
        let line_of = |offset: usize| text.offset_to_point(offset).row;
        let now = line_of(window.selections.primary().cursor);
        let up = last.is_some_and(|(window, line)| window == id && now < line);
        let update = window.selections.update(|selection| {
            let line = line_of(selection.cursor);
            if !is_filler(&rows, line) {
                return Ok::<(), ()>(());
            }
            let Some(to) = off_filler(&rows, line, up) else {
                return Ok(());
            };
            selection.cursor = match selection.goal {
                Some(Goal::LineEnd) => motion::line_end(&display, to),
                Some(Goal::Column(column)) => {
                    display.display_point_to_offset(DisplayPoint::new(to, column), Bias::Left)
                }
                None => motion::line_start(&display, to),
            };
            Ok(())
        });
        let moved = update.changed;
        let now = line_of(window.selections.primary().cursor);
        if let Some(panes) = self
            .reviews
            .get_mut(review)
            .and_then(|open| open.panes.as_mut())
        {
            panes.last = Some((id, now));
        }
        if moved {
            self.follow_window(index);
        }
    }

    /// Where the window at `index` shows its review, by its buffer's rows: a filler row by the
    /// row of the other pane beside it, in a pair of the panes, else by the nearest row that is
    /// not one.
    fn review_places(&self, index: usize) -> Places {
        let window = &self.windows[index];
        let buffer = self.shown(window);
        let display = buffer.display();
        let cursor = window.cursor_point(&display);
        let goal = (window.selections.primary().goal).unwrap_or(Goal::Column(cursor.column));
        let top = display.offset_to_display_point(window.scroll().start).row;
        let rows = self.review_rows_of(window.buffer());
        let other = (self.pair_of(window.id()))
            .and_then(|other| self.window_by_id(other))
            .and_then(|other| self.review_rows_of(other.buffer()))
            .filter(|_| self.review_of(window.buffer()).is_some());
        let place = |line: usize| {
            let rows = rows?;
            let found = rows.place(line).or_else(|| other?.place(line));
            let found = found.or_else(|| rows.place(off_filler(rows, line, false)?))?;
            Some((Box::<[u8]>::from(found.0), found.1))
        };
        Places {
            cursor: place(cursor.row),
            top: place(top),
            goal,
            left: window.scroll().left,
        }
    }
}

/// The pane and its line that `place` of the file at `path` goes to: a side's line to its own
/// pane, any other row to the new pane, or to the old one when only it holds the row, as an
/// old side's card; the file's header when neither does.
fn pane_line(panes: &Panes, path: &[u8], place: Place) -> Option<(usize, usize)> {
    let order = match place {
        Place::Line { side, .. } => [side_index(side), side_index(side)],
        Place::Row { .. } => [NEW, OLD],
    };
    for side in order {
        if let Some(line) = panes.rows[side].find(path, place) {
            return Some((side, line));
        }
    }
    Some((order[0], panes.rows[order[0]].file_start(path)?))
}

fn is_filler(rows: &Rows, line: usize) -> bool {
    rows.at(line).is_some_and(|row| row.kind == RowKind::Filler)
}

/// The nearest line from `line` that is not a filler row, looked for upward first when `up`,
/// downward first otherwise.
fn off_filler(rows: &Rows, line: usize, up: bool) -> Option<usize> {
    let mut down = (line + 1..rows.lines()).filter(|&at| !is_filler(rows, at));
    let mut upward = (0..line).rev().filter(|&at| !is_filler(rows, at));
    if up {
        upward.next().or_else(|| down.next())
    } else {
        down.next().or_else(|| upward.next())
    }
}

#[cfg(test)]
#[path = "panes_tests.rs"]
mod tests;
