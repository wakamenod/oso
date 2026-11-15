//! The directory editor's file operations as the app runs them (ADR 0134, step 1b-2): what
//! `C`, `R`, `D` and `+` asked, the questions before them, and what the `oso-files` thread
//! reports.
//!
//! Must not depend on: key presses or how a frame is drawn. No file is touched here: the thread
//! does it all, and the questions follow ADR 0072, each `y` running a command that checks again.
//!
//! - An operation acts on the marked rows, or with none on the rows the command named.
//! - A file whose buffer has unsaved edits is neither deleted nor moved away: the operation
//!   names the buffer and does nothing (the author's choice).
//! - `D` asks first, naming how many and the first three. A copy or a move finds what is at its
//!   targets first, and asks before writing over them, naming how many and the first three; `y`
//!   writes over exactly those, and a different set found then asks again.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Instant;

use oso_core::buffer::BufferId;

use super::App;
use crate::app::UserEvent;
use crate::dired::ops::worker::{Report, Worker};
use crate::dired::ops::{Existing, Kind, Outcome, Plan};

/// The file operations the app keeps.
#[derive(Default)]
pub(in crate::app) struct Ops {
    worker: Option<Worker>,
    next_id: u64,
    /// An operation waiting for the person's yes.
    asked: Option<Asked>,
    /// The operation running.
    running: Option<Running>,
}

/// What a question asks about.
struct Asked {
    buffer: BufferId,
    plan: Plan,
    /// The listing's generation then, which must hold when the person says yes.
    generation: u64,
    /// The question, while it is open; once it closes on anything but `y`, nothing is asked.
    prompt: oso_core::minibuffer::PromptId,
}

struct Running {
    id: u64,
    /// What it was asked to do, which a question about writing over targets asks about again.
    plan: Plan,
    buffer: BufferId,
    started: Instant,
}

/// How many names a question or a message lists before it says how many more.
const NAMED: usize = 3;

/// A name as a question or a message says it: control, bidirectional and invisible characters
/// as escapes, so that no name passes for another.
fn said(name: &std::ffi::OsStr) -> String {
    oso_core::editor::row_text(&name.to_string_lossy())
}

/// `count` entries, the first [`NAMED`] of `names` named: `3 files (a.txt, b.txt, c.rs)`.
fn counted(names: &[String]) -> String {
    let mut shown: Vec<&str> = names.iter().take(NAMED).map(String::as_str).collect();
    if names.len() > NAMED {
        shown.push("…");
    }
    let noun = if names.len() == 1 { "file" } else { "files" };
    format!("{} {noun} ({})", names.len(), shown.join(", "))
}

impl App {
    /// Starts the operation a command asked for, or the question before it.
    pub(super) fn ask_for_operation(
        &mut self,
        buffer: BufferId,
        kind: Kind,
        lines: &[usize],
        target: PathBuf,
    ) {
        if self.dired.ops.running.is_some() {
            return self.editor.message("A file operation runs: C-g stops it");
        }
        let Some(listing) = self.dired.by_buffer.get(&buffer) else {
            return;
        };
        if listing.rows_of != listing.directory {
            return self.editor.message("Still listing the directory");
        }
        // The marked rows, or with none those the command named, and which are directories.
        let rows: Vec<&crate::dired::Row> = if listing.marked.is_empty() {
            lines
                .iter()
                .filter_map(|line| listing.row_at(*line))
                .filter(|row| row.name != "." && row.name != "..")
                .collect()
        } else {
            (listing.rows.iter())
                .filter(|row| listing.marked.contains(&row.name))
                .collect()
        };
        let mut seen = std::collections::HashSet::new();
        let rows: Vec<&crate::dired::Row> = rows
            .into_iter()
            .filter(|row| seen.insert(row.name.clone()))
            .collect();
        let shown: Vec<String> = rows
            .iter()
            .map(|row| {
                let directory = row.face == Some(oso_core::syntax::Highlight::DiredDirectory);
                let slash = if directory { "/" } else { "" };
                format!("{}{slash}", said(&row.name))
            })
            .collect();
        let directories = shown.iter().any(|name| name.ends_with('/'));
        let names: Vec<OsString> = rows.iter().map(|row| row.name.clone()).collect();
        let listed_directories: Vec<bool> = rows
            .iter()
            .map(|row| row.face == Some(oso_core::syntax::Highlight::DiredDirectory))
            .collect();
        if names.is_empty() {
            return self.editor.message("No file on this line");
        }
        let plan = Plan {
            kind,
            directory: listing.directory.clone(),
            identity: listing.identity,
            names,
            directories: listed_directories,
            target,
            overwrite: None,
        };
        let generation = listing.generation;
        if matches!(kind, Kind::Delete | Kind::Rename)
            && let Some(name) = self.unsaved_under(&plan)
        {
            return self
                .editor
                .message(format!("{name} has unsaved edits: save or kill it first"));
        }
        if kind == Kind::Delete {
            let holds = if directories {
                " and all the directories hold"
            } else {
                ""
            };
            let question = format!("Delete {}{holds}?", counted(&shown));
            return self.ask_about(
                buffer,
                plan,
                generation,
                question,
                "dired-do-delete-confirmed",
                ["delete", "keep"],
            );
        }
        self.start_operation(buffer, plan);
    }

