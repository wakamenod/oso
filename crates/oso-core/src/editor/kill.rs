//! Kills and yanks of the selected window, kills in a buffer a jump's action names
//! (ADR 0116), and what goes to the OS clipboard (ADR 0074).
//!
//! Must not depend on: `command`, the clipboard itself or the host's threads; the host
//! takes what to write and brings what it read.
//!
//! A kill takes one piece per cursor, as [`kill::kill_ranges`] merges them, and puts
//! them in the ring as one entry, or appends them to the newest one when the command
//! before also killed. The dispatcher says when a command ends, so that a kill knows
//! whether it follows one. A yank puts back one piece per cursor if their numbers
//! match, and the pieces joined at every cursor otherwise (ADR 0007).

use std::fmt;
use std::ops::Range;

use super::Editor;
use crate::buffer::{BufferId, EditError, Replacement};
use crate::kill::{self, Direction, Fingerprint, KillEntry, Piece};
use crate::rope::Rope;
use crate::selection::{MarkActive, Selection};

/// The newest entry of the ring, for the host to put on the OS clipboard.
#[derive(Debug)]
pub struct ClipboardWrite {
    /// The entry it is, for [`Editor::note_clipboard_written`].
    pub id: u64,
    /// The text, joined only by [`ClipboardWrite::text`], off the UI thread.
    pub pieces: Vec<Piece>,
    /// Whether the clipboard should mark it concealed, for clipboard managers.
    pub secret: bool,
}

impl ClipboardWrite {
    /// The text the clipboard gets: the pieces joined with line breaks. O(its length),
    /// so the host calls it on the clipboard's thread.
    pub fn text(&self) -> String {
        kill::join(&self.pieces)
    }
}

/// Why a yank did nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YankError {
    /// There is no such entry.
    Empty,
    /// The text, times the cursors it goes to, is over [`kill::MAX_YANK`].
    TooMuch,
    /// The buffer refused the edit.
    Edit(EditError),
}

