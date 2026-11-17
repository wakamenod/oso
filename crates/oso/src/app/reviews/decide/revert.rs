//! Reject in the work tree (ADR 0122, step 5, Q13): a hunk's old lines put back in the file
//! through its buffer, as one undo step, and the buffer saved safely; and undo of it.
//!
//! Must not depend on: key presses or how a frame is drawn; the loader opens the file and the
//! saver writes it.
//!
//! - The file's open buffer is used, or one opened unshown, which stays open (Q-D). It must be
//!   the very file the review read: its real place, links followed, is the project's root joined
//!   with the path, so no link, of the file or a directory on the way, leads elsewhere; and it
//!   may not hold secrets.
//! - Each hunk's lines, and up to [`CONTEXT`] lines around it, must be where the reading put
//!   them, the lines around by the reading's new side; else nothing changes: "that hunk
//!   changed; look again".
//! - The edit waits while the buffer is saving, and a file changed on disk since it was read is
//!   not saved over. One revert of a file at a time.
//! - The save starts at once, as the person's.
//! - The save cleans up as any save does: a missing final newline goes in, and trailing white
//!   space on the lines put back goes when the cleanup is on.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use oso_core::buffer::{BufferId, Replacement};
use oso_core::editor::ReviewId;
use oso_core::review::Hunk;
use oso_core::rope::Rope;

use super::{Done, Marking, Picked, hunks_said, named, new_side};
use crate::app::App;
use crate::commands::review::{open_buffer_of, place_of};
use crate::review::Reading;
use crate::review::stage::CHANGED;

/// How many lines around a hunk are checked.
const CONTEXT: usize = 3;

/// A revert of hunks in a file's buffer, waiting for the buffer.
pub(super) struct Reverting {
    review: ReviewId,
    picked: Picked,
    reading: Arc<Reading>,
    buffer: BufferId,
    /// Where the buffer must be: the project's root joined with the path.
    place: PathBuf,
    /// Whether it puts the new side back, as undo does.
    undo: bool,
    phase: Phase,
    /// Whether it said it waits for the file to load.
    said_loading: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Waiting for the buffer to load, or for its save under way to end.
    Waiting,
    /// Edited, waiting for its save to end.
    Saving,
}

/// The line `line` of `lines`, empty past them.
fn line_of<'a>(lines: &[&'a [u8]], line: usize) -> &'a [u8] {
    lines.get(line).copied().unwrap_or_default()
}

/// The text of `text`'s line `line`, its line break too, or `None` past the last.
fn text_line(text: &Rope, line: usize, lines: usize) -> Option<String> {
    if line >= lines {
        return None;
    }
    let start = text.line_start(line);
    let end = if line + 1 < lines {
        text.line_start(line + 1)
    } else {
        text.len()
    };
    Some(text.chunks_in_range(start..end).collect())
}

/// Checks the lines `range` of the new side `new` are in `text`, each `shift` lines on, beside
/// the hunk at new line `hunk`; says in the log which is not, with no text.
fn around(
    text: &Rope,
    new: &[&[u8]],
    range: Range<usize>,
    shift: isize,
    hunk: u32,
) -> Result<(), &'static str> {
    let lines = text.max_point().row + 1;
    for line in range {
        let there = text_line(text, line.checked_add_signed(shift).ok_or(CHANGED)?, lines);
        if there.as_deref().map(str::as_bytes) != Some(line_of(new, line)) {
            log::debug!(
                "a revert refused: new line {} beside the hunk at new line {hunk} differs \
                 ({} new-side lines, {lines} in the buffer)",
                line + 1,
                new.len()
            );
            return Err(CHANGED);
        }
    }
    Ok(())
}

