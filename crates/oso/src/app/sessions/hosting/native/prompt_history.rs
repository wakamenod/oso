//! The history of what the person sent from native sessions' composers (ADR 0165, "The composer's
//! history, as amended"), in the app: held for all sessions here, and kept by `oso-prompts`, a thread of
//! its own, in oso's state directory. `sessions::prompt_history` has its rules and its file.
//!
//! Must not depend on: key presses or how a frame is drawn. The file is read and written on
//! the thread only; the UI thread waits for it but at an exit or a restart's start
//! (`flush_records`).
//!
//! - Nothing is read or written before the configuration's first look applied, so that
//!   `[sessions] prompt_history = false` holds from the start. Off, what is sent is in memory
//!   only, and what waits to be written is dropped; it stays off until oso's next start, so
//!   that nothing sent while it is off is written later.
//! - A write waits [`QUIET`] after the last send, and takes in what other instances wrote.
//! - Nothing of a message reaches a log or a message of oso's.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::app::{App, UserEvent};
use crate::sessions::prompt_history::{Entry, History, Step, file};

/// How long a write waits after the last change, so that a run of sends writes once.
const QUIET: Duration = Duration::from_secs(1);

/// What goes to the thread.
#[derive(Debug)]
enum Job {
    Read,
    /// The history as this instance knows it; with `clears`, at once.
    Write {
        history: History,
        clears: bool,
    },
    /// What waits is not written.
    Drop,
    /// Answer once what was handed over before is written.
    Flush(SyncSender<()>),
}

/// What the thread did.
#[derive(Debug)]
pub enum PromptHistoryEvent {
    /// The file as read, or as a write left it.
    Known(History),
    NotRead(String),
    NotWritten(String),
    NotCleared(String),
}

/// Whether the history is kept in the file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Kept {
    /// The configuration's first look has not applied yet.
    #[default]
    Unknown,
    Yes,
    No,
}

impl Kept {
    /// What it is now that the configuration says `configured`, which is `None` before it
    /// applied.
    fn after(self, configured: Option<bool>) -> Self {
        match (self, configured) {
            (_, Some(false)) | (Self::No, _) => Self::No,
            (Self::Unknown, Some(true)) => Self::Yes,
            (now, _) => now,
        }
    }
}

/// The history, and what keeps it.
#[derive(Debug, Default)]
pub(super) struct Prompts {
    history: History,
    kept: Kept,
    /// Something was sent since the thread was last handed the history.
    unsaved: bool,
    /// That it is not written was said once.
    said: bool,
    jobs: Option<Sender<Job>>,
}

impl Prompts {
    /// The message `count` before the one sent at `from`, as [`History::step`] says.
    pub(super) fn step(&self, from: Option<u64>, count: isize) -> Step<'_> {
        self.history.step(from, count)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.history.is_empty()
    }
}

/// Now, in milliseconds since the epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |now| u64::try_from(now.as_millis()).unwrap_or(u64::MAX))
}

impl App {
    /// Notes that the person sent `text` from a native session's composer.
    pub(super) fn note_prompt(&mut self, text: &str) {
        self.hosting.natives.prompts.history.note(text, now());
        self.hosting.natives.prompts.unsaved = true;
        self.list_past_prompts();
    }

    /// `native-prompt-history-clear`: takes every message out, here and in the file, which other
    /// instances' next writes follow.
    pub(in crate::app) fn clear_prompt_history(&mut self) {
        let count = self.hosting.natives.prompts.history.clear(now());
        self.hosting.natives.prompts.unsaved = false;
        self.list_past_prompts();
        let job = Job::Write {
            history: self.hosting.natives.prompts.history.clone(),
            clears: true,
        };
        self.send_prompts(job);
        let s = if count == 1 { "" } else { "s" };
        self.editor
            .message(format!("Cleared {count} past prompt{s}"));
    }

    /// After an event: reads the file once the configuration lets it be kept, and hands what
    /// changed to the thread.
    pub(in crate::app) fn sync_prompt_history(&mut self) {
        let was = self.hosting.natives.prompts.kept;
        self.hosting.natives.prompts.kept = was.after(self.configuration.prompt_history());
        match (was, self.hosting.natives.prompts.kept) {
            (Kept::Unknown, Kept::Yes) => self.send_prompts(Job::Read),
            (Kept::Yes, Kept::No) => self.send_prompts(Job::Drop),
            _ => {}
        }
        if self.hosting.natives.prompts.kept == Kept::Yes
            && std::mem::take(&mut self.hosting.natives.prompts.unsaved)
        {
            let job = Job::Write {
                history: self.hosting.natives.prompts.history.clone(),
                clears: false,
            };
            self.send_prompts(job);
        }
    }