impl fmt::Display for YankError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("Kill ring is empty"),
            Self::TooMuch => f.write_str("Too much text to yank"),
            Self::Edit(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for YankError {}

impl From<EditError> for YankError {
    fn from(error: EditError) -> Self {
        Self::Edit(error)
    }
}

/// What the editor knows of kills between commands.
#[derive(Debug, Default)]
pub(super) struct Kills {
    pub(super) ring: kill::KillRing,
    /// The command before the running one killed, so a kill now appends to it.
    follows_kill: bool,
    /// The running command killed.
    killed: bool,
    /// The newest entry, to go to the clipboard once the command ends.
    to_write: Option<u64>,
    /// What the last yank inserted, for `M-y` right after it (ADR 0091).
    pub(super) last_yank: Option<super::yank_pop::LastYank>,
}

impl Editor {
    /// The kill ring.
    pub fn kill_ring(&self) -> &kill::KillRing {
        &self.kills.ring
    }

    /// Puts entries another instance's ring held in front of what this one holds, given the
    /// newest first, as `oso dev` carries them (ADR 0090). They hold no secret and nothing
    /// from the clipboard, as those are never carried. The bounds of the ring apply.
    pub fn carry_kills(&mut self, newest_first: Vec<Vec<Piece>>) {
        for pieces in newest_first.into_iter().rev() {
            self.kills.ring.push(pieces, false);
        }
    }

    /// Kills the text in `ranges` of the selected window's buffer, one range for each
    /// cursor, gone `direction` from it: puts it in the ring, and deletes it unless
    /// `copy_only` is set, as `M-w` does. The text is taken as rope slices, so this costs
    /// O(log n) per range whatever their size.
    ///
    /// It appends to the newest entry if the command before also killed and the pieces
    /// match in number. Nothing is taken if every range is empty. Text from a buffer that
    /// may hold secrets makes a secret entry, as does the agent channel's from a buffer
    /// withheld from it, and text whose verdict may not be killed, a
    /// passphrase's, is deleted and goes nowhere (ADR 0072).
    ///
    /// # Errors
    ///
    /// [`EditError::ReadOnly`] for a read-only buffer, after the text has gone to the
    /// ring, as Emacs's `kill-region` copies what it cannot delete.
    pub fn kill(
        &mut self,
        ranges: impl IntoIterator<Item = Range<usize>>,
        direction: Direction,
        copy_only: bool,
    ) -> Result<(), EditError> {
        let id = self.current().0.buffer();
        self.kill_in(id, ranges, direction, copy_only)
    }

    /// [`Editor::kill`] in the buffer `id`, which another window than the selected one may
    /// show, as a jump's actions kill there (ADR 0116).
    pub(super) fn kill_in(
        &mut self,
        id: BufferId,
        ranges: impl IntoIterator<Item = Range<usize>>,
        direction: Direction,
        copy_only: bool,
    ) -> Result<(), EditError> {
        let buffer = self.buffer(id).ok_or(EditError::NoSuchBuffer)?;
        let merged = kill::kill_ranges(ranges, !copy_only);
        let text = buffer.text();
        let pieces: Vec<Piece> = merged
            .iter()
            .map(|range| Piece::from(text.slice(range.clone())))
            .collect();
        let kept = buffer.verdict().may_be_killed();
        if pieces.iter().all(Piece::is_empty) {
            return if copy_only {
                Ok(())
            } else {
                buffer
                    .read_only()
                    .map_or(Ok(()), |reason| Err(EditError::ReadOnly(reason)))
            };
        }
        // What the agent channel kills from a buffer it may not read is secret, so that it
        // cannot yank the text where it may read it (ADR 0121). The person's kills are theirs.
        let secret = buffer.is_secret() || (buffer.withheld_from_agents() && self.runs_for_agent());
        let read_only = buffer.read_only();
        if kept {
            self.add_kill(pieces, direction, secret);
        }
        if copy_only {
            return Ok(());
        }
        if let Some(reason) = read_only {
            return Err(EditError::ReadOnly(reason));
        }
        let edits: Vec<Replacement<'_>> = merged
            .into_iter()
            .filter(|range| !range.is_empty())
            .map(|range| Replacement { range, text: "" })
            .collect();
        self.apply_edits(id, &edits)
    }

    /// Puts `pieces` in the ring as an entry of its own, which goes to the clipboard as a
    /// kill's does, as a removed review comment does (ADR 0122).
    pub(super) fn push_kill(&mut self, pieces: Vec<Piece>) {
        let entry = self.kills.ring.push(pieces, false);
        self.kills.killed = true;
        self.kills.to_write = Some(entry);
    }

    /// Puts `pieces` in the ring, appended to the newest entry if this follows a kill.
    fn add_kill(&mut self, pieces: Vec<Piece>, direction: Direction, secret: bool) {
        let kills = &mut self.kills;
        let appended = if kills.follows_kill {
            kills.ring.append(pieces, direction, secret)
        } else {
            Err(pieces)
        };
        let entry = appended.unwrap_or_else(|pieces| kills.ring.push(pieces, secret));
        kills.killed = true;
        kills.to_write = Some(entry);
    }

    /// Puts text another app put on the clipboard in the ring as an entry of one piece, as
    /// Emacs's `interprogram-paste-function` does, for a yank to insert, unless the clipboard
    /// holds what it was last seen to, wherever `M-y` has moved that entry. It counts as secret if the clipboard marks it concealed,
    /// as a password manager does, or if a secret entry went to the clipboard as the same
    /// text, so a round trip through another app does not wash it (ADR 0074).
    pub fn take_from_clipboard(&mut self, text: Rope, concealed: bool, fingerprint: Fingerprint) {
        let ring = &mut self.kills.ring;
        if ring.clipboard_seen(fingerprint) {
            if concealed {
                ring.make_written_secret(fingerprint);
            }
            return;
        }
        let secret = concealed || ring.holds_secret(fingerprint);
        let id = ring.push_from_clipboard(text, secret);
        ring.note_written(id, fingerprint);
    }

    /// The newest entry, if a command has killed since the host last asked and it is
    /// small enough for the clipboard. A larger one stays in the ring, and the echo area
    /// says so. Kills from a buffer that may hold secrets come with `secret` set.
    pub fn take_clipboard_write(&mut self) -> Option<ClipboardWrite> {
        let id = self.kills.to_write.take()?;
        let entry = self.kills.ring.get(0).filter(|entry| entry.id() == id)?;
        if entry.len() > kill::MAX_CLIPBOARD {
            // A secret entry's size stays out of the log and `*Messages*`.
            let said = "Not copied to the clipboard: too large";
            if entry.is_secret() {
                self.echo(said);
            } else {
                self.message(said);
            }
            return None;
        }
        Some(ClipboardWrite {
            id,
            pieces: entry.pieces().to_vec(),
            secret: entry.is_secret(),
        })
    }

    /// Records what the clipboard got for the entry `id`.
    pub fn note_clipboard_written(&mut self, id: u64, fingerprint: Fingerprint) {
        self.kills.ring.note_written(id, fingerprint);
    }

    /// Tells the kill ring that a command, any command, has ended. A kill after it
    /// appends to its entry only if it killed. One that left a prefix argument (`C-u`,
    /// digits) changes nothing, as it is part of the next command.
    pub fn note_command_ended(&mut self, left_prefix: bool) {
        let kills = &mut self.kills;
        if !left_prefix {
            kills.follows_kill = kills.killed;
        }
        kills.killed = false;
    }

    /// Ends a run of kills, for a key sequence that was undefined or cancelled.
    pub fn forget_kills(&mut self) {
        self.kills.follows_kill = false;
    }

    /// Yanks the `n`th newest entry at every cursor of the selected window, replacing the
    /// region if it shows (`delete-selection-mode`). Each cursor ends after what it
    /// inserted, with its mark at the start, and the region hidden, as Emacs's `yank`
    /// does, or the other way round if `cursor_at_start` (`C-u C-y`). Yanking a secret
    /// entry makes the buffer secret (ADR 0051).
    ///
    /// # Errors
    ///
    /// [`YankError::Empty`] with no such entry, [`YankError::TooMuch`] past
    /// [`kill::MAX_YANK`], and the buffer's refusal.
    pub fn yank(&mut self, n: usize, cursor_at_start: bool) -> Result<(), YankError> {
        let entry = self.kills.ring.get(n).ok_or(YankError::Empty)?;
        let entry_id = entry.id();
        let (window, buffer) = self.current();
        if let Some(reason) = buffer.read_only() {
            return Err(EditError::ReadOnly(reason).into());
        }
        let id = window.buffer();
        let set = &window.selections;
        let texts = yank_texts(entry, set.len())?;
        let secret = entry.is_secret();
        let replaces = set.mark_active().is_active();
        let ranges: Vec<Range<usize>> = set
            .iter()
            .map(|selection| yank_range(selection, replaces))
            .collect();
        // One text for every cursor repeats, as `cycle` does for a list of one.
        let edits: Vec<Replacement<'_>> = ranges
            .iter()
            .zip(texts.iter().cycle())
            .map(|(range, text)| Replacement {
                range: range.clone(),
                text,
            })
            .collect();
        // Marks follow edits as markers do: one at the start of a replaced range stays
        // before the new text, where Emacs's `yank` sets it.
        let window = &mut self.windows[self.selected];
        window.selections.set_mark_active(MarkActive::Inactive);
        window.selections.push_marks();
        let mut starts = ranges.iter().map(|range| range.start);
        window.selections.update(|selection| {
            selection.mark = starts.next();
            Ok::<(), ()>(())
        });
        self.apply_edits(id, &edits)?;
        if cursor_at_start {
            self.windows[self.selected].selections.update(|selection| {
                if let Some(mark) = selection.mark {
                    selection.mark = Some(selection.cursor);
                    selection.cursor = mark;
                }
                Ok::<(), ()>(())
            });
        }
        self.note_yank(entry_id);
        if secret
            && self
                .buffer_mut(id)
                .is_some_and(|buffer| buffer.treat_as_secret())
        {
            // The verdict of a buffer on the screen changed, as a window's buffer can.
            self.note_secret_shown();
            self.message("Buffer now treated as secret");
        }
        Ok(())
    }
}

/// What a selection's yank replaces: its region if regions show, or nothing at its
/// cursor.
fn yank_range(selection: &Selection, replaces: bool) -> Range<usize> {
    if replaces {
        selection.range()
    } else {
        selection.cursor..selection.cursor
    }
}

/// Whether a yank of `pieces` at `cursors` cursors is within [`kill::MAX_YANK`]: one piece
/// each if their numbers match, or else the pieces joined at every one. A jump's action asks
/// before it kills what it yanks (ADR 0116).
pub(super) fn yank_fits(pieces: &[Piece], cursors: usize) -> bool {
    let text: usize = pieces.iter().map(Piece::len).sum();
    let total = if pieces.len() == cursors {
        Some(text)
    } else {
        let joined = text + kill::PIECE_SEPARATOR.len() * pieces.len().saturating_sub(1);
        joined.checked_mul(cursors)
    };
    total.is_some_and(|total| total <= kill::MAX_YANK)
}

/// The text a yank of `entry` inserts at each of `cursors` cursors: one piece each if
/// their numbers match, or else one text, the pieces joined, for all of them.
fn yank_texts(entry: &KillEntry, cursors: usize) -> Result<Vec<String>, YankError> {
    let pieces = entry.pieces();
    let per_cursor = pieces.len() == cursors;
    if !yank_fits(pieces, cursors) {
        return Err(YankError::TooMuch);
    }
    Ok(if per_cursor {
        pieces.iter().map(Piece::text).collect()
    } else {
        vec![entry.joined()]
    })
}

#[cfg(test)]
mod tests {
    use super::Editor;
    use crate::kill::Piece;
    use crate::rope::Rope;

    #[test]
    fn carried_kills_go_in_front_in_their_order_and_count_as_no_secret() {
        let mut editor = Editor::with_buffer(crate::buffer::Buffer::new("*scratch*"));
        editor
            .kills
            .ring
            .push(vec![Piece::from(Rope::from("mine"))], true);
        let piece = |text: &str| Piece::from(Rope::from(text));
        editor.carry_kills(vec![vec![piece("newest")], vec![piece("a"), piece("b")]]);
        let ring = editor.kill_ring();
        let joined: Vec<String> = ring.iter().map(|entry| entry.joined()).collect();
        assert_eq!(joined, ["newest", "a\nb", "mine"]);
        assert!(ring.iter().take(2).all(|entry| !entry.is_secret()));
        assert!(ring.iter().all(|entry| !entry.came_from_clipboard()));
    }

    #[test]
    fn the_agents_kill_from_a_buffer_withheld_from_it_is_secret_and_the_persons_is_not() {
        use crate::buffer::Mode;
        use crate::kill::Direction;
        let mut editor = Editor::with_buffer(crate::buffer::Buffer::new("*scratch*"));
        let id = editor.add_internal_buffer("*Report*", Mode::Fundamental);
        editor.set_internal_text(id, "screen text");
        editor.buffer_mut(id).unwrap().withhold_from_agents();
        editor.show(id);
        editor
            .kill(std::iter::once(0..6), Direction::Forward, true)
            .unwrap();
        assert!(
            !editor.kill_ring().get(0).unwrap().is_secret(),
            "the person's"
        );
        editor.forget_kills();
        editor
            .for_agent(|editor| editor.kill(std::iter::once(7..11), Direction::Forward, true))
            .unwrap();
        let newest = editor.kill_ring().get(0).unwrap();
        assert_eq!(newest.joined(), "text");
        assert!(newest.is_secret(), "the agent's");
        assert!(!editor.current().1.is_secret(), "the buffer is no secret");
    }
}