/// The edits that put each of `hunks` back to its old lines in `text`, or to its new lines when
/// `undone`, as undo after a revert. Each hunk's lines, and the lines around it up to the next,
/// must be where `new_side`, the reading's new side, puts them, moved by what the hunks before
/// it changed when `undone`. Descending, as edits apply.
///
/// # Errors
///
/// [`CHANGED`] when anything checked is not there.
pub(in crate::app::reviews) fn revert_edits(
    text: &Rope,
    new_side: &[u8],
    hunks: &[&Hunk],
    undone: bool,
) -> Result<Vec<(Range<usize>, String)>, &'static str> {
    let mut hunks = hunks.to_vec();
    hunks.sort_by_key(|hunk| hunk.new_start);
    let new: Vec<&[u8]> = new_side.split_inclusive(|byte| *byte == b'\n').collect();
    let lines = text.max_point().row + 1;
    let joined = |lines: &[Box<[u8]>]| String::from_utf8(lines.concat()).map_err(|_| CHANGED);
    // How far a line of the buffer is from the same line of the new side: what the hunks
    // before it put back changed, when undone.
    let mut shift: isize = 0;
    let mut edits = Vec::with_capacity(hunks.len());
    for (at, hunk) in hunks.iter().enumerate() {
        let (want, put) = if undone {
            (&hunk.removed, &hunk.added)
        } else {
            (&hunk.added, &hunk.removed)
        };
        let first = (hunk.new_start as usize).saturating_sub(1);
        let start_line = first.checked_add_signed(shift).ok_or(CHANGED)?;
        if start_line > lines {
            log::debug!(
                "a revert refused: hunk at new line {} past the {lines} lines",
                hunk.new_start
            );
            return Err(CHANGED);
        }
        // The lines before it, back to the last hunk's, as the new side has them.
        let before = (at > 0).then(|| hunks[at - 1]);
        let floor = before.map_or(0, |before| {
            (before.new_start as usize).saturating_sub(1) + before.added.len()
        });
        let range = first.saturating_sub(CONTEXT).max(floor)..first;
        around(text, &new, range, shift, hunk.new_start)?;
        let start = if start_line < lines {
            text.line_start(start_line)
        } else {
            text.len()
        };
        let want = joined(want)?;
        let end = start + want.len();
        let there: String = if end <= text.len() {
            text.chunks_in_range(start..end).collect()
        } else {
            String::new()
        };
        if there != want {
            log::debug!(
                "a revert refused: the hunk at new line {} is not there ({} bytes from {start}, text of {})",
                hunk.new_start,
                want.len(),
                text.len()
            );
            return Err(CHANGED);
        }
        let after_shift = if undone {
            shift + hunk.removed.len() as isize - hunk.added.len() as isize
        } else {
            0
        };
        // The lines after it, up to the next hunk's, as the new side has them.
        let next = hunks
            .get(at + 1)
            .map(|next| (next.new_start as usize).saturating_sub(1));
        let from = first + hunk.added.len();
        let until = (from + CONTEXT)
            .min(next.unwrap_or(usize::MAX))
            .min(new.len());
        around(text, &new, from..until, after_shift, hunk.new_start)?;
        shift = after_shift;
        edits.push((start..end, joined(put)?));
    }
    edits.reverse();
    Ok(edits)
}

impl App {
    /// Reverts `picked` through its file's buffer, opened unshown if it is not open.
    pub(super) fn revert_picked(&mut self, index: usize, picked: Picked) {
        let readings = &self.reviews.open[index].decisions.readings;
        let reading = readings.get(&picked.path).filter(|reading| {
            let shown = match &reading.content {
                crate::review::Content::Text { shape, .. } => Some(shape),
                _ => None,
            };
            shown.is_some_and(|shape| Arc::ptr_eq(shape, &picked.shape))
        });
        let reading = reading.cloned();
        let Some(reading) = reading else {
            return self
                .editor
                .message("This file is being read again: try in a moment");
        };
        self.start_revert(index, picked, reading, false);
    }

    /// Puts back what the revert `picked` took out, keeping the decision if that fails.
    pub(super) fn put_back(&mut self, index: usize, picked: Picked, reading: Arc<Reading>) {
        self.start_revert(index, picked, reading, true);
    }

    fn start_revert(&mut self, index: usize, picked: Picked, reading: Arc<Reading>, undo: bool) {
        let running = &self.reviews.open[index];
        let review = running.id;
        let Some(place) = place_of(running.root.path(), &picked.path) else {
            return self.revert_failed(
                index,
                picked,
                reading,
                undo,
                "git named a path oso does not follow",
            );
        };
        let buffer = match open_buffer_of(&self.editor, &place) {
            Some(buffer) => buffer,
            None => match self.visit(place.clone(), oso_core::load::UNASKED) {
                Some(buffer) => buffer,
                None => {
                    return self.revert_failed(
                        index,
                        picked,
                        reading,
                        undo,
                        "The file could not be opened",
                    );
                }
            },
        };
        self.reviews.open[index]
            .decisions
            .reverting
            .push(Reverting {
                review,
                picked,
                reading,
                buffer,
                place,
                undo,
                phase: Phase::Waiting,
                said_loading: false,
            });
        self.go_on_reverting();
    }

    /// Says why a revert or its undo did not start, keeping an undo's decision.
    fn revert_failed(
        &mut self,
        index: usize,
        picked: Picked,
        reading: Arc<Reading>,
        undo: bool,
        why: &str,
    ) {
        if undo {
            self.keep_undone(index, Done::Reverted { picked, reading });
        }
        self.editor.message(why);
    }

