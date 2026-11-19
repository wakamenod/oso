//! Jumping to the source from a review (ADR 0122, "Jump to the source", step 3, part three):
//! what `RET` and `C-u RET` ask, carried out. A line of a review of the work tree opens its
//! file at that line carried through the edits since the reading, counted on the review's
//! thread (Q5); a side the work tree does not hold, as a commit's new side or any old side,
//! shows read-only in a buffer of its own (Q4), built on that thread too and put in at O(1).
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here reads a file, diffs or
//! builds a text: the review's thread does, and its answer comes by the jump's ticket.
//!
//! - The file or side goes to the review layout's slot (ADR 0160, `slot`). Outside the layout,
//!   it goes to another window, as Emacs's `diff-goto-source` does (Q2): the next tiled window
//!   that shows none of the review's buffers, its panes and files pane too, else one split below
//!   the selected, outside its pair, so the panes stay (Q3).
//! - Only the latest jump asked counts, and only while the window it was asked in is still
//!   selected, shows the review and holds nothing that waits on it, as a prompt or a search: an
//!   answer that comes after the person moved on is dropped. Trust and a secret open buffer are
//!   looked at again when it comes.
//! - A file opens as the agent channel opens one, refused with a second name, and only once the
//!   review's thread found it a regular file reached through no link.
//! - A review keeps the side buffers a window shows and the one asked for; the rest go, and those
//!   no window shows go with the review.
//! - Nothing of a file that may hold secrets is opened: not its side, nor its file from the
//!   review (Q6). A side is not shown either while the file's open buffer may hold secrets, as
//!   the agent channel may read a buffer of oso's own.
//! - The line is found in the reading the review shows: a file whose newer reading has not gone
//!   in yet waits, as a comment does.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::{Editor, ReviewId};
use oso_core::frame::Way;
use oso_core::review::Side;
use oso_core::selection::SelectionSet;
use oso_core::window::WindowId;

use super::scope::scope_label;
use crate::app::App;
use crate::commands::review::{MAX_NAME, one_line, open_buffer_of, place_of};
use crate::review::jump::{Ask, Carried, Jump, Jumped, Landed, across};
use crate::review::{Content, Reading, Request, Scope, Status};

/// What the review says of a file that may hold secrets, which it opens nothing of (Q6).
const WITHHELD: &str =
    "This file may hold secrets, so the review opens nothing of it: C-x C-f opens it";

/// What the review says once its project is no longer trusted.
const REVOKED: &str = "This project is no longer trusted: M-x trust-project, then g";

/// A jump asked of the review's thread, waiting for its answer.
pub(super) struct Jumping {
    ticket: u64,
    /// The window it was asked in.
    window: WindowId,
    to: To,
}

/// Where a jump's answer goes.
enum To {
    /// The work tree's file at `place`, at the line the answer gives; `line` is the line as the
    /// review read it.
    File { place: PathBuf, line: u32 },
    /// A buffer named `name`, of `mode`, holding a side of the file at `place`, at `line`.
    Side {
        name: String,
        mode: Mode,
        line: u32,
        place: PathBuf,
    },
}

/// The name of the buffer showing `side` of `path` in a review of `scope`, its bidirectional
/// and invisible characters escaped, as a git status's names are.
fn side_name(path: &[u8], side: Side, scope: &Scope) -> String {
    let path = crate::vcs::agent::one_line(&String::from_utf8_lossy(path), MAX_NAME);
    let side = match side {
        Side::Old => "old",
        Side::New => "new",
    };
    let scope = crate::vcs::agent::one_line(&scope_label(scope), MAX_NAME);
    format!("*{side}: {path} \u{b7} {scope}*")
}

impl App {
    /// Goes to line `number` of `side` of the file at `path` in the review `review`: its
    /// source, or with `old_side` its old side, read-only.
    pub(super) fn visit_review_line(
        &mut self,
        review: ReviewId,
        path: &[u8],
        (side, number): (Side, u32),
        old_side: bool,
    ) {
        let Some(index) = self.reviews.index(review) else {
            return;
        };
        match self.plan_jump(index, path, (side, number), old_side) {
            Ok((ask, to)) => {
                let ticket = self.reviews.serial();
                let window = self.editor.window().id();
                self.reviews.open[index].jumping = Some(Jumping { ticket, window, to });
                self.ask_review(index, Request::Jump(Jump { ticket, ask }));
            }
            Err(why) => self.editor.message(why),
        }
        self.request_redraw();
    }