    /// Makes the directory `path` with its parents, `+`.
    pub(super) fn make_listed_directory(&mut self, buffer: BufferId, path: PathBuf) {
        if self.dired.ops.running.is_some() {
            return self.editor.message("A file operation runs: C-g stops it");
        }
        let directory = self
            .dired
            .by_buffer
            .get(&buffer)
            .map_or_else(PathBuf::new, |listing| listing.directory.clone());
        let plan = Plan {
            kind: Kind::Mkdir,
            directory,
            identity: None,
            names: Vec::new(),
            directories: Vec::new(),
            target: super::lexically_clean(&path, self.editor.start_directory()),
            overwrite: None,
        };
        self.start_operation(buffer, plan);
    }

    /// The name of a buffer with unsaved edits whose file `plan` would delete or move away.
    /// It compares by the words of each path, and by the real places the listing and each
    /// buffer's file were found at, so that a link or another spelling on the way does not hide
    /// one.
    fn unsaved_under(&self, plan: &Plan) -> Option<String> {
        let real = (self.dired.by_buffer.values())
            .find(|listing| listing.directory == plan.directory)
            .and_then(|listing| listing.real.clone());
        let mut sources: Vec<PathBuf> = (plan.names.iter())
            .map(|name| plan.directory.join(name))
            .collect();
        if let Some(real) = real {
            sources.extend(plan.names.iter().map(|name| real.join(name)));
        }
        let under = |path: &std::path::Path| sources.iter().any(|source| path.starts_with(source));
        self.editor
            .buffers()
            .filter(|(_, buffer)| buffer.is_modified())
            .find(|(_, buffer)| {
                buffer.path().is_some_and(under)
                    || buffer
                        .baseline()
                        .is_some_and(|baseline| under(&baseline.location))
            })
            .map(|(_, buffer)| buffer.name().to_owned())
    }