    /// Makes the edit of each revert whose buffer has loaded, and starts its save at once.
    pub(in crate::app::reviews) fn go_on_reverting(&mut self) {
        let mut saving = false;
        for index in 0..self.reviews.open.len() {
            let mut waiting = std::mem::take(&mut self.reviews.open[index].decisions.reverting);
            let mut kept = Vec::with_capacity(waiting.len());
            for mut reverting in waiting.drain(..) {
                let before = reverting.phase;
                let now = self.revert_now(&mut reverting);
                saving |= before == Phase::Waiting && reverting.phase == Phase::Saving;
                match now {
                    Ok(true) => kept.push(reverting),
                    Ok(false) => {}
                    Err(why) => {
                        let Reverting {
                            picked,
                            reading,
                            undo,
                            ..
                        } = reverting;
                        self.revert_failed(index, picked, reading, undo, why);
                    }
                }
            }
            self.reviews.open[index].decisions.reverting.extend(kept);
        }
        // A person's save queued here starts now, not with whoever's command comes next.
        if saving {
            let _ = self.spawn_saves(false);
            self.start_trims();
        }
    }

    /// Makes `reverting`'s edit once its buffer has loaded and no save of it runs, and queues
    /// its save. Returns whether it still waits.
    fn revert_now(&mut self, reverting: &mut Reverting) -> Result<bool, &'static str> {
        if reverting.phase == Phase::Saving {
            return Ok(true);
        }
        let Some(buffer) = self.editor.buffer(reverting.buffer) else {
            return Err("The file's buffer has gone: nothing reverted");
        };
        if buffer.loading().is_some() || buffer.is_saving() {
            if !reverting.said_loading {
                reverting.said_loading = true;
                let name = named(&reverting.picked.path);
                self.editor
                    .message(format!("Reverting once {name} has loaded\u{2026}"));
            }
            return Ok(true);
        }
        // The very file the review read, and none that may hold secrets.
        let same = buffer
            .baseline()
            .is_some_and(|baseline| baseline.location == reverting.place);
        if !same {
            return Err("The file is not where the review read it, or is a link: nothing reverted");
        }
        if buffer.is_secret() {
            return Err("The file may hold secrets: nothing reverted");
        }
        if buffer.changed_on_disk() {
            return Err("The file changed on disk since it was read: revert the buffer first");
        }
        let new = new_side(&reverting.reading).ok_or(CHANGED)?;
        let hunks: Vec<&Hunk> = reverting.picked.hunks().collect();
        let edits = revert_edits(buffer.text(), new, &hunks, reverting.undo)?;
        let replacements: Vec<Replacement<'_>> = (edits.iter())
            .map(|(range, text)| Replacement {
                range: range.clone(),
                text,
            })
            .collect();
        self.editor
            .apply_edits(reverting.buffer, &replacements)
            .map_err(|_| "The file's buffer cannot be edited: nothing reverted")?;
        self.editor
            .start_save_of(reverting.buffer)
            .map_err(|_| "Reverted in the buffer, not saved: save it to finish")?;
        reverting.phase = Phase::Saving;
        Ok(true)
    }

    /// The save of `buffer` ended, `written` or not: a revert waiting for it is done.
    pub(in crate::app) fn review_save_ended(&mut self, buffer: BufferId, written: bool) {
        for index in 0..self.reviews.open.len() {
            let decisions = &mut self.reviews.open[index].decisions;
            let Some(at) = (decisions.reverting.iter())
                .position(|it| it.phase == Phase::Saving && it.buffer == buffer)
            else {
                continue;
            };
            let Reverting {
                review,
                picked,
                reading,
                undo,
                ..
            } = decisions.reverting.remove(at);
            if !written {
                if undo {
                    self.keep_undone(index, Done::Reverted { picked, reading });
                }
                self.editor
                    .message("Reverted in the buffer, not saved: save it to finish");
                continue;
            }
            let verb = if undo { "Put back" } else { "Reverted" };
            let said = format!(
                "{verb} {} of {}",
                hunks_said(picked.hunks.len()),
                named(&picked.path)
            );
            // A reverted hunk leaves the review: its whole mark goes (Q-G).
            if !undo {
                self.mark_picked(index, &picked, Marking::Leaves);
                self.keep_done(index, Done::Reverted { picked, reading });
            }
            self.editor.message(said);
            self.refresh_review(review);
        }
    }
}
