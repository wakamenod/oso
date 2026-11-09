//! `M-y` (ADR 0091): the kill ring as a list that completes, the newest first, with the
//! selected entry's text in the preview, and yanking the one picked; and `M-y` in a prompt
//! right after a yank, which goes to the next older entry, as Emacs's `yank-pop` always did.
//!
//! Must not depend on: `command`, `keymap`, the clipboard or the host's window. The
//! dispatcher reads the clipboard before it opens the list.
//!
//! - A row is the first [`ROW_BYTES`] of an entry, its pieces joined by line breaks, each
//!   drawn as `⤶`, and the rest escaped as `*Messages*` escapes a row. Its annotation says
//!   how many lines and pieces it has, when more than one.
//! - A secret entry is listed with no text, and hidden as soon as anything is typed; its
//!   preview is withheld. An entry equal to a newer one is listed once.
//! - What a yank inserted is remembered with the revision it left, so that a yank picked
//!   right after it, with the text and cursors as it left them, takes its place.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use super::Editor;
use super::completion::CompletionStyle;
use super::kill::YankError;
use super::messages::row_text;
use crate::buffer::BufferId;
use crate::complete::Candidate;
use crate::kill::{KillEntry, PIECE_SEPARATOR};
use crate::minibuffer::{Prompt, PromptError, PromptId};
use crate::selection::{MarkActive, Selection, SelectionSet};

/// The most bytes of an entry a row shows, and what the input matches against.
pub(crate) const ROW_BYTES: usize = 1024;

/// What a line break reads as in a row.
const LINE_BREAK: char = '⤶';

/// The ring's entries a `M-y` list shows, by their ids in the order of its rows, and the one
/// picked when it closed.
#[derive(Debug)]
pub(super) struct KillList {
    prompt: PromptId,
    entries: Vec<u64>,
    picked: Option<u64>,
}

/// What the last yank inserted: in which window and buffer, the revision it left, and each
/// cursor's range of new text, so that `M-y` right after it can take its place.
#[derive(Debug)]
pub(super) struct LastYank {
    window: usize,
    buffer: BufferId,
    revision: u64,
    ranges: Vec<Range<usize>>,
    entry: u64,
}

impl Editor {
    /// Opens a prompt that says `label` and completes over the kill ring, as the module says.
    /// It keeps no history.
    ///
    /// # Errors
    ///
    /// [`PromptError::Invalid`] with an empty ring, and [`PromptError::TooDeep`] with as many
    /// prompts open as may be. Nothing changes.
    pub fn open_kill_list(&mut self, label: &str) -> Result<PromptId, PromptError> {
        let mut seen_text = HashSet::new();
        let mut seen_print = HashSet::new();
        let mut entries = Vec::new();
        let mut items = Vec::new();
        for entry in self.kills.ring.iter() {
            // The same text again is listed once, at its newest; only small texts are
            // compared whole, and large ones by what the clipboard got of them.
            let again = if entry.len() <= ROW_BYTES {
                !seen_text.insert(entry.joined())
            } else {
                entry
                    .fingerprint()
                    .is_some_and(|print| !seen_print.insert(print))
            };
            if again {
                continue;
            }
            entries.push(entry.id());
            items.push(candidate(entry));
        }
        if entries.is_empty() {
            return Err(PromptError::Invalid);
        }
        let style = CompletionStyle {
            given_order: true,
            takes_input: false,
            preview: true,
        };
        let items: Arc<[Candidate]> = items.into();
        // No history: the rows are the kills' text.
        let prompt = self.open_completion_as(label, "", items, style)?;
        self.kill_list = Some(KillList {
            prompt,
            entries,
            picked: None,
        });
        Ok(prompt)
    }

    /// Whether a `M-y` list is the innermost prompt, whose rows the control socket leaves
    /// out, and of which no screenshot is taken (ADR 0091).
    pub fn kill_list_open(&self) -> bool {
        let list = self.kill_list.as_ref();
        self.prompt()
            .is_some_and(|prompt| list.is_some_and(|list| list.prompt == prompt.id()))
    }

    /// The entry the innermost prompt's selected row names, while it is a `M-y` list.
    pub(super) fn selected_kill(&self) -> Option<&KillEntry> {
        let list = self.kill_list.as_ref()?;
        let prompt = self.prompt().filter(|prompt| prompt.id() == list.prompt)?;
        let index = prompt.candidates()?.selected_item()?;
        let id = *list.entries.get(index)?;
        self.kills.ring.iter().find(|entry| entry.id() == id)
    }

