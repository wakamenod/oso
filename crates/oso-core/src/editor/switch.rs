//! Which buffer a window shows: going to one and back to where the window was in it, walking
//! the buffers with `M-,` and `M-/`, killing one, and the names that tell files with one name
//! apart (ADR 0088). Also what a prompt's preview shows and where a buffer's preview starts
//! (ADR 0088, ADR 0114), and the home directory a path is shown from.
//!
//! Must not depend on: the host's window, threads or I/O; the host frees killed buffers
//! and stops their loads.

use super::Editor;
use super::follow::clip_selections;
use crate::buffer::{self, Buffer, BufferId};
use crate::display_map::DisplaySnapshot;
use crate::selection::{MarkActive, SelectionSet};
use crate::window::{Place, Walk};

use super::SCRATCH;

/// The longest name a new buffer may have, in bytes.
pub const MAX_NAME_BYTES: usize = 255;

/// What a prompt's preview shows (ADR 0088).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preview {
    /// The buffer `id`, from the row that holds the offset `start`, with a cursor at the
    /// offset `cursor`.
    Buffer {
        id: BufferId,
        start: usize,
        cursor: usize,
    },
    /// A buffer that may hold secrets, or whose file is not judged yet, or a secret kill:
    /// nothing of it shows.
    Withheld,
    /// The kill ring's entry `entry`, as [`Editor::kill_preview`] gives its lines (ADR 0091).
    Text { entry: u64 },
    /// What the host read of a project search's row's file, as
    /// [`Editor::preview_excerpt_display`] gives it, at its own `revision`, with a cursor at the
    /// offset `cursor`, where the row's match starts (ADR 0114).
    Excerpt { revision: u64, cursor: usize },
}

/// Why a person's kill of a buffer did not happen (ADR 0088).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillRefused {
    /// An internal buffer, such as `*Messages*`, which holds the only copy of what it shows.
    Internal,
    /// A save of it is running, which would no longer count for the exit guard.
    Saving,
    /// It holds unsaved text. [`Editor::kill_asked`] names it, for a question whose `y`
    /// kills it with `anyway`.
    Modified,
    /// There is no such buffer.
    Gone,
}

impl Editor {
    /// Shows `buffer` in the tiled window `id`, selected or not, as [`Editor::show`] shows it in
    /// the selected one, as a restart shows each window's (ADR 0112). Does nothing for a window
    /// that is not tiled or a buffer that does not exist.
    pub fn show_in(&mut self, id: crate::window::WindowId, buffer: BufferId) {
        let Some(index) = self.window_index(id).filter(|&i| i < self.tiled_count()) else {
            return;
        };
        if self.buffer(buffer).is_some() {
            self.window_moves_on();
            self.window_shows(index, buffer);
        }
    }

    /// Makes the window at `index` show `buffer`, where it was there the last time, clipped to
    /// the text as it is now, with the region hidden, or from its start. Every change of what a
    /// window shows comes through here, so that what was around a buffer that may hold secrets,
    /// before or after it, is marked so.
    pub(super) fn window_shows(&mut self, index: usize, buffer: BufferId) {
        // A pair holds while each window shows what it was paired with (ADR 0122).
        self.leave_pair(index, buffer);
        // A search keeps to the buffer it began in (ADR 0082).
        self.isearch_abandon();
        self.replace_abandon();
        // Onto one of the user's buffers, so that the way back through internal ones no longer
        // says anything (ADR 0063).
        let users = self
            .buffer(buffer)
            .is_some_and(|shown| !shown.is_internal());
        if users {
            self.windows[index].way_back_mut().clear();
        }
        self.note_secret_shown();
        // A buffer it never showed starts where another window left it (ADR 0112).
        let elsewhere = if self.windows[index].has_shown(buffer) {
            None
        } else {
            self.place_elsewhere(index, buffer)
        };
        // The wrap it drops gives back what is left of the command's budget.
        let budget = self.gather_wrap_budget();
        self.wrap_pool = budget;
        if let Some(place) = self.windows[index].show(buffer).or(elsewhere) {
            // Its wrap first, for the top to stay where the place says (ADR 0103).
            self.sync_wrap(index);
            self.restore_place(index, place);
        }
        // At once, so that no caller lays the new buffer out at the old one's gutter, which
        // says how many lines that one has (ADR 0097).
        self.fit_gutter();
        self.note_secret_shown();
    }

