//! Offering emergency copies, and what comes of a person's answer (ADR 0064). This holds what
//! was found, and says what the host must start, with no window, so that each answer has a
//! test.
//!
//! Must not depend on: `app`, `dispatch` or winit; the host owns threads and files, and does
//! what a [`CopyWork`] says.
//!
//! A copy only ever reaches the buffer of a file the person opened, and only on their answer.
//! It then goes when a save of that buffer ends with the disk's confirmation, so that the
//! text is safe first.

use std::path::PathBuf;
use std::time::SystemTime;

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::editor::Editor;
use oso_core::load::LoadSnapshot;

use super::copies::{HEADER_ROWS, Row, Scan, ScanRequest, Word, age, targets_of};

mod answer;
mod work;

use work::Deleting;
pub use work::{CopiesEvent, CopyWork};

/// A buffer showing one copy.
struct Shown {
    buffer: BufferId,
    key: PathBuf,
}

/// The copies found, and what has become of them.
pub struct Offer {
    directory: Option<PathBuf>,
    rows: Vec<Row>,
    /// The number of the next scan. The one at startup is 0.
    next_scan: u64,
    applied_scan: Option<u64>,
    /// The copies a notice has told of, and those a notice has told of as a file's.
    told: Vec<PathBuf>,
    told_for_a_file: Vec<PathBuf>,
    told_problem: bool,
    /// Copies whose text went in a buffer, until the copy has gone.
    recovered: Vec<(Row, BufferId)>,
    view: Option<Shown>,
    compare: Option<Shown>,
    /// The list's rows, for when it is first shown.
    list: Option<LoadSnapshot>,
    /// Puts the cursor on this buffer's row when the list next changes.
    cursor_for: Option<BufferId>,
}

impl Offer {
    /// An offer of what is in `directory`. Nothing is read until a scan.
    pub fn new(directory: Option<PathBuf>) -> Self {
        Self {
            directory,
            rows: Vec::new(),
            next_scan: 1,
            applied_scan: None,
            told: Vec::new(),
            told_for_a_file: Vec::new(),
            told_problem: false,
            recovered: Vec::new(),
            view: None,
            compare: None,
            list: None,
            cursor_for: None,
        }
    }

    /// What a scan needs to be told now.
    pub fn scan_request(&mut self, editor: &Editor) -> ScanRequest {
        let id = self.next_scan;
        self.next_scan += 1;
        let stands = |buffer: BufferId| editor.buffer(buffer).is_some_and(Buffer::recovery_stands);
        ScanRequest {
            id,
            directory: self.directory.clone(),
            targets: targets_of(editor),
            recovered: self
                .recovered
                .iter()
                .filter(|(_, buffer)| stands(*buffer))
                .map(|(row, _)| row.text.clone())
                .collect(),
        }
    }

    /// What a thread reported, as an event. A list that is being read also tells whether a
    /// file is still loading, as it may then have more to say at once.
    pub fn take_event(
        &mut self,
        editor: &mut Editor,
        event: CopiesEvent,
        loading: bool,
    ) -> Vec<CopyWork> {
        match event {
            CopiesEvent::Scan(scan) => {
                self.take_scan(editor, *scan, loading);
                Vec::new()
            }
            CopiesEvent::Compared(compared) => self.take_compared(editor, *compared),
            CopiesEvent::Deleted { name, problem } => {
                match problem {
                    None => editor.message(format!("Deleted the emergency copy of {name}")),
                    Some(problem) => {
                        editor.message(format!("Cannot delete the copy of {name}: {problem}"));
                    }
                }
                vec![CopyWork::Scan(self.scan_request(editor))]
            }
        }
    }