    /// Hands `job` to the thread, started at the first; with no directory, to none.
    fn send_prompts(&mut self, job: Job) {
        let Some(directory) = self.hosting.directory.clone() else {
            return;
        };
        if self.hosting.natives.prompts.jobs.is_none() {
            let (jobs, waiting) = mpsc::channel();
            let waker = self.waker.clone();
            let spawned = thread::Builder::new()
                .name("oso-prompts".into())
                .spawn(move || {
                    run(&directory, &waiting, |event| {
                        let _ = waker.send(UserEvent::PromptHistory(event));
                    });
                });
            match spawned {
                Ok(_) => self.hosting.natives.prompts.jobs = Some(jobs),
                Err(e) => log::warn!("the prompt history will not be kept: {e}"),
            }
        }
        if let Some(jobs) = &self.hosting.natives.prompts.jobs {
            let _ = jobs.send(job);
        }
    }

    /// The thread did something.
    pub(in crate::app) fn prompt_history_event(&mut self, event: PromptHistoryEvent) {
        match event {
            PromptHistoryEvent::Known(history) => {
                if self.hosting.natives.prompts.kept == Kept::Yes {
                    self.hosting.natives.prompts.history.merge(&history, now());
                    self.list_past_prompts();
                }
            }
            PromptHistoryEvent::NotRead(why) => self
                .editor
                .message(format!("The prompt history is not read: {why}")),
            PromptHistoryEvent::NotCleared(why) => self
                .editor
                .message(format!("The prompt history's file is not cleared: {why}")),
            PromptHistoryEvent::NotWritten(why) => {
                if !std::mem::replace(&mut self.hosting.natives.prompts.said, true) {
                    self.editor
                        .message(format!("The prompt history is not kept: {why}"));
                }
            }
        }
    }

    /// Asks the thread to say once all it was handed is written, what is unsaved among it, if
    /// there is something to wait for.
    pub(in crate::app) fn flush_prompt_history(&mut self) -> Option<Receiver<()>> {
        self.sync_prompt_history();
        let (done, answer) = mpsc::sync_channel(1);
        let jobs = self.hosting.natives.prompts.jobs.as_ref()?;
        jobs.send(Job::Flush(done)).ok().map(|()| answer)
    }

    /// Gives the editor the messages to list.
    fn list_past_prompts(&mut self) {
        let texts = (self.hosting.natives.prompts.history.entries().iter())
            .map(|Entry { text, .. }| Arc::clone(text))
            .collect();
        self.editor.set_past_prompts(texts);
    }
}

/// What waits to be written.
struct Waiting {
    history: History,
    clears: bool,
}

/// The thread: reads the file as asked, and writes the newest history once [`QUIET`] has passed
/// with no other, or at once for a clear or a flush, until the app is gone. A file another
/// instance holds is tried again.
fn run(directory: &Path, jobs: &Receiver<Job>, report: impl Fn(PromptHistoryEvent)) {
    let mut waiting: Option<Waiting> = None;
    loop {
        let job = match &waiting {
            Some(_) => match jobs.recv_timeout(QUIET) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    write_waiting(directory, &mut waiting, &report);
                    return;
                }
            },
            None => match jobs.recv() {
                Ok(job) => Some(job),
                Err(_) => return,
            },
        };
        match job {
            Some(Job::Read) => report(match file::read(directory) {
                Ok(history) => PromptHistoryEvent::Known(history),
                Err(e) => PromptHistoryEvent::NotRead(e.to_string()),
            }),
            Some(Job::Write { history, clears }) => {
                let cleared = waiting.as_ref().is_some_and(|waiting| waiting.clears);
                waiting = Some(Waiting {
                    history,
                    clears: clears || cleared,
                });
                if clears {
                    write_waiting(directory, &mut waiting, &report);
                }
            }
            // A clear that waits still goes, with nothing to write beside it.
            Some(Job::Drop) => waiting = waiting.filter(|waiting| waiting.clears),
            Some(Job::Flush(done)) => {
                write_waiting(directory, &mut waiting, &report);
                let _ = done.send(());
            }
            None => write_waiting(directory, &mut waiting, &report),
        }
    }
}

/// Writes what waits, if anything does; it waits on while another instance holds the file.
fn write_waiting(
    directory: &Path,
    waiting: &mut Option<Waiting>,
    report: &impl Fn(PromptHistoryEvent),
) {
    let Some(Waiting { history, clears }) = waiting.take() else {
        return;
    };
    match file::write(directory, &history, clears, now()) {
        Ok(known) => report(PromptHistoryEvent::Known(known)),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            *waiting = Some(Waiting { history, clears });
        }
        Err(e) => {
            log::warn!("could not write the prompt history: {e}");
            report(match clears {
                true => PromptHistoryEvent::NotCleared(e.to_string()),
                false => PromptHistoryEvent::NotWritten(e.to_string()),
            });
        }
    }
}

#[cfg(test)]
mod tests;
