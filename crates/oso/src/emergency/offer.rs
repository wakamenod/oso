//! Offering emergency copies, and what comes of a person's answer (ADR 0064). This holds what
//! was found, and says what the host must start, with no window, so that each answer has a
//! test.
//!
//! Must not depend on: `app`, `dispatch` or winit; nothing here starts a thread or touches a
//! file. The host does what a [`CopyWork`] says.
//!
//! A copy only ever reaches the buffer of a file the person opened, and only on their answer.
//! It then goes when a save of that buffer ends with the disk's confirmation, so that the
//! text is safe first.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::editor::Editor;
use oso_core::load::LoadSnapshot;

use super::copies::{HEADER_ROWS, Row, RowState, Scan, ScanRequest, age, targets_of};
use super::plural;

mod answer;
mod work;

use work::Deleting;
pub use work::{CopiesEvent, CopyWork};

/// How long a thread of the offer may go unheard from before another is started in its place,
/// as one may be stuck on a network home directory, or may never have started. It costs only
/// that thread, which goes with the process.
const PATIENCE: Duration = Duration::from_secs(60);

/// A buffer showing one copy.
struct Shown {
    buffer: BufferId,
    key: PathBuf,
}

/// A copy whose text went in a buffer.
struct Recovered {
    row: Row,
    buffer: BufferId,
    /// No undo has taken it back, as far as was last looked.
    stands: bool,
}

/// The copies found, and what has become of them.
pub struct Offer {
    directory: Option<PathBuf>,
    rows: Vec<Row>,
    /// The number of the next scan. The one at startup is 0.
    next_scan: u64,
    applied_scan: Option<u64>,
    /// When the scan running started, as one at a time is enough however fast keys come.
    scanning: Option<Instant>,
    /// Something changed while it ran, so that it may already be out of date.
    scan_again: bool,
    /// When the compare running started, as each can hold a lot of memory.
    comparing: Option<Instant>,
    /// The copies a notice has told of, and those a notice has told of as a file's.
    told: HashSet<PathBuf>,
    told_for_a_file: HashSet<PathBuf>,
    told_problem: bool,
    /// Copies whose text went in a buffer, until the copy has gone. A buffer has one at most,
    /// as what a later recovery replaces is what the person turned down.
    recovered: Vec<Recovered>,
    /// What a first `d` marked, by which a second knows it means the same copy.
    marked: Vec<PathBuf>,
    /// What the window last showed one copy of, for the way back to its row.
    last_shown: Option<PathBuf>,
    view: Option<Shown>,
    compare: Option<Shown>,
    /// The list's rows, for when it is first shown.
    list: Option<LoadSnapshot>,
    /// Puts the cursor on this buffer's row when the list next changes.
    cursor_for: Option<BufferId>,
}

/// Whether the recovery `buffer` remembers stands.
fn stands(editor: &Editor, buffer: BufferId) -> bool {
    editor.buffer(buffer).is_some_and(Buffer::recovery_stands)
}

impl Offer {
    /// An offer of what is in `directory`. Nothing is read until a scan.
    pub fn new(directory: Option<PathBuf>) -> Self {
        Self {
            directory,
            rows: Vec::new(),
            next_scan: 1,
            applied_scan: None,
            scanning: None,
            scan_again: false,
            comparing: None,
            told: HashSet::new(),
            told_for_a_file: HashSet::new(),
            told_problem: false,
            recovered: Vec::new(),
            marked: Vec::new(),
            last_shown: None,
            view: None,
            compare: None,
            list: None,
            cursor_for: None,
        }
    }

    /// The scan at startup has begun, so that the first one asked for waits for it.
    pub fn startup_scan_begins(&mut self) {
        self.scanning = Some(Instant::now());
    }

    /// A scan, or if one is running, another once it ends.
    pub fn ask_for_scan(&mut self, editor: &Editor) -> Vec<CopyWork> {
        if self
            .scanning
            .is_some_and(|since| since.elapsed() < PATIENCE)
        {
            self.scan_again = true;
            return Vec::new();
        }
        self.scanning = Some(Instant::now());
        vec![CopyWork::Scan(self.scan_request(editor))]
    }

    /// What a scan needs to be told now.
    fn scan_request(&mut self, editor: &Editor) -> ScanRequest {
        let id = self.next_scan;
        self.next_scan += 1;
        ScanRequest {
            id,
            directory: self.directory.clone(),
            targets: targets_of(editor),
            recovered: self
                .recovered
                .iter()
                .filter(|recovered| stands(editor, recovered.buffer))
                .map(|recovered| recovered.row.text.clone())
                .collect(),
        }
    }

