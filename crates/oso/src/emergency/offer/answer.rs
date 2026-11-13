//! What comes of each of a person's answers about a copy (ADR 0064).
//!
//! Must not depend on: `app`, `dispatch` or winit; what must happen off the UI thread comes back
//! as a [`CopyWork`].

use std::path::PathBuf;
use std::time::Instant;

use oso_core::buffer::{Buffer, BufferId, Mode, Verdict};
use oso_core::decode::LineEnding;
use oso_core::editor::{Editor, RecoveryAction};

use super::work::{Compared, Comparing, Deleting};
use super::{CopyLoad, CopyWork, Offer, PATIENCE, Recovered, Shown};
use crate::emergency::copies::{HEADER_ROWS, MAX_LOOK, Row, RowState};

/// What the echo area says while a compare runs.
const COMPARING: &str = "Comparing…";

/// The longest name of a copy in a buffer's name. A `.meta` can say anything.
const MOST_OF_A_NAME: usize = 256;

/// A buffer's name for `name` of a copy, cut on a character. The modeline escapes what it draws.
fn copy_buffer_name(prefix: &str, name: &str) -> String {
    let end = name.floor_char_boundary(MOST_OF_A_NAME);
    format!("*{prefix} {}*", &name[..end])
}

/// Whether what `verdict` says of a copy keeps its text from dev tooling.
fn is_withheld(verdict: &(Vec<PathBuf>, Verdict)) -> bool {
    let mut buffer = Buffer::new("");
    buffer.judge_copy(&verdict.0, verdict.1);
    buffer.is_secret()
}

/// Says what depends on a copy's text. If that text is withheld, it goes in the echo area alone,
/// as these run after their command has ended, and `Editor::message` holds only a command to
/// that. With no withheld buffer on the screen the socket reads the echo area too, so there it
/// says only `in_general`.
fn say_of_text(
    editor: &mut Editor,
    verdict: &(Vec<PathBuf>, Verdict),
    text: String,
    in_general: &str,
) {
    if !is_withheld(verdict) {
        editor.message(text);
    } else if editor.shows_secret() {
        editor.echo(text);
    } else {
        editor.echo(in_general);
    }
}

impl Offer {
    /// Does what a person asked. What must happen off this thread comes back.
    pub fn answer(&mut self, editor: &mut Editor, action: RecoveryAction) -> Vec<CopyWork> {
        match action {
            RecoveryAction::View(line) => match self.row_at(editor, line) {
                Some(index) => self.look(editor, index),
                None => no_row(editor),
            },
            RecoveryAction::Compare(line) => match self.row_at(editor, line) {
                Some(index) => self.compare(editor, index),
                None => no_row(editor),
            },
            RecoveryAction::Recover(line) => match self.row_at(editor, line) {
                Some(index) => self.recover(editor, index),
                None => no_row(editor),
            },
            RecoveryAction::ForgetMarks => {
                self.marked.clear();
                Vec::new()
            }
            RecoveryAction::MarkForDelete(line) => match self.row_at(editor, line) {
                Some(index) => {
                    let row = &self.rows[index];
                    self.marked.push(row.text.clone());
                    editor.echo(format!(
                        "Press d again to delete the emergency copy of {} for good",
                        row.name
                    ));
                    Vec::new()
                }
                None => no_row(editor),
            },
            RecoveryAction::Delete(line) => match self.row_at(editor, line) {
                // Rows can change between the two keys, and this cannot be undone.
                Some(index) if !self.marked.contains(&self.rows[index].text) => {
                    editor.echo("That row has changed, so nothing was deleted");
                    Vec::new()
                }
                Some(index) => self.delete(editor, index),
                None => no_row(editor),
            },
            RecoveryAction::Refresh => self.ask_for_scan(editor),
            RecoveryAction::Toggled => self.toggled(editor),
            RecoveryAction::WentBack => self.went_back(editor),
        }
    }

    /// The row a line means: the list's, or in a buffer showing one copy, that copy.
    fn row_at(&self, editor: &Editor, line: usize) -> Option<usize> {
        match editor.current().1.mode() {
            // A line of anything else means nothing here, as a command can run anywhere.
            Mode::Fundamental | Mode::Rust | Mode::Markdown | Mode::Toml | Mode::Terminal => None,
            Mode::CopyList => {
                let index = line.checked_sub(HEADER_ROWS)?;
                (index < self.rows.len()).then_some(index)
            }
            Mode::CopyView => {
                let key = self.shown_key(editor)?;
                self.rows.iter().position(|row| row.text == *key)
            }
        }
    }

