//! What comes of each of a person's answers about a copy (ADR 0064).
//!
//! Must not depend on: `app`, `dispatch` or winit; what must happen off the UI thread comes back
//! as a [`CopyWork`].

use std::path::PathBuf;
use std::time::SystemTime;

use oso_core::buffer::{BufferId, FileStamp, Mode, Verdict};
use oso_core::decode::LineEnding;
use oso_core::editor::{CopyAction, Editor, copy_buffer_name};

use super::work::{Compared, Comparing, Deleting};
use super::{CopyWork, Offer, Shown};
use crate::emergency::copies::{HEADER_ROWS, MAX_LOOK, Row, Word};

impl Offer {
    /// Does what a person asked. What must happen off this thread comes back.
    pub fn answer(&mut self, editor: &mut Editor, action: CopyAction) -> Vec<CopyWork> {
        match action {
            CopyAction::View(line) => match self.row_at(editor, line) {
                Some(index) => self.look(editor, index),
                None => no_row(editor),
            },
            CopyAction::Compare(line) => match self.row_at(editor, line) {
                Some(index) => self.compare(editor, index),
                None => no_row(editor),
            },
            CopyAction::Recover(line) => match self.row_at(editor, line) {
                Some(index) => self.recover(editor, index),
                None => no_row(editor),
            },
            CopyAction::Delete(line) => match self.row_at(editor, line) {
                Some(index) => self.delete(editor, index),
                None => no_row(editor),
            },
            CopyAction::Refresh => vec![CopyWork::Scan(self.scan_request(editor))],
            CopyAction::Toggled => self.toggled(editor),
            CopyAction::WentBack => self.went_back(editor),
        }
    }