    /// Where the tiled window selected most recently that showed `buffer`, other than the one at
    /// `index`, is in it, or left it: its primary cursor with its mark and the ring, and its
    /// view, as a split copies them (ADR 0088, ADR 0112). `None` if no other window showed it.
    fn place_elsewhere(&self, index: usize, buffer: BufferId) -> Option<Place> {
        let me = self.windows[index].id();
        let window = self
            .recent
            .iter()
            .filter(|id| **id != me)
            .filter_map(|id| self.window_by_id(*id))
            .find(|window| window.has_shown(buffer))?;
        let (selections, scroll) = if window.buffer() == buffer {
            (&window.selections, window.scroll())
        } else {
            let (_, place) = window.places().find(|(id, _)| *id == buffer)?;
            (&place.selections, place.scroll)
        };
        let primary = SelectionSet::from_parts(vec![*selections.primary()], 0)?;
        Some(Place {
            selections: primary.with_ring(*selections.mark_ring()),
            scroll,
        })
    }

    /// Puts the window at `index` back where `place` says, in the buffer it shows.
    fn restore_place(&mut self, index: usize, place: Place) {
        let Place {
            mut selections,
            scroll,
        } = place;
        let (window, shown) = self.window_mut(index);
        // The text may have changed since, by a reload or an edit elsewhere. A cursor that
        // stays where it was keeps its goal, as `C-n` then has it to go on, and the marks go
        // where a cursor can be too (ADR 0071).
        let display = shown.display();
        selections.set_mark_active(MarkActive::Inactive);
        clip_selections(&mut selections, &display, &[], false, None);
        window.restore_positions(selections, scroll, &display);
        self.follow_window(index);
    }