    /// Which copy the window shows, if it shows one.
    fn shown_key(&self, editor: &Editor) -> Option<&PathBuf> {
        let on = editor.window().buffer();
        [&self.view, &self.compare]
            .into_iter()
            .flatten()
            .find(|shown| shown.buffer == on)
            .map(|shown| &shown.key)
    }

    fn toggled(&mut self, editor: &mut Editor) -> Vec<CopyWork> {
        let shows_list = editor.current().1.mode() == Mode::CopyList;
        if !shows_list {
            return self.free_views(editor);
        }
        self.cursor_for = editor
            .place_behind_internal_of(editor.window().id())
            .map(|(buffer, _, _)| buffer);
        if let Some(list) = &self.list {
            editor.set_copies_text(list.clone(), HEADER_ROWS, None);
        }
        self.ask_for_scan(editor)
    }

    /// The window went back from a copy: to its row, or, from the list, away from them all.
    fn went_back(&mut self, editor: &mut Editor) -> Vec<CopyWork> {
        if editor.current().1.mode() != Mode::CopyList {
            return self.free_views(editor);
        }
        let shown = self.last_shown.as_ref();
        if let Some(row) = shown.and_then(|key| self.rows.iter().position(|row| row.text == *key)) {
            editor.set_copies_cursor(HEADER_ROWS + row);
        }
        Vec::new()
    }

    pub(super) fn free_views(&mut self, editor: &mut Editor) -> Vec<CopyWork> {
        self.last_shown = None;
        let shown = [self.view.take(), self.compare.take()];
        shown
            .into_iter()
            .flatten()
            .filter_map(|shown| editor.remove_internal_buffer(shown.buffer))
            .map(|buffer| CopyWork::Free(Box::new(buffer)))
            .collect()
    }

    /// Judges `buffer` as what it shows of `row` allows.
    fn judge(editor: &mut Editor, buffer: BufferId, verdict: &(Vec<PathBuf>, Verdict)) {
        if let Some(buffer) = editor.buffer_mut(buffer) {
            buffer.judge_copy(&verdict.0, verdict.1);
        }
    }

    fn verdict_of(row: &Row) -> (Vec<PathBuf>, Verdict) {
        (row.paths.clone(), row.at_least)
    }

    fn look(&mut self, editor: &mut Editor, index: usize) -> Vec<CopyWork> {
        let row = &self.rows[index];
        if let Some(view) = &self.view
            && view.key == row.text
            && editor.buffer(view.buffer).is_some()
        {
            editor.show(view.buffer);
            self.last_shown = Some(row.text.clone());
            return Vec::new();
        }
        if row.size >= MAX_LOOK {
            editor.message(format!(
                "This copy is too large to read here. Its text is in {}",
                row.text.display()
            ));
            return Vec::new();
        }
        let buffer =
            editor.add_internal_buffer(copy_buffer_name("copy of", &row.name), Mode::CopyView);
        let verdict = Self::verdict_of(row);
        Self::judge(editor, buffer, &verdict);
        if let Some(shown) = editor.buffer_mut(buffer) {
            shown.begin_loading();
        }
        let key = row.text.clone();
        let limit = row.size;
        let work: Vec<CopyWork> = self
            .view
            .take()
            .and_then(|old| editor.remove_internal_buffer(old.buffer))
            .map(|buffer| CopyWork::Free(Box::new(buffer)))
            .into_iter()
            .collect();
        editor.show(buffer);
        self.load = Some(CopyLoad {
            buffer,
            path: key.clone(),
            limit,
        });
        self.last_shown = Some(key.clone());
        self.view = Some(Shown { buffer, key });
        work
    }

    fn compare(&mut self, editor: &mut Editor, index: usize) -> Vec<CopyWork> {
        let row = &self.rows[index];
        let name = row.name.clone();
        let has_file = matches!(
            row.state,
            RowState::Unchanged | RowState::Changed | RowState::Damaged | RowState::Recovered
        );
        let Some(file) = row.location.clone().filter(|_| has_file) else {
            editor.message(format!("{name} has no file open here to compare it with"));
            return Vec::new();
        };
        if self
            .comparing
            .is_some_and(|(_, since)| since.elapsed() < PATIENCE)
        {
            editor.echo("Still comparing the last one");
            return Vec::new();
        }
        let id = self.next_compare;
        self.next_compare += 1;
        self.comparing = Some((id, Instant::now()));
        editor.echo(COMPARING);
        vec![CopyWork::Compare(Comparing {
            id,
            key: row.text.clone(),
            name,
            file,
            verdict: Self::verdict_of(row),
        })]
    }