    /// A command has ended. An undo or a redo may have changed whether a recovery stands, which
    /// changes what its row and its buffer's modeline say. It costs a look at an empty list.
    pub fn after_command(&mut self, editor: &Editor) -> Vec<CopyWork> {
        let mut changed = false;
        for recovered in &mut self.recovered {
            let now = stands(editor, recovered.buffer);
            changed |= now != recovered.stands;
            recovered.stands = now;
        }
        if changed {
            self.ask_for_scan(editor)
        } else {
            Vec::new()
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
            CopiesEvent::Scan(scan) => self.take_scan(editor, *scan, loading),
            CopiesEvent::Compared(compared) => self.take_compared(editor, *compared),
            CopiesEvent::Deleted { name, problem } => {
                match problem {
                    None => editor.message(format!("Deleted the emergency copy of {name}")),
                    Some(problem) => {
                        editor.message(format!("Cannot delete the copy of {name}: {problem}"));
                    }
                }
                self.ask_for_scan(editor)
            }
        }
    }

    /// Takes in what a scan found, unless a later one has come in already. What comes back
    /// is another scan, if something changed while this one ran.
    pub fn take_scan(&mut self, editor: &mut Editor, scan: Scan, loading: bool) -> Vec<CopyWork> {
        self.scanning = None;
        let again = if std::mem::take(&mut self.scan_again) {
            self.ask_for_scan(editor)
        } else {
            Vec::new()
        };
        if self.applied_scan.is_some_and(|applied| scan.id < applied) {
            return again;
        }
        self.applied_scan = Some(scan.id);
        match &scan.removed[..] {
            [] => {}
            [stem] => editor.message(format!(
                "Removed the emergency copy {stem}: its file holds the same text"
            )),
            many => editor.message(format!(
                "Removed {} emergency copies: their files hold the same text",
                many.len()
            )),
        }
        if let Some(problem) = &scan.problem
            && !self.told_problem
        {
            self.told_problem = true;
            editor.message(format!("Emergency copies are not offered: {problem}"));
        }
        let shows_list = editor.current().1.mode() == Mode::CopyList;
        // The cursor stays on its copy as the rows change, as a key acts on what is under it.
        let on_row = shows_list
            .then(|| editor.cursor_line().checked_sub(HEADER_ROWS))
            .flatten()
            .and_then(|index| self.rows.get(index))
            .map(|row| row.text.clone());
        self.rows = scan.rows;
        let line = self.list_line(editor, shows_list, on_row.as_ref());
        if editor.has_copies_list() {
            editor.set_copies_text(scan.list.clone(), line);
        }
        self.list = Some(scan.list);
        self.update_warnings(editor);
        if !loading {
            self.tell(editor);
        }
        again
    }

    /// Where the list's cursor belongs, now that its rows have changed.
    fn list_line(&mut self, editor: &Editor, shows_list: bool, on_row: Option<&PathBuf>) -> usize {
        let last = HEADER_ROWS + self.rows.len().saturating_sub(1);
        let position = |wanted: &dyn Fn(&Row) -> bool| self.rows.iter().position(wanted);
        let wanted = self
            .cursor_for
            .take()
            .and_then(|buffer| position(&|row| row.target == Some(buffer)))
            .or_else(|| on_row.and_then(|text| position(&|row| row.text == *text)))
            .map(|row| HEADER_ROWS + row);
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
                .any(|row| row.target == Some(id) && row.state != RowState::Recovered);
            if let Some(buffer) = editor.buffer_mut(id) {
                buffer.set_copy_waits(waits);
            }
        }
    }

    /// Says what has not been said: each file's copy once, and how many others wait.
    fn tell(&mut self, editor: &mut Editor) {
        let now = SystemTime::now();
        // The newest of each file's new copies, as rows come newest first, and how many.
        let mut files: Vec<(BufferId, &Row, usize)> = Vec::new();
        for row in &self.rows {
            let Some(buffer) = row.target.filter(|_| row.state != RowState::Recovered) else {
                continue;
            };
            if self.told_for_a_file.insert(row.text.clone()) {
                self.told.insert(row.text.clone());
                match files.iter_mut().find(|file| file.0 == buffer) {
                    Some(file) => file.2 += 1,
                    None => files.push((buffer, row, 1)),
                }
            }
        }
        let mut saying: Vec<String> = files
            .into_iter()
            .map(|(_, row, count)| {
                let (name, from) = (&row.name, age(row.written, now));
                if count == 1 {
                    format!("{name} has an emergency copy, from {from}: C-h r")
                } else {
                    format!("{name} has {count} emergency copies, the newest from {from}: C-h r")
                }
            })
            .collect();
        let others = self
            .rows
            .iter()
            .filter(|row| self.told.insert(row.text.clone()))
            .count();
        if others > 0 {
            let what = plural(others, "copy", "copies");
            let verb = plural(others, "waits", "wait");
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
                .partition(|recovered| recovered.buffer == buffer);
            self.recovered = kept;
            work.extend(done.into_iter().map(|recovered| {
                CopyWork::Delete(Deleting {
                    text: recovered.row.text,
                    meta: recovered.row.meta,
                    name: recovered.row.name,
                })
            }));
            if let Some(shown) = editor.buffer_mut(buffer) {
                shown.forget_recovery();
            }
        }
        if work.is_empty() && self.rows.iter().any(|row| row.target == Some(buffer)) {
            // The file's stamp has changed, so what its rows say may have.
            work = self.ask_for_scan(editor);
        }
        work
    }
}

#[cfg(test)]
mod tests;