    /// Shows the buffer `count` before the one shown in the tiled window's order, the most
    /// recent first and then those it never showed, skipping internal buffers: `M-,`. A
    /// negative count goes the other way, as `M-/` does. Repeating it walks on through every
    /// buffer, as long as nothing else showed one.
    ///
    /// # Errors
    ///
    /// With no other buffer to show.
    pub fn walk_buffers(&mut self, count: isize) -> Result<(), &'static str> {
        let index = self.tiled_index();
        let walk = match self.windows[index].walk() {
            Some(walk) => walk.clone(),
            None => {
                let ring = self.walk_order(index);
                // From an internal buffer, which the walk leaves out, as if from just before
                // the most recent of the user's: `M-,` goes there.
                let on_internal = ring.first() != Some(&self.windows[index].buffer());
                let at = if on_internal && count > 0 {
                    ring.len().saturating_sub(1)
                } else {
                    0
                };
                if ring.is_empty() {
                    return Err("No other buffer");
                }
                Walk { ring, at }
            }
        };
        let len = walk.ring.len();
        let step = count.rem_euclid(isize::try_from(len).unwrap_or(isize::MAX));
        let step = usize::try_from(step).unwrap_or(0);
        let at = (walk.at + step) % len;
        let next = walk.ring[at];
        if next == self.windows[index].buffer() {
            return Err("No other buffer");
        }
        self.window_moves_on();
        self.window_shows(index, next);
        self.windows[index].places_mut().set_walk(Walk {
            ring: walk.ring,
            at,
        });
        Ok(())
    }

    /// The buffers a walk goes through from the window at `index`: the one it shows, those it
    /// showed before, the most recent first, then the others in the order they opened. Internal
    /// buffers are left out.
    fn walk_order(&self, index: usize) -> Vec<BufferId> {
        let window = &self.windows[index];
        let mut ring = Vec::new();
        let shown = std::iter::once(window.buffer());
        let shown_before = shown.chain(window.places().map(|(id, _)| id));
        let rest = self.buffers().map(|(id, _)| id);
        for id in shown_before.chain(rest) {
            let kept = self.buffer(id).is_some_and(|buffer| !buffer.is_internal());
            if kept && !ring.contains(&id) {
                ring.push(id);
            }
        }
        ring
    }

    /// The buffers a person goes between, the one the tiled window shows first, then those it
    /// showed before, the most recent first, then the rest in the order they opened: every
    /// buffer of the user's, `*Messages*`, and the list of emergency copies and `*Agent*` once
    /// made. A prompt's buffer and a copy being looked at are left out (ADR 0088).
    pub fn buffers_by_recency(&self) -> Vec<BufferId> {
        let window = &self.windows[self.tiled_index()];
        let listed = |id: BufferId| {
            self.buffer(id)
                .is_some_and(|buffer| self.listed(id, buffer))
        };
        let mut order = Vec::new();
        let shown = std::iter::once(window.buffer()).chain(window.places().map(|(id, _)| id));
        let all = self.buffers.iter().map(|(id, _)| *id);
        for id in shown.chain(all) {
            if listed(id) && !order.contains(&id) {
                order.push(id);
            }
        }
        order
    }

    /// The buffer a person may go to that is named `name`, as [`Editor::buffers_by_recency`]
    /// lists them.
    pub fn buffer_named(&self, name: &str) -> Option<BufferId> {
        // Names are unique, so one pass finds it, whatever the order.
        self.buffers
            .iter()
            .find(|(id, buffer)| buffer.name() == name && self.listed(*id, buffer))
            .map(|(id, _)| *id)
    }

    /// Whether a person goes to `buffer`: one of the user's, or the internal buffers a person
    /// reads, `*Messages*`, the list of emergency copies and `*Agent*` (ADR 0088).
    fn listed(&self, id: BufferId, buffer: &Buffer) -> bool {
        !buffer.is_internal()
            || id == self.messages_buffer
            || Some(id) == self.copies_buffer
            || Some(id) == self.agent_log_buffer()
    }

    /// Shows the buffer named `name`, making an empty one with no file if none is (ADR 0088).
    /// An empty name shows nothing.
    ///
    /// # Errors
    ///
    /// For a new buffer's name of more than [`MAX_NAME_BYTES`], or with a control character,
    /// or one an internal buffer a person does not go to holds, such as `*Session explain*`:
    /// a second buffer of that name could pass for oso's own (ADR 0121).
    pub fn switch_to_named(&mut self, name: &str) -> Result<(), &'static str> {
        if name.is_empty() {
            return Ok(());
        }
        let id = match self.buffer_named(name) {
            Some(id) => id,
            None if name.len() > MAX_NAME_BYTES => return Err("That name is too long"),
            None if name.chars().any(char::is_control) => {
                return Err("A buffer's name holds no control characters");
            }
            None if self.buffers.iter().any(|(_, buffer)| buffer.name() == name) => {
                return Err("That name is one of oso's own buffers");
            }
            None => self.add_buffer(Buffer::new(name)),
        };
        self.show(id);
        Ok(())
    }

    /// What the innermost prompt's preview shows, if it has one and a candidate is selected:
    /// the buffer that candidate names, from where the tiled window last was in it, or that
    /// it may hold secrets, which it never shows (ADR 0088).
    pub fn preview(&self) -> Option<Preview> {
        // A project's search previews its row's file, never by a buffer's name (ADR 0114).
        if self.searches_project() {
            return self.search_preview();
        }
        // `M-y` previews the kill its selected row names, never a secret one (ADR 0091).
        if self.kill_list_open() {
            let entry = self.selected_kill()?;
            return Some(if entry.is_secret() {
                Preview::Withheld
            } else {
                Preview::Text { entry: entry.id() }
            });
        }
        // `C-.` previews the buffer it lists the marks of, unless it may hold secrets, as any
        // preview.
        if let Some((id, cursor)) = self.selected_mark() {
            return self.buffer_preview_at(id, cursor);
        }
        let prompt = self.prompt()?;
        let candidates = prompt
            .candidates()
            .filter(|candidates| candidates.previews())?;
        let name = candidates.selected_candidate()?.text();
        let id = self.buffer_named(name)?;
        let buffer = self.buffer(id)?;
        if buffer.verdict().is_withheld() {
            return Some(Preview::Withheld);
        }
        let window = &self.windows[self.tiled_index()];
        let (selections, scroll) = if window.buffer() == id {
            (&window.selections, window.scroll())
        } else {
            let place = window.places().find(|(other, _)| *other == id);
            match place {
                Some((_, place)) => (&place.selections, place.scroll),
                None => {
                    return Some(Preview::Buffer {
                        id,
                        start: 0,
                        cursor: 0,
                    });
                }
            }
        };
        let len = buffer.text().len();
        let (cursor, start) = (selections.primary().cursor.min(len), scroll.start.min(len));
        Some(Preview::Buffer { id, start, cursor })
    }

    /// The preview of the open buffer `id` with its cursor at `cursor`, from the line about
    /// halfway up the preview's rows from the cursor's, as `C-.` and a project's search show
    /// one; or that it may hold secrets, which it never shows.
    pub(super) fn buffer_preview_at(&self, id: BufferId, cursor: usize) -> Option<Preview> {
        let buffer = self.buffer(id)?;
        if buffer.verdict().is_withheld() {
            return Some(Preview::Withheld);
        }
        Some(Preview::Buffer {
            id,
            start: self.preview_start(&buffer.display(), cursor),
            cursor,
        })
    }

    /// Where a preview whose cursor is at `cursor` starts: the line about halfway up the
    /// preview's rows from the cursor's.
    pub(super) fn preview_start(&self, display: &DisplaySnapshot, cursor: usize) -> usize {
        let rows = self
            .prompt_layout()
            .map_or(0, |layout| layout.regions.preview.rows);
        let row = display.offset_to_display_point(cursor).row;
        crate::motion::line_start(display, row.saturating_sub(rows / 2))
    }

    /// The home directory, which `~` stands for, as the host read it once at startup.
    pub fn home(&self) -> Option<&std::path::Path> {
        self.home.as_deref()
    }

    /// Tells the editor the home directory, read once by the host at startup.
    pub fn set_home(&mut self, home: Option<std::path::PathBuf>) {
        self.home = home;
    }

    /// Kills `id` as a person asked: a modified buffer only with `anyway`, after a question
    /// about that buffer, never while it saves, and never an internal one (ADR 0088).
    ///
    /// # Errors
    ///
    /// Why it stays.
    pub fn kill_by_person(&mut self, id: BufferId, anyway: bool) -> Result<(), KillRefused> {
        let buffer = self.buffer(id).ok_or(KillRefused::Gone)?;
        if buffer.is_internal() {
            return Err(KillRefused::Internal);
        }
        if buffer.is_saving() {
            return Err(KillRefused::Saving);
        }
        let modified = buffer.is_modified();
        let asked = self.kill_asked.take();
        if modified && !(anyway && asked == Some(id)) {
            self.kill_asked = Some(id);
            return Err(KillRefused::Modified);
        }
        self.kill_buffer(id);
        Ok(())
    }

    /// The buffer a question about killing it with unsaved text was asked of, until it is
    /// answered or the buffer goes.
    pub fn kill_asked(&self) -> Option<BufferId> {
        self.kill_asked
    }

    /// Removes a buffer. Each window that showed it shows the buffer it showed before it, or
    /// `*scratch*`, which is created again if needed. An internal buffer stays. What it held is
    /// kept for [`Editor::take_freed`], so that the host frees it off the UI thread.
    pub fn kill_buffer(&mut self, id: BufferId) {
        if self.buffer(id).is_some_and(Buffer::is_internal) {
            return;
        }
        if self.buffer(id).is_none() {
            return;
        }
        let shown = self.windows.iter().any(|window| window.buffer() == id);
        if shown {
            self.isearch_abandon();
            self.replace_abandon();
            self.window_moves_on();
            // Before it goes, as what it was cannot be asked after.
            self.note_secret_shown();
        }
        for index in 0..self.windows.len() {
            if self.windows[index].buffer() != id {
                continue;
            }
            // One of the user's: an internal buffer is reached by its own key.
            let back = self.windows[index]
                .places()
                .map(|(other, _)| other)
                .find(|other| {
                    self.buffer(*other)
                        .is_some_and(|buffer| !buffer.is_internal())
                });
            let back = match back {
                Some(back) => back,
                None => self.scratch_other_than(id),
            };
            self.window_shows(index, back);
        }
        for window in &mut self.windows {
            window.places_mut().forget(id);
        }
        if self.kill_asked == Some(id) {
            self.kill_asked = None;
        }
        if let Some(position) = self.buffers.iter().position(|(other, _)| *other == id) {
            let (_, buffer) = self.buffers.remove(position);
            let name = buffer
                .path()
                .map_or_else(|| buffer.name().to_owned(), buffer::file_base);
            self.freed.push(buffer);
            self.name_files_like(&name);
        }
        if shown {
            self.note_secret_shown();
        }
    }

    /// What killed buffers held, for the host to free off the UI thread, as freeing a large
    /// one's trees takes tens of milliseconds a GB.
    pub fn take_freed(&mut self) -> Vec<Buffer> {
        std::mem::take(&mut self.freed)
    }

    /// The `*scratch*` buffer, created again if needed.
    pub(super) fn scratch(&mut self) -> BufferId {
        self.scratch_other_than(None)
    }

    /// A `*scratch*` buffer that is not `going`, created if needed.
    fn scratch_other_than(&mut self, going: impl Into<Option<BufferId>>) -> BufferId {
        let going = going.into();
        let scratch = self.buffers.iter().find(|(id, buffer)| {
            Some(*id) != going && buffer.name() == SCRATCH && buffer.path().is_none()
        });
        match scratch {
            Some((scratch, _)) => *scratch,
            None => self.add_buffer(Buffer::new(SCRATCH)),
        }
    }

    /// Names the buffers whose file is called `name` so that no two share a name, and none
    /// takes the name of a buffer with no file (ADR 0088). Only that group can change when a
    /// buffer called `name` comes or goes, so this costs one pass over the buffers.
    pub(super) fn name_files_like(&mut self, name: &str) {
        let taken: Vec<String> = self
            .buffers
            .iter()
            .filter(|(_, buffer)| buffer.path().is_none() && buffer.name() == name)
            .map(|(_, buffer)| buffer.name().to_owned())
            .collect();
        let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
        let group: Vec<usize> = (0..self.buffers.len())
            .filter(|index| {
                self.buffers[*index]
                    .1
                    .path()
                    .is_some_and(|path| buffer::file_base(path) == name)
            })
            .collect();
        let paths: Vec<&std::path::Path> = group
            .iter()
            .filter_map(|index| self.buffers[*index].1.path())
            .collect();
        let names = buffer::distinct_names(&paths, &taken);
        for (index, name) in group.into_iter().zip(names) {
            self.buffers[index].1.set_name(name);
        }
    }
}

#[cfg(test)]
mod tests;
