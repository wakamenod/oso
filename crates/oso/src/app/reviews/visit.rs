//! Jumping to the source from a review (ADR 0122, "Jump to the source", step 3, part three):
//! what `RET`, `M-RET` and `C-u RET` ask, carried out. `RET` opens the checkout's file at the
//! line carried through the edits since the reading, counted on the review's thread (Q5), or a
//! commit's side when the checkout has no such file; the other two show a side as the review
//! read it, read-only in a buffer of its own (Q4), built on that thread too and put in at O(1).
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
use oso_core::editor::{Editor, ReviewId, VisitTo};
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
    /// The files whose open buffers must hold no secrets: all but a work tree review's own.
    guards: Vec<PathBuf>,
    /// A side a layout put back asked for (ADR 0184): the rows above its line, and whether the
    /// slot stays selected.
    then: Option<(u32, bool)>,
}

impl Jumping {
    /// The jump puts back a layout's side (ADR 0184): `above` rows above its line, and the
    /// slot selected with `select`, else the window it was asked in.
    pub(super) fn put_back(&mut self, above: u32, select: bool) {
        self.then = Some((above, select));
    }
}

/// A buffer showing a side of a file of the review, found again by the path and the side: its
/// name is cut, so two paths may share one.
pub(super) struct SideBuffer {
    id: BufferId,
    path: Box<[u8]>,
    side: Side,
}

/// Where a jump's answer goes.
enum To {
    /// The checkout's file at `place`, at the line the answer gives; `line` is the line as the
    /// review read it. `commit` says what a review whose new side is a commit adds.
    File {
        place: PathBuf,
        line: u32,
        commit: Option<FromCommit>,
    },
    Side(SideTo),
}

/// A jump to the checkout's file from a review whose new side is a commit.
struct FromCommit {
    /// What the echo area says of the file.
    said: String,
    /// Where the commit's side goes when the checkout has no such file.
    or_side: Option<SideTo>,
}

/// A buffer named `name`, of `mode`, holding `side` of the file at `path`, at `line`.
struct SideTo {
    name: String,
    mode: Mode,
    path: Box<[u8]>,
    side: Side,
    line: u32,
}