    /// The row a line means: the list's, or in a buffer showing one copy, that copy.
    fn row_at(&self, editor: &Editor, line: usize) -> Option<usize> {
        match editor.current().1.mode() {
            // A line of anything else means nothing here, as a command can run anywhere.
            Mode::Fundamental => None,
            Mode::EmergencyCopies => {
                let index = line.checked_sub(HEADER_ROWS)?;
                (index < self.rows.len()).then_some(index)
            }
            Mode::EmergencyCopy => {
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
        let shows_list = editor.current().1.mode() == Mode::EmergencyCopies;
        if !shows_list {
            return self.free_views(editor);
        }
        self.cursor_for = editor.place_behind_internal().map(|(buffer, _, _)| buffer);
        if let Some(list) = &self.list {
            editor.set_copies_text(list.clone(), HEADER_ROWS);
        }
        vec![CopyWork::Scan(self.scan_request(editor))]
    }

    /// The window went back from a copy: to its row, or, from the list, away from them all.
    fn went_back(&mut self, editor: &mut Editor) -> Vec<CopyWork> {
        if editor.current().1.mode() != Mode::EmergencyCopies {
            return self.free_views(editor);
        }
        let on = |key: &PathBuf| self.rows.iter().position(|row| row.text == *key);
        let shown = self.view.iter().chain(self.compare.iter());
        if let Some(row) = shown.filter_map(|shown| on(&shown.key)).next() {
            editor.set_copies_cursor(HEADER_ROWS + row);
        }
        Vec::new()
    }

    pub(super) fn free_views(&mut self, editor: &mut Editor) -> Vec<CopyWork> {
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
            editor.add_internal_buffer(copy_buffer_name("copy of", &row.name), Mode::EmergencyCopy);
        let verdict = Self::verdict_of(row);
        Self::judge(editor, buffer, &verdict);
        if let Some(shown) = editor.buffer_mut(buffer) {
            shown.begin_loading();
        }
        let key = row.text.clone();
        let limit = row.size;
        let mut work: Vec<CopyWork> = self
            .view
            .take()
            .and_then(|old| editor.remove_internal_buffer(old.buffer))
            .map(|buffer| CopyWork::Free(Box::new(buffer)))
            .into_iter()
            .collect();
        editor.show(buffer);
        work.push(CopyWork::Load {
            buffer,
            path: key.clone(),
            limit,
        });
        self.view = Some(Shown { buffer, key });
        work
    }

    fn compare(&mut self, editor: &mut Editor, index: usize) -> Vec<CopyWork> {
        let row = &self.rows[index];
        let name = row.name.clone();
        let has_file = matches!(row.word, Word::Unchanged | Word::Changed | Word::Damaged);
        let Some(file) = row.location.clone().filter(|_| has_file) else {
            editor.message(format!("{name} has no file here to compare it with"));
            return Vec::new();
        };
        editor.echo("Comparing…");
        vec![CopyWork::Compare(Comparing {
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
            key,
            name,
            verdict,
            result,
        } = compared;
        let diff = match result {
            Ok(diff) => diff,
            Err(known) => {
                editor.message(format!("Comparing {name}: {known}"));
                return Vec::new();
            }
        };
        // What said it was comparing is over.
        editor.clear_echo();
        if !editor.current().1.mode().shows_copies() {
            editor.message(format!("The compare of {name} is ready: C-h r, then ="));
            return Vec::new();
        }
        let buffer =
            editor.add_internal_buffer(copy_buffer_name("compare", &name), Mode::EmergencyCopy);
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
                editor.message(why);
                Vec::new()
            }
            None => self.put_in_buffer(editor, row, view),
        }
    }

    fn put_in_buffer(&mut self, editor: &mut Editor, row: Row, view: BufferId) -> Vec<CopyWork> {
        let Some(target) = row.target else {
            return Vec::new();
        };
        editor.show(target);
        editor.begin_undo_group();
        let replaced = editor.replace_text_from(target, view);
        editor.end_undo_group();
        if let Err(e) = replaced {
            editor.message(format!("Cannot recover {}: {e}", row.name));
            return Vec::new();
        }
        set_format(editor, target, &row);
        if let Some(buffer) = editor.buffer_mut(target) {
            buffer.set_copy_waits(false);
        }
        editor.message(recovered_message(&row));
        self.recovered.push((row, target));
        // The window has moved on, so the way back goes, and with it the view.
        editor.leave_copies();
        editor.show(target);
        let mut work = self.free_views(editor);
        work.push(CopyWork::Scan(self.scan_request(editor)));
        work
    }

    fn delete(&mut self, editor: &mut Editor, index: usize) -> Vec<CopyWork> {
        let row = &self.rows[index];
        let (text, meta, name) = (row.text.clone(), row.meta.clone(), row.name.clone());
        if let Some(position) = self.recovered.iter().position(|(row, _)| row.text == text) {
            let (_, buffer) = self.recovered.swap_remove(position);
            if let Some(buffer) = editor.buffer_mut(buffer) {
                buffer.forget_recovery();
            }
        }
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

/// Why `word` cannot go in a buffer, if it cannot.
fn no_recovery(row: &Row) -> Option<String> {
    let name = &row.name;
    match row.word {
        Word::Unchanged | Word::Changed | Word::Gone => None,
        Word::NoFile if row.target.is_some() => None,
        Word::NoFile => Some(format!("{name} is not empty, so nothing goes in it")),
        Word::NotOpen => Some(format!("Open {} first, with oso PATH", row.shown)),
        Word::LeadsElsewhere => Some(format!(
            "{} now leads somewhere other than where this copy belongs",
            row.shown
        )),
        Word::Recovered => Some(format!("{name} already holds this text")),
        Word::Damaged => Some("This copy has been changed since it was written".to_owned()),
        Word::Unknown | Word::LeftBySave => Some("Nothing says where this text belongs".to_owned()),
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
    if row.word == Word::NoFile && !target.text().is_empty() {
        return Some(format!("{name} is not empty, so nothing goes in it"));
    }
    None
}

/// With no file there, the copy says what a save should write, and a file that has gone comes
/// back as `0600`.
fn set_format(editor: &mut Editor, target: BufferId, row: &Row) {
    let no_file_there = matches!(row.word, Word::Gone | Word::NoFile);
    let Some(buffer) = editor.buffer_mut(target) else {
        return;
    };
    if no_file_there {
        buffer.set_file_format(row.bom, row.line_ending.unwrap_or(LineEnding::Lf));
    }
    if row.word == Word::Gone {
        buffer.expect_a_file_that_has_gone(FileStamp {
            modified: row.file_modified.unwrap_or(SystemTime::UNIX_EPOCH),
            len: row.file_bytes.unwrap_or(0),
            mode: 0o600,
        });
    }
}

/// What a recovery says, which is more where saving does something a person should know.
fn recovered_message(row: &Row) -> String {
    let name = &row.name;
    match row.word {
        Word::Changed => format!(
            "Recovered {name}. The file changed since this copy was made, so C-x C-s replaces what is there now. C-z undoes this"
        ),
        Word::Gone => format!(
            "Recovered {name}. Its file has gone, and C-x C-s brings it back with mode 0600. C-z undoes this"
        ),
        _ => format!("Recovered {name}. C-x C-s saves it, and C-z undoes this"),
    }
}