    /// Takes in what a scan found, unless a later one has come in already.
    pub fn take_scan(&mut self, editor: &mut Editor, scan: Scan, loading: bool) {
        if self.applied_scan.is_some_and(|applied| scan.id < applied) {
            return;
        }
        self.applied_scan = Some(scan.id);
        for stem in &scan.removed {
            editor.message(format!(
                "Removed the emergency copy {stem}: its file holds the same text"
            ));
        }
        if let Some(problem) = &scan.problem
            && !self.told_problem
        {
            self.told_problem = true;
            editor.message(format!("Emergency copies are not offered: {problem}"));
        }
        self.rows = scan.rows;
        let shows_list = editor.current().1.mode() == Mode::EmergencyCopies;
        let line = self.list_line(editor, shows_list);
        if editor.has_copies_list() {
            editor.set_copies_text(scan.list.clone(), line);
        }
        self.list = Some(scan.list);
        self.update_warnings(editor);
        if !loading {
            self.tell(editor);
        }
    }

    /// Where the list's cursor belongs, now that its rows have changed.
    fn list_line(&mut self, editor: &Editor, shows_list: bool) -> usize {
        let last = HEADER_ROWS + self.rows.len().saturating_sub(1);
        let wanted = self.cursor_for.take().and_then(|buffer| {
            let row = self
                .rows
                .iter()
                .position(|row| row.target == Some(buffer))?;
            Some(HEADER_ROWS + row)
        });
        match wanted {
            Some(line) => line,
            None if shows_list => editor.cursor_line().clamp(HEADER_ROWS, last),
            None => HEADER_ROWS,
        }
    }

    /// Sets the modeline's warning on each buffer of the user's.
    fn update_warnings(&self, editor: &mut Editor) {
        let ids: Vec<BufferId> = editor.buffers().map(|(id, _)| id).collect();
        for id in ids {
            let waits = self
                .rows
                .iter()
                .any(|row| row.target == Some(id) && row.word != Word::Recovered);
            if let Some(buffer) = editor.buffer_mut(id) {
                buffer.set_copy_waits(waits);
            }
        }
    }

    /// Says what has not been said: each file's copy once, and how many others wait.
    fn tell(&mut self, editor: &mut Editor) {
        let now = SystemTime::now();
        let mut saying = Vec::new();
        for row in &self.rows {
            if row.target.is_some()
                && row.word != Word::Recovered
                && !self.told_for_a_file.contains(&row.text)
            {
                self.told_for_a_file.push(row.text.clone());
                self.told.push(row.text.clone());
                saying.push(format!(
                    "{} has an emergency copy, from {}: C-h r",
                    row.name,
                    age(row.written, now)
                ));
            }
        }
        let others = self
            .rows
            .iter()
            .filter(|row| !self.told.contains(&row.text))
            .count();
        if others > 0 {
            for row in &self.rows {
                if !self.told.contains(&row.text) {
                    self.told.push(row.text.clone());
                }
            }
            let (verb, what) = if others == 1 {
                ("waits", "copy")
            } else {
                ("wait", "copies")
            };
            saying.push(format!("{others} emergency {what} {verb}: C-h r"));
        }
        for said in saying {
            editor.message(said);
        }
    }

    /// A save of `buffer` has ended. If it reached the disk, confirmed, holding what was
    /// recovered there, the copy has done its work, and goes.
    pub fn save_ended(
        &mut self,
        editor: &mut Editor,
        buffer: BufferId,
        confirmed: bool,
    ) -> Vec<CopyWork> {
        let holds = editor
            .buffer(buffer)
            .is_some_and(|shown| shown.recovery_stands() && !shown.is_modified());
        let mut work = Vec::new();
        if confirmed && holds {
            let (done, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.recovered)
                .into_iter()
                .partition(|(_, id)| *id == buffer);
            self.recovered = kept;
            work.extend(done.into_iter().map(|(row, _)| {
                CopyWork::Delete(Deleting {
                    text: row.text,
                    meta: row.meta,
                    name: row.name,
                })
            }));
            if let Some(shown) = editor.buffer_mut(buffer) {
                shown.forget_recovery();
            }
        }
        if work.is_empty() && self.rows.iter().any(|row| row.target == Some(buffer)) {
            // The file's stamp has changed, so what its rows say may have.
            work.push(CopyWork::Scan(self.scan_request(editor)));
        }
        work
    }
}

#[cfg(test)]
mod tests;