    fn ask_about(
        &mut self,
        buffer: BufferId,
        plan: Plan,
        generation: u64,
        question: String,
        yes: &'static str,
        verbs: [&'static str; 2],
    ) {
        match self.editor.ask_with(question, yes, verbs) {
            Ok(prompt) => {
                self.dired.ops.asked = Some(Asked {
                    buffer,
                    plan,
                    generation,
                    prompt,
                });
                self.request_redraw();
            }
            Err(error) => self.editor.message(error.to_string()),
        }
    }

    /// The person said yes to the question asked: the listing must be the one asked about, the
    /// buffers checked again, and the operation goes to the thread, which checks the rest.
    pub(super) fn operation_confirmed(&mut self, overwrite: bool) {
        let Some(asked) = self.dired.ops.asked.take() else {
            return;
        };
        if self.dired.ops.running.is_some() {
            return self.editor.message("A file operation runs: C-g stops it");
        }
        let same = self
            .dired
            .by_buffer
            .get(&asked.buffer)
            .is_some_and(|listing| listing.generation == asked.generation);
        let wanted = if overwrite {
            asked.plan.overwrite.is_some()
        } else {
            asked.plan.kind == Kind::Delete
        };
        if !same || !wanted {
            return self
                .editor
                .message("The listing changed since it asked: nothing was done");
        }
        if matches!(asked.plan.kind, Kind::Delete | Kind::Rename)
            && let Some(name) = self.unsaved_under(&asked.plan)
        {
            return self
                .editor
                .message(format!("{name} has unsaved edits: save or kill it first"));
        }
        self.start_operation(asked.buffer, asked.plan);
    }

    fn start_operation(&mut self, buffer: BufferId, plan: Plan) {
        let ops = &mut self.dired.ops;
        let waker = self.waker.clone();
        let worker = ops.worker.get_or_insert_with(|| {
            let waker = std::sync::Mutex::new(waker);
            Worker::new(move || {
                waker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .send(UserEvent::FileOps)
            })
        });
        ops.next_id += 1;
        let id = ops.next_id;
        if let Err(error) = worker.run(id, plan.clone()) {
            return self.editor.message(error);
        }
        ops.running = Some(Running {
            id,
            plan,
            buffer,
            started: Instant::now(),
        });
        self.editor.set_operation_running(true);
    }

    /// `C-g` asked the operation running to stop.
    pub(in crate::app) fn stop_operation_if_asked(&mut self) {
        if self.editor.take_operation_stop()
            && let Some(worker) = &self.dired.ops.worker
        {
            worker.stop();
        }
    }

    /// Forgets a question closed without its `y`: `n`, `C-g` or another prompt over it, so that
    /// nothing runs what was refused. `confirming` is whether a `y` came with this batch.
    pub(super) fn forget_refused_question(&mut self, confirming: bool) {
        let open = (self.dired.ops.asked.as_ref()).is_some_and(|asked| {
            self.editor
                .prompts()
                .iter()
                .any(|prompt| prompt.id() == asked.prompt)
        });
        if !open && !confirming {
            self.dired.ops.asked = None;
        }
    }

    /// What the `oso-files` thread told.
    pub(in crate::app) fn file_ops_reported(&mut self) {
        let Some(worker) = &self.dired.ops.worker else {
            return;
        };
        for report in worker.take() {
            match report {
                Report::Progress { id, done } => {
                    if let Some(running) = self.dired.ops.running.as_ref().filter(|r| r.id == id) {
                        let verb = verb(running.plan.kind);
                        self.editor
                            .echo(format!("{verb}… {done} so far (C-g stops)"));
                    }
                }
                Report::Finished { id, outcome } => {
                    let Some(running) = self.dired.ops.running.take_if(|r| r.id == id) else {
                        continue;
                    };
                    self.editor.set_operation_running(false);
                    self.operation_finished(&running, outcome);
                }
            }
        }
        self.request_redraw();
    }

    fn operation_finished(&mut self, running: &Running, outcome: Outcome) {
        if !outcome.existing.is_empty() {
            return self.ask_to_overwrite(running, outcome.existing);
        }
        let kind = running.plan.kind;
        for (from, to) in &outcome.moved {
            self.editor.file_moved(from, to);
        }
        // Every listing of a directory it changed is listed again.
        let mut relisted = Vec::new();
        for directory in &outcome.touched {
            if let Some(id) = self.listing_of_directory(directory) {
                relisted.push(id);
            }
        }
        relisted.push(running.buffer);
        relisted.sort_unstable();
        relisted.dedup();
        for id in relisted {
            if let Some(listing) = self.dired.by_buffer.get_mut(&id)
                && kind == Kind::Delete
                && id == running.buffer
            {
                listing.marked = std::sync::Arc::default();
            }
            self.list_again(id);
        }
        let took = running.started.elapsed().as_secs_f64();
        let mut message = format!("{}: {} done in {took:.1} s", verb(kind), outcome.done);
        if !outcome.skipped.is_empty() {
            let names: Vec<String> = outcome.skipped.iter().map(|name| said(name)).collect();
            message += &format!("; left {}, which are no files", counted(&names));
        }
        if let Some(error) = outcome.error {
            message += &format!("; stopped: {error}");
        }
        // Names in a listing that may hold secrets stay out of the log and `*Messages*`.
        let withheld = (self.editor.buffer(running.buffer)).is_none_or(|shown| shown.is_secret());
        if withheld {
            self.editor.echo(message);
        } else {
            self.editor.message(message);
        }
    }

    /// Asks before a copy or a move writes over what is at its targets, naming them, and saying
    /// of a directory that the copy merges into it. Not over another prompt, where keys typed
    /// for that one would answer it.
    fn ask_to_overwrite(&mut self, running: &Running, existing: Vec<Existing>) {
        if self.editor.prompt().is_some() {
            return self
                .editor
                .message("Targets are there already: nothing was written; ask again");
        }
        let Some(listing) = self.dired.by_buffer.get(&running.buffer) else {
            return;
        };
        let generation = listing.generation;
        let mut plan = running.plan.clone();
        let names: Vec<String> = (existing.iter())
            .map(|there| {
                let name = said(there.path.file_name().unwrap_or(there.path.as_os_str()));
                if there.directory {
                    format!("{name}/")
                } else {
                    name
                }
            })
            .collect();
        let merge = if existing.iter().any(|there| there.directory) {
            "; directories merge, their files of the same names written over"
        } else {
            ""
        };
        let question = format!("{} there already{merge}: write over them?", counted(&names));
        plan.overwrite = Some(existing.into_iter().map(|there| there.path).collect());
        self.ask_about(
            running.buffer,
            plan,
            generation,
            question,
            "dired-do-overwrite-confirmed",
            ["write over", "keep"],
        );
    }
}

/// What the echo area calls an operation.
fn verb(kind: Kind) -> &'static str {
    match kind {
        Kind::Copy => "Copying",
        Kind::Rename => "Moving",
        Kind::Delete => "Deleting",
        Kind::Mkdir => "Making directories",
    }
}

#[cfg(test)]
mod tests {
    use super::{counted, said};

    #[test]
    fn a_name_said_shows_what_could_hide() {
        assert_eq!(said("it's a.txt".as_ref()), "it's a.txt");
        assert_eq!(said("a\nb".as_ref()), "a\\nb");
        assert_eq!(said("evil\u{202e}txt.exe".as_ref()), "evil\\u{202e}txt.exe");
        assert_eq!(said("x\u{2028}y".as_ref()), "x\\u{2028}y");
    }

    #[test]
    fn names_are_counted_and_the_first_three_named() {
        let names: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
        assert_eq!(counted(&names), "4 files (a, b, c, …)");
        assert_eq!(counted(&names[..1]), "1 file (a)");
    }
}