    /// The first `rows` lines of the entry `id`, each cut to `columns` characters and
    /// escaped, its pieces joined by line breaks, for the preview. It reads at most a few
    /// bytes per cell it fills, so a large entry costs no more than a small one.
    pub fn kill_preview(&self, id: u64, rows: usize, columns: usize) -> Vec<String> {
        let Some(entry) = self.kills.ring.iter().find(|entry| entry.id() == id) else {
            return Vec::new();
        };
        let mut budget = rows
            .saturating_mul(columns.saturating_add(1))
            .saturating_mul(4);
        let mut lines = vec![String::new()];
        let mut cells = 0;
        let pieces = entry.pieces();
        let texts = pieces.iter().enumerate().flat_map(|(index, piece)| {
            let between = (index > 0).then_some(PIECE_SEPARATOR);
            between.into_iter().chain(piece.chunks())
        });
        'read: for text in texts {
            for c in text.chars() {
                budget = budget.saturating_sub(c.len_utf8());
                if c == '\n' {
                    if lines.len() == rows {
                        break 'read;
                    }
                    lines.push(String::new());
                    cells = 0;
                } else if cells < columns {
                    if let Some(line) = lines.last_mut() {
                        line.push(c);
                    }
                    cells += 1;
                }
                if budget == 0 {
                    break 'read;
                }
            }
        }
        lines.truncate(rows);
        lines.iter().map(|line| row_text(line)).collect()
    }

    /// Notes which entry a `M-y` list's selected row named as `closed` closes, and forgets
    /// the list.
    pub(super) fn forget_kill_list(&mut self, closed: &Prompt) {
        let Some(list) = self
            .kill_list
            .as_mut()
            .filter(|list| list.prompt == closed.id())
        else {
            return;
        };
        let index = closed.candidates().and_then(|list| list.selected_item());
        list.picked = index.and_then(|index| list.entries.get(index).copied());
        list.entries.clear();
    }

    /// Yanks the entry a `M-y` list picked, as `C-y` yanks, in place of what the last yank
    /// inserted if the text and the cursors are as it left them, as consult-yank-pop does.
    /// The entry moves to the front of the ring, and the clipboard is not written.
    ///
    /// # Errors
    ///
    /// [`YankError::Empty`] if nothing was picked or the ring no longer holds it, and what
    /// [`Editor::yank`] says.
    pub fn yank_picked(&mut self) -> Result<(), YankError> {
        let picked = self.kill_list.take().and_then(|list| list.picked);
        let id = picked.ok_or(YankError::Empty)?;
        self.kills.ring.position(id).ok_or(YankError::Empty)?;
        self.select_last_yank();
        self.kills.ring.bring_to_front(id);
        self.yank(0, false)
    }

    /// `M-y` in a prompt: right after a yank, puts the next older entry in place of what it
    /// inserted, round to the newest past the oldest, passing secret ones.
    ///
    /// # Errors
    ///
    /// [`YankPopError::NotAfterYank`], [`YankPopError::NoOther`], or what the yank says.
    pub fn yank_pop(&mut self) -> Result<(), YankPopError> {
        let entry = self.last_yank().ok_or(YankPopError::NotAfterYank)?.entry;
        let n = self
            .next_older(entry, |entry| !entry.is_secret())
            .ok_or(YankPopError::NoOther)?;
        self.select_last_yank();
        self.yank(n, false).map_err(YankPopError::Yank)
    }

    /// Where the next entry older than `id` is that `takes` takes, going round the ring, and
    /// not `id` itself.
    pub(super) fn next_older(&self, id: u64, takes: impl Fn(&KillEntry) -> bool) -> Option<usize> {
        let ring = &self.kills.ring;
        let at = ring.position(id)?;
        (1..ring.len())
            .map(|step| (at + step) % ring.len())
            .find(|&n| ring.get(n).is_some_and(&takes))
    }

    /// Remembers what the yank of the entry `entry` just inserted in the selected window.
    pub(super) fn note_yank(&mut self, entry: u64) {
        let (window, buffer) = self.current();
        let ranges = window.selections.iter().map(Selection::range).collect();
        self.kills.last_yank = Some(LastYank {
            window: self.selected,
            buffer: window.buffer(),
            revision: buffer.revision(),
            ranges,
            entry,
        });
    }

    /// What the last yank inserted, if the selected window's text and cursors are as it left
    /// them.
    fn last_yank(&self) -> Option<&LastYank> {
        let last = self.kills.last_yank.as_ref()?;
        let (window, buffer) = self.current();
        let same = last.window == self.selected
            && last.buffer == window.buffer()
            && last.revision == buffer.revision()
            && window
                .selections
                .iter()
                .map(Selection::range)
                .eq(last.ranges.iter().cloned());
        same.then_some(last)
    }

    /// Selects what the last yank inserted, as regions that the next yank replaces, if it is
    /// still as it left it.
    fn select_last_yank(&mut self) {
        let Some(ranges) = self.last_yank().map(|last| last.ranges.clone()) else {
            return;
        };
        let selections = &mut self.windows[self.selected].selections;
        let parts = ranges
            .into_iter()
            .map(|range| Selection {
                cursor: range.end,
                mark: Some(range.start),
                goal: None,
            })
            .collect();
        let primary = selections.primary_index();
        if let Some(set) = SelectionSet::from_parts(parts, primary) {
            let mut set = set.with_ring(*selections.mark_ring());
            set.set_mark_active(MarkActive::Active);
            *selections = set;
        }
    }
}