/// What the echo area says of the checkout's file a review of a commit goes to; `pull` is the
/// pull request the commit is of, if its head is not checked out here.
fn now_said(pull: Option<u64>) -> String {
    match pull {
        Some(number) => format!(
            "#{number}'s head is not checked out here: this checkout's file \u{b7} M-RET shows #{number}'s"
        ),
        None => "The file as checked out now \u{b7} M-RET shows it as the review read it".into(),
    }
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

/// The mode of `side` of the file at `path`, which came from `old_path` if that is named.
fn side_mode(path: &[u8], old_path: Option<&[u8]>, side: Side) -> Mode {
    let named = match side {
        Side::Old => old_path.unwrap_or(path),
        Side::New => path,
    };
    crate::git::os_path(named).map_or(Mode::Fundamental, |name| Mode::for_path(Path::new(name)))
}

/// The path and side the buffer `buffer` of `sides` shows, if it is one of them.
pub(super) fn side_of(sides: &[SideBuffer], buffer: BufferId) -> Option<(Box<[u8]>, Side)> {
    (sides.iter())
        .find(|shown| shown.id == buffer)
        .map(|shown| (shown.path.clone(), shown.side))
}

/// The buffer of `sides` showing `side` of the file at `path`.
pub(super) fn side_shown(sides: &[SideBuffer], path: &[u8], side: Side) -> Option<BufferId> {
    (sides.iter())
        .find(|shown| shown.side == side && *shown.path == *path)
        .map(|shown| shown.id)
}

impl App {
    /// Goes to line `number` of `side` of the file at `path` in the review `review`,
    /// where `to` says.
    pub(super) fn visit_review_line(
        &mut self,
        review: ReviewId,
        path: &[u8],
        (side, number): (Side, u32),
        to: VisitTo,
    ) {
        let Some(index) = self.reviews.index(review) else {
            return;
        };
        match self.plan_jump(index, path, (side, number), to) {
            Ok((ask, to, guards)) => {
                let ticket = self.reviews.serial();
                let window = self.editor.window().id();
                self.reviews.open[index].jumping = Some(Jumping {
                    ticket,
                    window,
                    to,
                    guards,
                    then: None,
                });
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
        to: VisitTo,
    ) -> Result<(Ask, To, Vec<PathBuf>), String> {
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
        let source = to == VisitTo::Source;
        let guards = if source && on_disk {
            Vec::new()
        } else {
            guards_of(reading, root, &place)
        };
        if secret_buffer(&self.editor, &guards) {
            return Err(WITHHELD.into());
        }
        let said = (!on_disk).then(|| now_said(self.pull_elsewhere(index)));
        let Content::Text {
            shape, old, new, ..
        } = &reading.content
        else {
            let (ask, to) = not_text(reading, place, !source, said)?;
            return Ok((ask, to, guards));
        };
        // The reading the rows were laid out from: one read again since waits for its text.
        let shown = (self.editor.review(running.id)).and_then(|model| model.file(path));
        if !shown.is_some_and(|shown| Arc::ptr_eq(shown, shape)) {
            return Err("This file is being read again: try in a moment".into());
        }
        let want = match to {
            VisitTo::Source => Side::New,
            VisitTo::Side => side,
            VisitTo::OldSide => Side::Old,
        };
        let line = if side == want {
            number
        } else {
            across(shape.hunks(), side, number)
        };
        if let Some(why) = no_side(reading.entry.status, want, old) {
            return Err(why.into());
        }
        let side_to = || SideTo {
            name: side_name(path, want, &running.scope),
            mode: side_mode(path, reading.entry.old_path.as_deref(), want),
            path: path.into(),
            side: want,
            line,
        };
        if !source {
            let bytes = Arc::clone(if want == Side::Old { old } else { new });
            return Ok((Ask::Side { bytes }, To::Side(side_to()), guards));
        }
        let commit = said.map(|said| FromCommit {
            said,
            or_side: Some(side_to()),
        });
        let now = open_text(&self.editor, &place);
        let ask = Ask::Carry {
            path: path.into(),
            new: Arc::clone(new),
            line,
            now,
            or_side: commit.is_some(),
        };
        let to = To::File {
            place,
            line,
            commit,
        };
        Ok((ask, to, guards))
    }

    /// The number of the pull request the review at `index` is of, or of a commit of, if its
    /// head was not checked out here when last asked.
    fn pull_elsewhere(&self, index: usize) -> Option<u64> {
        let running = &self.reviews.open[index];
        let whole = match running.pull {
            Some((pull, _)) => &self.reviews.open[self.reviews.index(pull)?],
            None => running,
        };
        let found = whole.pr.as_ref()?;
        (!found.head_checked_out).then_some(found.number)
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
        if secret_buffer(&self.editor, &jumping.guards) {
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
            (
                To::File {
                    place,
                    line: read,
                    commit,
                },
                Landed::Line { line, carried },
            ) => self.show_file(place, read, (line, carried), commit),
            (
                To::File {
                    commit:
                        Some(FromCommit {
                            or_side: Some(to), ..
                        }),
                    ..
                },
                Landed::Side(text),
            ) => self.show_side(
                index,
                to,
                *text,
                "Not in this checkout now: the side as the review read it, read-only",
            ),
            (To::Side(to), Landed::Side(text)) => {
                self.show_side(index, to, *text, "Read-only: a side as the review read it");
                // A layout's side put back: its rows above, and the side that was selected.
                if let Some((above, select)) = jumping.then {
                    let (window, shown) = self.editor.current();
                    let row = shown
                        .text()
                        .offset_to_point(window.selections.primary().cursor)
                        .row;
                    let (slot, line) = (window.id(), u32::try_from(row + 1).unwrap_or(u32::MAX));
                    super::slot::put_at(&mut self.editor, slot, (line, 0), above);
                    if !select {
                        self.editor.select_window(jumping.window);
                    }
                }
            }
            (_, landed) => self.freer.free(landed),
        }
        self.request_redraw();
    }

    /// Opens the checkout's file at `place` in the selected window, at `line`, where the review's
    /// line `read` was `carried` to.
    fn show_file(
        &mut self,
        place: PathBuf,
        read: u32,
        (line, carried): (u32, Carried),
        commit: Option<FromCommit>,
    ) {
        // Loaded as the agent channel loads a file: one with a second name, as a hard
        // link swapped in after the thread looked gives, is refused, and a link is
        // judged by where it leads (ADR 0044, ADR 0085).
        (self.editor).request_visit_at(place.clone(), oso_core::load::UNASKED, line as usize, 0);
        self.serve_files_of(Some(&place));
        match (carried, commit) {
            (Carried::Kept, None) => {}
            (Carried::Kept, Some(commit)) => self.editor.message(commit.said),
            (Carried::Changed, _) => self.editor.message(format!(
                "Line {read} changed since the review read it: at the start of what replaced it"
            )),
            (Carried::Not(why), _) => self
                .editor
                .message(format!("At line {read} as the review read it: {why}")),
        }
    }

    /// Shows `text` in the selected window, as `to` says, and says `said`.
    fn show_side(
        &mut self,
        index: usize,
        to: SideTo,
        text: oso_core::load::Reloaded,
        said: &'static str,
    ) {
        let buffer = self.side_buffer(index, (&to.path, to.side), to.name, to.mode);
        // Kept, at O(1), so that the text it replaces is freed off the UI thread.
        let before =
            (self.editor.buffer(buffer)).map(|shown| (shown.text().clone(), shown.display()));
        let left = self.editor.set_internal_reloaded(buffer, text);
        self.freer.free((before, left));
        let window = self.editor.window().id();
        self.editor.show_in(window, buffer);
        put_cursor_on_line(&mut self.editor, to.line);
        self.editor.message(said);
    }

    /// The review at `index`'s buffer of `side` of the file at `path`, or a new one named
    /// `name`, of `mode`. Its other side buffers that no window shows go, freed off the UI
    /// thread, so a review holds the sides shown and the one asked for.
    fn side_buffer(
        &mut self,
        index: usize,
        (path, side): (&[u8], Side),
        name: String,
        mode: Mode,
    ) -> BufferId {
        self.drop_unshown_sides(index, Some((path, side)));
        let sides = &self.reviews.open[index].sides;
        if let Some(id) = side_shown(sides, path, side) {
            return id;
        }
        let id = self.editor.add_side_buffer(name, mode);
        self.reviews.open[index].sides.push(SideBuffer {
            id,
            path: path.into(),
            side,
        });
        id
    }

    /// Removes the review at `index`'s side buffers that no window shows, but the one of `keep`,
    /// a path and a side.
    pub(super) fn drop_unshown_sides(&mut self, index: usize, keep: Option<(&[u8], Side)>) {
        let mut shown = Vec::new();
        self.editor.shown_ranges(&mut shown);
        let sides = std::mem::take(&mut self.reviews.open[index].sides);
        let mut kept = Vec::with_capacity(sides.len());
        for side in sides {
            if self.editor.buffer(side.id).is_none() {
                continue;
            }
            let wanted = keep.is_some_and(|(path, of)| *side.path == *path && side.side == of);
            if wanted || shown.iter().any(|(shown, _)| *shown == side.id) {
                kept.push(side);
            } else if let Some(gone) = self.editor.remove_internal_buffer(side.id) {
                self.freer.free(gone);
            }
        }
        self.reviews.open[index].sides = kept;
    }
}

/// The text of the open buffer of the file at `place`, an O(1) copy.
fn open_text(editor: &Editor, place: &Path) -> Option<oso_core::rope::Rope> {
    let buffer = editor.buffer(open_buffer_of(editor, place)?)?;
    Some(buffer.text().clone())
}

/// The places of the file at `place` and of the one it came from, whose old side may show.
fn guards_of(reading: &Reading, root: &Path, place: &Path) -> Vec<PathBuf> {
    let before = (reading.entry.old_path.as_deref()).and_then(|old| place_of(root, old));
    before.into_iter().chain([place.to_path_buf()]).collect()
}

/// Whether an open buffer of a file at one of `places` may hold secrets, so that nothing of it
/// shows.
fn secret_buffer(editor: &Editor, places: &[PathBuf]) -> bool {
    (places.iter())
        .filter_map(|place| open_buffer_of(editor, place))
        .filter_map(|buffer| editor.buffer(buffer))
        .any(oso_core::buffer::Buffer::is_secret)
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

/// Why a file of `status`, whose old side is `old`, has no side `want`, if it has none.
fn no_side(status: Status, want: Side, old: &[u8]) -> Option<&'static str> {
    match status {
        Status::Deleted if want == Side::New => {
            Some("Deleted in this review: C-u RET shows the old side")
        }
        Status::Added | Status::Copied if want == Side::Old && old.is_empty() => {
            Some("Added in this review: it has no old side")
        }
        _ => None,
    }
}

/// Where a jump to a file the review shows no text of goes: the checkout's file, as "too large
/// to show: `RET` opens it" says, once the review's thread looked at it as a carry reads one;
/// nowhere for a side the review did not read. `said` is what a review of a commit says of it.
fn not_text(
    reading: &Reading,
    place: PathBuf,
    side_wanted: bool,
    said: Option<String>,
) -> Result<(Ask, To), String> {
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
            let to = To::File {
                place,
                line: 1,
                commit: said.map(|said| FromCommit {
                    said,
                    or_side: None,
                }),
            };
            Ok((Ask::Look { path }, to))
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