    /// What to ask the review's thread for the jump, and where its answer goes, or why there is
    /// nowhere to go.
    fn plan_jump(
        &self,
        index: usize,
        path: &[u8],
        (side, number): (Side, u32),
        old_side: bool,
    ) -> Result<(Ask, To), String> {
        let running = &self.reviews.open[index];
        if running.revoked {
            return Err(REVOKED.into());
        }
        let reading = running
            .decisions
            .reading(path)
            .ok_or("This file is being read: try in a moment")?;
        let root = running.root.path();
        if matches!(reading.content, Content::Withheld) || reading.entry.verdict(root).is_withheld()
        {
            return Err(WITHHELD.into());
        }
        let place =
            place_of(root, path).ok_or("That file's name is not one this system can open")?;
        let checked_out = (running.pr.as_ref()).is_some_and(|found| found.head_checked_out);
        let on_disk = running.scope.new_side_on_disk(checked_out);
        let Content::Text {
            shape, old, new, ..
        } = &reading.content
        else {
            return not_text(reading, place, old_side || !on_disk);
        };
        // The reading the rows were laid out from: one read again since waits for its text.
        let shown = (self.editor.review(running.id)).and_then(|model| model.file(path));
        if !shown.is_some_and(|shown| Arc::ptr_eq(shown, shape)) {
            return Err("This file is being read again: try in a moment".into());
        }
        let want = if old_side { Side::Old } else { Side::New };
        let line = if side == want {
            number
        } else {
            across(shape.hunks(), side, number)
        };
        match reading.entry.status {
            Status::Deleted if !old_side => {
                return Err("Deleted in this review: C-u RET shows the old side".into());
            }
            Status::Added | Status::Copied if old_side && old.is_empty() => {
                return Err("Added in this review: it has no old side".into());
            }
            _ => {}
        }
        if !old_side && on_disk {
            let now = open_buffer_of(&self.editor, &place)
                .and_then(|buffer| self.editor.buffer(buffer))
                .map(|buffer| buffer.text().clone());
            let ask = Ask::Carry {
                path: path.into(),
                new: Arc::clone(new),
                line,
                now,
            };
            return Ok((ask, To::File { place, line }));
        }
        if secret_buffer(&self.editor, &place) {
            return Err(WITHHELD.into());
        }
        let bytes = Arc::clone(if old_side { old } else { new });
        let to = To::Side {
            name: side_name(path, want, &running.scope),
            mode: Mode::for_path(&place),
            line,
            place,
        };
        Ok((Ask::Side { bytes }, to))
    }

    /// Puts in where the jump `jumped` landed, if it is the review's latest, the person is
    /// still in the window it was asked in, and nothing they do there waits on the review, as a
    /// prompt or a search does.
    pub(super) fn review_jumped(&mut self, index: usize, jumped: Jumped) {
        let running = &mut self.reviews.open[index];
        let latest =
            (running.jumping.as_ref()).is_some_and(|jumping| jumping.ticket == jumped.ticket);
        if !latest {
            return self.freer.free(jumped);
        }
        let Some(jumping) = running.jumping.take() else {
            return;
        };
        let review = running.id;
        let here = self.editor.window();
        let still =
            here.id() == jumping.window && self.editor.review_of(here.buffer()) == Some(review);
        if !still || self.editor.review_waits() {
            return self.freer.free(jumped);
        }
        let landed = match jumped.landed {
            Ok(landed) => landed,
            Err(why) => {
                let why = one_line(&why, super::MAX_MESSAGE);
                return self.editor.message(why);
            }
        };
        // What may have changed while the thread looked.
        if self.reviews.open[index].revoked {
            self.freer.free(landed);
            return self.editor.message(REVOKED);
        }
        if let To::Side { place, .. } = &jumping.to
            && secret_buffer(&self.editor, place)
        {
            self.freer.free(landed);
            return self.editor.message(WITHHELD);
        }
        // The review layout's slot, else another window or one split below (Q2, Q3).
        if !self.select_jump_slot()
            && let Err(why) = select_source_window(&mut self.editor, review)
        {
            self.freer.free(landed);
            return self.editor.message(why);
        }
        match (jumping.to, landed) {
            (To::File { place, line: read }, Landed::Line { line, carried }) => {
                // Loaded as the agent channel loads a file: one with a second name, as a hard
                // link swapped in after the thread looked gives, is refused, and a link is
                // judged by where it leads (ADR 0044, ADR 0085).
                (self.editor).request_visit_at(
                    place.clone(),
                    oso_core::load::UNASKED,
                    line as usize,
                    0,
                );
                self.serve_files_of(Some(&place));
                match carried {
                    Carried::Kept => {}
                    Carried::Changed => self.editor.message(format!(
                        "Line {read} changed since the review read it: at the start of what replaced it"
                    )),
                    Carried::Not(why) => self
                        .editor
                        .message(format!("At line {read} as the review read it: {why}")),
                }
            }
            (
                To::Side {
                    name, mode, line, ..
                },
                Landed::Side(text),
            ) => {
                let buffer = self.side_buffer(index, &name, mode);
                // Kept, at O(1), so that the text it replaces is freed off the UI thread.
                let before = (self.editor.buffer(buffer))
                    .map(|shown| (shown.text().clone(), shown.display()));
                let left = self.editor.set_internal_reloaded(buffer, *text);
                self.freer.free((before, left));
                let window = self.editor.window().id();
                self.editor.show_in(window, buffer);
                put_cursor_on_line(&mut self.editor, line);
                (self.editor).message("Read-only: a side as the review read it");
            }
            (_, landed) => self.freer.free(landed),
        }
        self.request_redraw();
    }