    pub(super) fn take_compared(
        &mut self,
        editor: &mut Editor,
        compared: Compared,
    ) -> Vec<CopyWork> {
        let Compared {
            id,
            key,
            name,
            verdict,
            result,
        } = compared;
        // One given up on may end while a later one runs, and nobody is waiting for it.
        if self.comparing.is_none_or(|(latest, _)| latest != id) {
            return Vec::new();
        }
        self.comparing = None;
        editor.clear_echo_saying(COMPARING);
        let diff = match result {
            Ok(diff) => diff,
            Err(known) => {
                let text = format!("Comparing {name}: {known}");
                say_of_text(editor, &verdict, text, "This copy cannot be compared here");
                return Vec::new();
            }
        };
        if !editor.current().1.mode().shows_copies() {
            editor.message(format!("The compare of {name} is ready: C-h r, then ="));
            return Vec::new();
        }
        let buffer = editor.add_internal_buffer(copy_buffer_name("compare", &name), Mode::CopyView);
        Self::judge(editor, buffer, &verdict);
        if let Some(shown) = editor.buffer_mut(buffer) {
            shown.apply_load(diff);
        }
        let work: Vec<CopyWork> = self
            .compare
            .take()
            .and_then(|old| editor.remove_internal_buffer(old.buffer))
            .map(|buffer| CopyWork::Free(Box::new(buffer)))
            .into_iter()
            .collect();
        editor.show(buffer);
        self.last_shown = Some(key.clone());
        self.compare = Some(Shown { buffer, key });
        work
    }

    /// The first `r` looks, and the second, on what that shows, puts the text in the buffer.
    fn recover(&mut self, editor: &mut Editor, index: usize) -> Vec<CopyWork> {
        let row = &self.rows[index];
        if let Some(why) = no_recovery(row) {
            editor.message(why);
            return Vec::new();
        }
        let shown = self
            .view
            .as_ref()
            .filter(|view| view.key == row.text && view.buffer == editor.window().buffer())
            .map(|view| view.buffer);
        let Some(view) = shown else {
            let name = row.name.clone();
            let work = self.look(editor, index);
            editor.message(format!("Press r again to put this text in {name}"));
            return work;
        };
        let row = row.clone();
        match refusal(editor, &row, view) {
            Some(why) => {
                let in_general = "This copy cannot be recovered now";
                say_of_text(editor, &Self::verdict_of(&row), why, in_general);
                Vec::new()
            }
            None => self.put_in_buffer(editor, row, view),
        }
    }

    fn put_in_buffer(&mut self, editor: &mut Editor, row: Row, view: BufferId) -> Vec<CopyWork> {
        let Some(target) = row.target else {
            return Vec::new();
        };
        // Back where the person was first, as that is what an undo returns them to. The way back
        // goes with it, and with that the view.
        editor.leave_all_internal();
        if editor.window().buffer() != target {
            editor.show(target);
        }
        editor.begin_undo_group();
        let replaced = editor.replace_text_from(target, view);
        editor.end_undo_group();
        if let Err(e) = replaced {
            editor.message(format!("Cannot recover {}: {e}", row.name));
            return Vec::new();
        }
        let private = set_format(editor, target, &row);
        if let Some(buffer) = editor.buffer_mut(target) {
            buffer.set_copy_waits(false);
        }
        editor.message(recovered_message(&row, private));
        // What an earlier recovery here put in is what this replaces: it was undone, or its text
        // is in its file. Its copy stays for a person to answer.
        self.recovered
            .retain(|recovered| recovered.buffer != target);
        self.recovered.push(Recovered {
            row,
            buffer: target,
            stands: true,
        });
        let mut work = self.free_views(editor);
        work.extend(self.ask_for_scan(editor));
        work
    }

    fn delete(&mut self, editor: &mut Editor, index: usize) -> Vec<CopyWork> {
        let row = &self.rows[index];
        let (text, meta, name) = (row.text.clone(), row.meta.clone(), row.name.clone());
        self.marked.retain(|marked| *marked != text);
        // The buffer keeps its mark: a redo can still bring this text back, with all that goes
        // with it. Only the copy has gone, which nothing now has to remove.
        self.recovered
            .retain(|recovered| recovered.row.text != text);
        let mut work = Vec::new();
        for shown in [&mut self.view, &mut self.compare] {
            if shown.as_ref().is_some_and(|shown| shown.key == text)
                && let Some(gone) = shown.take()
                && let Some(buffer) = editor.remove_internal_buffer(gone.buffer)
            {
                work.push(CopyWork::Free(Box::new(buffer)));
            }
        }
        work.push(CopyWork::Delete(Deleting { text, meta, name }));
        work
    }
}