/// Why `M-y` in a prompt did nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YankPopError {
    /// The command before was no yank, or the text changed since.
    NotAfterYank,
    /// No entry but the one yanked, or only secret ones.
    NoOther,
    /// The yank refused.
    Yank(YankError),
}

impl std::fmt::Display for YankPopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAfterYank => f.write_str("Previous command was not a yank"),
            Self::NoOther => f.write_str("No other kill to yank"),
            Self::Yank(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for YankPopError {}

/// The row of `entry` in a `M-y` list.
fn candidate(entry: &KillEntry) -> Candidate {
    let lines: usize = entry
        .pieces()
        .iter()
        .flat_map(|piece| piece.parts())
        .map(|part| part.max_point().row)
        .sum::<usize>()
        + entry.pieces().len().max(1);
    let pieces = entry.pieces().len();
    let mut said = Vec::new();
    if entry.is_secret() {
        said.push("secret".to_owned());
    }
    if lines > 1 {
        said.push(format!("{lines} lines"));
    }
    if pieces > 1 {
        said.push(format!("{pieces} cursors"));
    }
    let annotation = said.join(" · ");
    if entry.is_secret() {
        return Candidate::new("", "", annotation).unmatched();
    }
    let mut text = String::with_capacity(ROW_BYTES);
    'read: for (index, piece) in entry.pieces().iter().enumerate() {
        let between = (index > 0).then_some(PIECE_SEPARATOR);
        for chunk in between.into_iter().chain(piece.chunks()) {
            for c in chunk.chars() {
                if text.len() + c.len_utf8() > ROW_BYTES {
                    break 'read;
                }
                text.push(c);
            }
        }
    }
    let shown: String = text
        .split('\n')
        .map(row_text)
        .collect::<Vec<_>>()
        .join(&LINE_BREAK.to_string());
    Candidate::new(shown, "", annotation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, Replacement};
    use crate::editor::Preview;
    use crate::kill::Piece;
    use crate::rope::Rope;

    /// An editor showing `text`, with `kills` in the ring, the oldest first, each its
    /// pieces and whether it is secret.
    fn editor(text: &str, kills: &[(&[&str], bool)]) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", text));
        for (pieces, secret) in kills {
            let pieces = pieces
                .iter()
                .map(|piece| Piece::from(Rope::from(*piece)))
                .collect();
            editor.kills.ring.push(pieces, *secret);
        }
        editor
    }

    /// One, two lines in two pieces, a secret, two lines, and one again.
    const KILLS: &[(&[&str], bool)] = &[
        (&["one"], false),
        (&["a\nb"], false),
        (&["key"], true),
        (&["x", "y"], false),
        (&["one"], false),
    ];

    fn rows(editor: &Editor) -> Vec<(String, String)> {
        let candidates = editor.prompt().unwrap().candidates().unwrap();
        candidates
            .shown(18)
            .map(|(_, candidate)| {
                (
                    candidate.text().to_owned(),
                    candidate.annotation().to_owned(),
                )
            })
            .collect()
    }

    fn type_in_prompt(editor: &mut Editor, text: &str) {
        let id = editor.prompt().unwrap().buffer();
        let end = editor.buffer(id).unwrap().text().len();
        let edit = Replacement {
            range: end..end,
            text,
        };
        editor.apply_edits(id, &[edit]).unwrap();
        editor.match_completion();
    }

    fn text(editor: &Editor) -> String {
        editor.current().1.text().to_string()
    }

    /// Opens the list, picks the row `place` shows, and yanks it, as `RET` does.
    fn pick(editor: &mut Editor, place: isize) -> Result<(), YankError> {
        editor.open_kill_list("Yank from kill ring:").unwrap();
        editor.move_completion(place).unwrap();
        editor.submit_prompt().unwrap();
        editor.yank_picked()
    }

    #[test]
    fn the_ring_is_listed_newest_first_once_and_a_secret_with_no_text() {
        let mut editor = editor("", KILLS);
        editor.open_kill_list("Yank from kill ring:").unwrap();
        let row = |text: &str, said: &str| (text.to_owned(), said.to_owned());
        assert_eq!(
            rows(&editor),
            [
                row("one", ""),
                row("x⤶y", "2 lines · 2 cursors"),
                row("", "secret"),
                row("a⤶b", "2 lines"),
            ]
        );
        assert!(editor.kill_list_open());
    }

    #[test]
    fn anything_typed_hides_a_secret_row_whatever_it_asks() {
        let mut editor = editor("", KILLS);
        editor.open_kill_list("Yank from kill ring:").unwrap();
        type_in_prompt(&mut editor, "!zzz");
        let texts: Vec<String> = rows(&editor).into_iter().map(|(text, _)| text).collect();
        assert_eq!(texts, ["one", "x⤶y", "a⤶b"]);
    }

    #[test]
    fn the_preview_shows_a_kill_cut_to_its_cells_and_withholds_a_secret() {
        let mut editor = editor("", &[(&["abcd\nefgh\nijkl"], false), (&["key"], true)]);
        editor.open_kill_list("Yank from kill ring:").unwrap();
        assert_eq!(editor.preview(), Some(Preview::Withheld));
        editor.move_completion(1).unwrap();
        let Some(Preview::Text { entry }) = editor.preview() else {
            panic!("a kill's text");
        };
        assert_eq!(editor.kill_preview(entry, 2, 3), ["abc", "efg"]);
    }

    #[test]
    fn a_kill_picked_is_yanked_and_goes_to_the_front() {
        let mut editor = editor("", KILLS);
        pick(&mut editor, 3).unwrap();
        assert_eq!(text(&editor), "a\nb");
        assert_eq!(editor.kill_ring().get(0).unwrap().joined(), "a\nb");
        assert!(!editor.kill_list_open());
    }

    #[test]
    fn right_after_a_yank_the_kill_picked_takes_its_place() {
        let mut editor = editor("", KILLS);
        editor.yank(0, false).unwrap();
        assert_eq!(text(&editor), "one");
        pick(&mut editor, 3).unwrap();
        assert_eq!(text(&editor), "a\nb");
        // Not right after one: the cursor moved, so it goes in at the cursor.
        let mut editor = self::editor("", KILLS);
        editor.yank(0, false).unwrap();
        editor.current_mut().0.selections.move_primary(0);
        pick(&mut editor, 3).unwrap();
        assert_eq!(text(&editor), "a\nbone");
    }

    #[test]
    fn yank_pop_goes_to_older_kills_past_secret_ones_and_round() {
        let mut editor = editor("", KILLS);
        assert_eq!(editor.yank_pop(), Err(YankPopError::NotAfterYank));
        editor.yank(0, false).unwrap();
        editor.yank_pop().unwrap();
        assert_eq!(text(&editor), "x\ny");
        editor.yank_pop().unwrap();
        assert_eq!(text(&editor), "a\nb", "the secret one passed");
        editor.yank_pop().unwrap();
        editor.yank_pop().unwrap();
        assert_eq!(text(&editor), "one", "round to the newest");
        editor.current_mut().0.selections.move_primary(0);
        assert_eq!(editor.yank_pop(), Err(YankPopError::NotAfterYank));
    }

    #[test]
    fn m_y_in_isearch_puts_the_next_older_kill_in_place_past_secret_ones() {
        let mut editor = editor(
            "one x\ny a\nb",
            &[(&["a\nb"], false), (&["key"], true), (&["one"], false)],
        );
        editor.isearch_begin(false);
        assert_eq!(editor.isearch_yank_pop(), Err(YankPopError::NotAfterYank));
        editor.isearch_yank_kill();
        assert_eq!(editor.isearch().unwrap().string(), "one");
        editor.isearch_yank_pop().unwrap();
        assert_eq!(editor.isearch().unwrap().string(), "a\nb");
        assert!(
            !editor.isearch().unwrap().around_secret(),
            "the secret one was passed over"
        );
    }
}