    /// The review at `index`'s side buffer named `name`, or a new one of `mode`. Its other side
    /// buffers that no window shows go, freed off the UI thread, so a review holds the sides
    /// shown and the one asked for.
    fn side_buffer(&mut self, index: usize, name: &str, mode: Mode) -> BufferId {
        let kept = self.drop_unshown_sides(index, Some(name));
        let found = (kept.iter().copied())
            .find(|id| (self.editor.buffer(*id)).is_some_and(|buffer| buffer.name() == name));
        let id = found.unwrap_or_else(|| self.editor.add_internal_buffer(name, mode));
        let sides = &mut self.reviews.open[index].sides;
        *sides = kept;
        if found.is_none() {
            sides.push(id);
        }
        id
    }

    /// Removes the review at `index`'s side buffers that no window shows, but the one named
    /// `keep`, and returns those left.
    pub(super) fn drop_unshown_sides(&mut self, index: usize, keep: Option<&str>) -> Vec<BufferId> {
        let mut shown = Vec::new();
        self.editor.shown_ranges(&mut shown);
        let sides = std::mem::take(&mut self.reviews.open[index].sides);
        let mut kept = Vec::with_capacity(sides.len());
        for id in sides {
            let Some(buffer) = self.editor.buffer(id) else {
                continue;
            };
            let wanted = keep.is_some_and(|keep| buffer.name() == keep);
            if wanted || shown.iter().any(|(shown, _)| *shown == id) {
                kept.push(id);
            } else if let Some(gone) = self.editor.remove_internal_buffer(id) {
                self.freer.free(gone);
            }
        }
        kept
    }
}

/// Whether the open buffer of the file at `place` may hold secrets, so that no side of it shows.
fn secret_buffer(editor: &Editor, place: &Path) -> bool {
    open_buffer_of(editor, place)
        .and_then(|buffer| editor.buffer(buffer))
        .is_some_and(oso_core::buffer::Buffer::is_secret)
}

/// Puts the selected window's cursor alone at the start of line `line`, from 1, or the last.
fn put_cursor_on_line(editor: &mut Editor, line: u32) {
    let (window, shown) = editor.current_mut();
    let text = shown.text();
    let row = (line.max(1) as usize - 1).min(text.max_point().row);
    window.selections = SelectionSet::new(text.line_start(row));
    editor.follow_cursor();
}

/// Selects the window a jump from the review `review` shows its source in: the next tiled one
/// in `C-x o`'s order that shows none of the review's buffers, else a new one split below the
/// selected window, outside its pair (Q2, Q3). The windows are looked at, not selected, so the
/// order of the windows selected last keeps all but the one chosen.
pub(super) fn select_source_window(editor: &mut Editor, review: ReviewId) -> Result<(), String> {
    let me = editor.window().id();
    let side = editor.side_window();
    let ours = |buffer: BufferId| {
        editor.review_of(buffer) == Some(review) || editor.review_files(review) == Some(buffer)
    };
    // As `C-x o` goes: the windows the frame has room for, the side window left out.
    let order: Vec<WindowId> = (editor.frame().windows().iter())
        // One the frame has no room for has no modeline row.
        .filter(|(id, regions)| regions.modeline.rows > 0 && Some(*id) != side)
        .map(|(id, _)| *id)
        .collect();
    let at = order.iter().position(|id| *id == me).unwrap_or(0);
    let found = (order.iter().cycle().skip(at + 1).take(order.len()))
        .copied()
        .filter(|id| *id != me)
        .find(|id| (editor.window_by_id(*id)).is_some_and(|window| !ours(window.buffer())));
    match found {
        Some(id) => {
            editor.select_window(id);
            Ok(())
        }
        None => editor
            .split_window(Way::Down, true)
            .map(|_| ())
            .map_err(|error| error.to_string()),
    }
}

/// Where a jump to a file the review shows no text of goes: the work tree's file, when it is
/// on disk, as "too large to show: `RET` opens it" says, once the review's thread looked at it
/// as a carry reads one; nowhere for a side the review did not read.
fn not_text(reading: &Reading, place: PathBuf, side_wanted: bool) -> Result<(Ask, To), String> {
    if side_wanted {
        return Err("The review read no text of this file to show".into());
    }
    match reading.content {
        Content::Submodule => Err("A submodule: nothing to open".into()),
        _ if reading.entry.status == Status::Deleted => {
            Err("Deleted in this review: nothing to open".into())
        }
        _ => {
            let path = reading.entry.path.clone();
            Ok((Ask::Look { path }, To::File { place, line: 1 }))
        }
    }
}

impl std::fmt::Debug for Jumping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jumping")
            .field("ticket", &self.ticket)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "visit_tests.rs"]
mod tests;