/// Why `row` cannot go in a buffer, if it cannot.
fn no_recovery(row: &Row) -> Option<String> {
    let name = &row.name;
    match row.state {
        RowState::Unchanged | RowState::Changed | RowState::Gone => None,
        RowState::NoFile if row.target.is_some() => None,
        RowState::NoFile => Some(format!("No empty buffer named {name} is here to put it in")),
        RowState::NotOpen => Some(format!("Open {} first, with oso PATH", row.shown)),
        RowState::LeadsElsewhere => Some(format!(
            "{} now leads somewhere other than where this copy belongs",
            row.shown
        )),
        RowState::Recovered => Some(format!("{name} already holds this text")),
        RowState::Damaged => Some("This copy has been changed since it was written".to_owned()),
        RowState::Unknown | RowState::LeftBySave => {
            Some("Nothing says where this text belongs".to_owned())
        }
    }
}

/// Says that there is no copy on the line.
fn no_row(editor: &mut Editor) -> Vec<CopyWork> {
    editor.message("No emergency copy on this row");
    Vec::new()
}

/// Why this cannot happen now, if it cannot. Nothing is moved before this says.
fn refusal(editor: &Editor, row: &Row, view: BufferId) -> Option<String> {
    let name = &row.name;
    let source = editor.buffer(view)?;
    if source.loading().is_some() {
        return Some("The copy is still loading".to_owned());
    }
    if source.file_info().binary || !source.invalid_bytes().is_empty() {
        return Some("This copy has been changed since it was written: it is not text".to_owned());
    }
    // A copy holds text with no BOM of its own, so these bytes at its start are text, which a
    // load takes for a BOM. It would not come back exactly, and its copy would then go.
    if source.file_info().bom {
        return Some(format!(
            "This copy starts with a byte order mark, which would be lost. Its text is in {}",
            row.text.display()
        ));
    }
    if source.text().len() as u64 != row.size {
        return Some("This copy changed as it was read. Look at it again".to_owned());
    }
    let Some(target) = row.target.and_then(|target| editor.buffer(target)) else {
        return Some(format!("{name} is not open any more"));
    };
    if target.loading().is_some() {
        return Some(format!("{name} is still loading"));
    }
    if target.read_only().is_some() {
        return Some(format!("{name} is read-only"));
    }
    if target.is_saving() {
        return Some(format!("{name} is being saved"));
    }
    if target.is_modified() {
        return Some(format!(
            "{name} has unsaved edits. Save them or undo them first"
        ));
    }
    if row.state == RowState::NoFile && !target.text().is_empty() {
        return Some(format!("{name} is not empty, so nothing goes in it"));
    }
    // The window goes to the target. Until `C-x b`, no key could then reach what it was on.
    if let Some((behind, _, _)) = editor.place_behind_internal_of(editor.window().id())
        && Some(behind) != row.target
        && let Some(behind) = editor.buffer(behind)
        && behind.is_modified()
    {
        return Some(format!(
            "{} has unsaved edits, and nothing would lead back to it. Save them first",
            behind.name()
        ));
    }
    None
}

/// With no file there, the copy says what a save should write, and a file that comes back does
/// so as `0600`. That goes by what the buffer knows and not by what a `.meta` says. Returns
/// whether it will.
fn set_format(editor: &mut Editor, target: BufferId, row: &Row) -> bool {
    let Some(buffer) = editor.buffer_mut(target) else {
        return false;
    };
    let visits_a_file = buffer.path().is_some();
    let no_file_there = buffer
        .baseline()
        .is_none_or(|baseline| baseline.file.is_none());
    if no_file_there {
        let line_ending = row.line_ending.unwrap_or(LineEnding::Lf);
        buffer.take_format_of_copy(row.bom, line_ending, visits_a_file);
    }
    no_file_there && visits_a_file
}

/// What a recovery says, which is more where saving does something a person should know.
fn recovered_message(row: &Row, comes_back_private: bool) -> String {
    let name = &row.name;
    match row.state {
        _ if comes_back_private => format!(
            "Recovered {name}. Its file is not there, and C-x C-s makes it with mode 0600. C-z undoes this"
        ),
        RowState::Changed => format!(
            "Recovered {name}. The file changed since this copy was made, so C-x C-s replaces what is there now. C-z undoes this"
        ),
        _ => format!("Recovered {name}. C-x C-s saves it, and C-z undoes this"),
    }
}
