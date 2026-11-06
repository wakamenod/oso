//! The emergency save: writing the text that would be lost when oso panics outside a
//! command, or when the OS ends it (ADR 0013, ADR 0014, ADR 0051, ADR 0062).
//!
//! Must not depend on: `app`, `dispatch` or winit; the caller says what happened, and
//! this holds no way back to the window. It sits above the saver in the layers, whose
//! temporary file and syncing it shares.
//!
//! Three parts, each in its own file:
//!
//! - [`Publisher`], on the UI thread, keeps [`Shared`] up to date with an O(1) snapshot of
//!   each buffer whose text is only here. Neither the hook nor the exit reads the editor,
//!   which a panic may have left in the middle of anything.
//! - `hook` is the panic hook. It holds an `Arc` of [`Shared`], as nothing here is global.
//! - `write` puts one copy on disk.
//!
//! Text goes nowhere but into the copies. It is in no message, log or panic output.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, TryLockError};
use std::thread::{self, ThreadId};
use std::time::{Duration, SystemTime};

use oso_core::buffer::{BufferId, ContentId, FileStamp};
use oso_core::decode::LineEnding;
use oso_core::rope::Rope;
use oso_platform::privacy::{PrivateDir, state_home};

use crate::saver::{Agreement, AgreementCell};

mod hook;
mod publish;
mod write;

pub use publish::Publisher;

/// How long writing the copies may show no progress before oso goes on without them. A
/// write that is getting somewhere is never abandoned, but a stuck network home directory
/// must not hang a process that is ending. A sync says nothing until it is done, however
/// far it has got, so it has an allowance of its own: 2 GiB at 10 MB/s takes under 4 minutes.
const LIMITS: Limits = Limits {
    stall: Duration::from_secs(30),
    sync: Duration::from_secs(300),
};

/// How long the writing thread may go without a word.
#[derive(Clone, Copy)]
struct Limits {
    /// While it writes.
    stall: Duration,
    /// While it syncs.
    sync: Duration,
}

/// The owner of what the panic hook and the exit share. The UI thread makes this.
pub struct Emergency {
    shared: Arc<Shared>,
}

impl Emergency {
    /// State for this thread, the UI thread, with its copies going to `directory`, if a
    /// place for them could be told. Nothing touches the disk until they are needed.
    pub fn new(directory: Option<PathBuf>) -> Self {
        Self {
            shared: Arc::new(Shared {
                ui_thread: thread::current().id(),
                in_command: AtomicBool::new(false),
                panicked_in_command: AtomicBool::new(false),
                agreement: AgreementCell::default(),
                directory,
                state: Mutex::default(),
            }),
        }
    }

    /// Makes the panic hook this process's. Once, and only in the running app: tests share
    /// one process, and the hook is for all of it.
    pub fn install_hook(&self) {
        hook::install(Arc::clone(&self.shared));
    }

    /// The UI thread's way of keeping the state up to date.
    pub fn publisher(&self) -> Publisher {
        Publisher::new(Arc::clone(&self.shared))
    }

    /// Where what this instance has agreed to lose is kept, for the app to keep up to date.
    pub fn agreement_cell(&self) -> AgreementCell {
        self.shared.agreement.clone()
    }

    /// Finds out on a thread of its own, soon after startup, whether copies could be written,
    /// and tells `report` if not, as finding out at the crash loses the text. Creates nothing.
    pub fn check_directory(&self, report: impl FnOnce(String) + Send + 'static) {
        let directory = self.shared.directory.clone();
        // With no thread there is no finding out now, and a crash still says why.
        let _ = thread::Builder::new()
            .name("oso-emergency-check".into())
            .spawn(move || {
                if let Some(problem) = directory_problem(directory.as_deref()) {
                    report(problem);
                }
            });
    }

    /// The event loop is ending. Writes what nobody agreed to lose, and says so if there
    /// is anything to say.
    pub fn on_exit(&self) {
        let report = self.shared.write_copies(Why::Exit);
        if !report.is_empty() {
            hook::say_to(io::stderr(), move || report.to_string(), hook::SAY_LIMIT);
        }
    }
}

/// Why no copy could be written now, as far as can be told without creating anything.
fn directory_problem(directory: Option<&Path>) -> Option<String> {
    let Some(directory) = directory else {
        return Some(
            "No emergency copy can be written if oso crashes: no place for them could be told"
                .to_owned(),
        );
    };
    let problem = PrivateDir::check(directory).err()?;
    Some(format!(
        "No emergency copy can be written if oso crashes: {problem}"
    ))
}

/// What the hook, the exit and the UI thread share.
struct Shared {
    ui_thread: ThreadId,
    /// Under `Registry::execute`, whose catch means the editor runs on.
    in_command: AtomicBool,
    /// This command has already panicked. A second panic comes during its unwinding, and the
    /// process aborts before any catch can run.
    panicked_in_command: AtomicBool,
    /// What this instance has agreed to lose.
    agreement: AgreementCell,
    directory: Option<PathBuf>,
    /// Only the UI thread writes this, moving things while it holds the lock, and a hook
    /// only ever tries to take it. So it never blocks, and nothing under it can panic.
    state: Mutex<State>,
}

/// What was last published.
#[derive(Default)]
struct State {
    /// Each buffer whose text is only here, sorted by id.
    entries: Vec<Entry>,
    /// The states already on disk, which a second panic or an exit after a panic does not
    /// write again.
    written: Vec<(BufferId, ContentId)>,
}

/// One buffer's text at risk, or the fact of it if the text may not be copied.
#[derive(Clone)]
struct Entry {
    buffer: BufferId,
    identity: Arc<Identity>,
    /// `None` if its verdict forbids a copy: such text is never held here.
    text: Option<Rope>,
    content: ContentId,
    /// A save is running, so that the disk may hold either state.
    saving: bool,
    /// The file as this buffer last read or wrote it, if it was there. It says whether the
    /// file has changed since, which recovery needs to know.
    file: Option<FileStamp>,
}

/// What stays the same while a buffer's text changes, so that a key clones no path.
struct Identity {
    name: String,
    /// The path the buffer visits, if any.
    path: Option<PathBuf>,
    /// The real place the path led, if that could be told.
    location: Option<PathBuf>,
    bom: bool,
    line_ending: LineEnding,
}

/// Why copies are being written, as their metadata says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Why {
    Panic,
    Exit,
}

impl Why {
    fn as_str(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::Exit => "exit",
        }
    }
}

/// How writing the copies went. Holds no text, so it is safe to print.
#[derive(Debug, Default, PartialEq, Eq)]
struct Report {
    directory: Option<PathBuf>,
    wrote: usize,
    /// Copies wanted that are not on disk.
    failed: usize,
    /// Buffers at risk whose verdict forbids a copy.
    withheld: usize,
    /// Why some or all were not written.
    problem: Option<String>,
}

impl Report {
    fn is_empty(&self) -> bool {
        self.wrote == 0 && self.failed == 0 && self.withheld == 0 && self.problem.is_none()
    }
}

/// The plural of `copy` or `buffer` after `count`.
fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut lines: Vec<String> = Vec::new();
        if self.wrote > 0 {
            let place = self.directory.as_deref().unwrap_or(Path::new("?"));
            lines.push(format!(
                "oso wrote {} emergency {} in {}",
                self.wrote,
                plural(self.wrote, "copy", "copies"),
                place.display()
            ));
        }
        if self.failed > 0 {
            lines.push(format!(
                "oso could not write {} emergency {}, and that text is lost",
                self.failed,
                plural(self.failed, "copy", "copies"),
            ));
        }
        if let Some(problem) = &self.problem {
            lines.push(format!("oso: {problem}"));
        }
        if self.withheld > 0 {
            lines.push(format!(
                "oso wrote no copy of {} unsaved {} that may hold secrets",
                self.withheld,
                plural(self.withheld, "buffer", "buffers"),
            ));
        }
        f.write_str(&lines.join("\n"))
    }
}

/// Whether `entry` gets a copy, given what was agreed to. A person who confirmed an exit,
/// and the socket's `exit`, have agreed to lose modified buffers. A save it is waiting for is
/// another matter: that text was meant to reach the disk, and the end may stop the write.
/// Nobody can tell how far it got, so its buffer gets a copy, also if an undo made it look
/// unmodified, since the file may be about to differ from it. Once nothing is left to wait
/// for, that includes a save that outlived an exit, which the person was told of.
fn wanted(agreement: Agreement, entry: &Entry) -> bool {
    match agreement {
        Agreement::Nothing => true,
        Agreement::ModifiedButNotSaves => entry.saving,
        Agreement::Everything => false,
    }
}

impl Shared {
    /// The entries to write and how many are withheld, or `None` if the state cannot be
    /// read. It is held only if a panic came while publishing: there is nothing to wait
    /// for then, on this very thread. A poisoned lock is as good as any, since only moves
    /// happen under it.
    fn select(&self) -> Option<(Vec<Entry>, usize)> {
        let agreement = self.agreement.get();
        let state = match self.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        let new = |entry: &&Entry| {
            wanted(agreement, entry) && !state.written.contains(&(entry.buffer, entry.content))
        };
        let mut to_write: Vec<Entry> = state
            .entries
            .iter()
            .filter(new)
            .filter(|entry| entry.text.is_some())
            .cloned()
            .collect();
        // Smallest first, as giving up on a stall ends the whole run: one huge buffer on a slow
        // disk must not cost the small ones behind it.
        to_write.sort_by_key(|entry| entry.text.as_ref().map_or(0, Rope::len));
        let withheld = state
            .entries
            .iter()
            .filter(new)
            .filter(|entry| entry.text.is_none())
            .count();
        Some((to_write, withheld))
    }

    /// Writes a copy of each buffer at risk, and reports how that went. Safe to run twice:
    /// a state already written is not written again.
    fn write_copies(&self, why: Why) -> Report {
        let now = SystemTime::now();
        self.write_copies_with(LIMITS, move |directory, entry, ticks| {
            write::write_copy(directory, entry, why, now, ticks)
        })
    }

    /// [`Shared::write_copies`], through `write`, giving up past `limits` with no progress.
    fn write_copies_with<W>(&self, limits: Limits, write: W) -> Report
    where
        W: Fn(&Path, &Entry, &Ticks<'_>) -> io::Result<()> + Send + Sync + 'static,
    {
        let Some((entries, withheld)) = self.select() else {
            return Report {
                problem: Some(
                    "a panic came while the text was being recorded, so no emergency copy could be written"
                        .into(),
                ),
                ..Report::default()
            };
        };
        let mut report = Report {
            withheld,
            directory: self.directory.clone(),
            ..Report::default()
        };
        if entries.is_empty() {
            return report;
        }
        let Some(directory) = self.directory.clone() else {
            report.failed = entries.len();
            report.problem = Some("no place for emergency copies could be told".into());
            return report;
        };
        let count = entries.len();
        let work = Arc::new(Work {
            directory,
            entries,
            write,
        });
        let done = run_bounded(&work, limits);
        report.wrote = done.written.len();
        report.failed = count - report.wrote;
        report.problem = done.problem;
        if let Ok(mut state) = self.state.try_lock() {
            state.written.extend(done.written);
        }
        report
    }
}

/// The copies to write, on a thread of their own.
struct Work<W> {
    directory: PathBuf,
    entries: Vec<Entry>,
    write: W,
}

/// What the writing thread says as it goes.
enum Tick {
    /// Still getting somewhere.
    Progress,
    /// About to sync, which says nothing until it is done.
    Syncing,
    Wrote(BufferId, ContentId),
    Failed(String),
    Finished,
}

/// The writer's way of saying that it still gets somewhere.
struct Ticks<'a>(&'a mpsc::Sender<Tick>);

impl Ticks<'_> {
    fn progress(&self) {
        let _ = self.0.send(Tick::Progress);
    }

    fn before_sync(&self) {
        let _ = self.0.send(Tick::Syncing);
    }
}

/// What came of [`Work`] by the time it finished, or was given up on.
struct Done {
    written: Vec<(BufferId, ContentId)>,
    problem: Option<String>,
}

impl<W> Work<W>
where
    W: Fn(&Path, &Entry, &Ticks<'_>) -> io::Result<()>,
{
    /// Writes each buffer on its own, past any failure, and past a panic: another thread's
    /// panic can be caught, where one inside a panic hook aborts at once, whatever
    /// catches it. So one bad buffer cannot cost the others.
    fn run(&self, ticks: &mpsc::Sender<Tick>) {
        let directory = match PrivateDir::open(&self.directory) {
            Ok(directory) => directory,
            Err(e) => {
                let _ = ticks.send(Tick::Failed(format!("wrote no emergency copy: {e}")));
                let _ = ticks.send(Tick::Finished);
                return;
            }
        };
        for entry in &self.entries {
            let written = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (self.write)(directory.path(), entry, &Ticks(ticks))
            }));
            let _ = ticks.send(match written {
                Ok(Ok(())) => Tick::Wrote(entry.buffer, entry.content),
                Ok(Err(e)) => Tick::Failed(e.to_string()),
                Err(_) => Tick::Failed("writing an emergency copy panicked".into()),
            });
        }
        let _ = ticks.send(Tick::Finished);
    }
}

/// Runs `work` on another thread, and waits for as long as it gets somewhere. If no thread
/// can start, it runs here, where only a panic could cost the rest.
fn run_bounded<W>(work: &Arc<Work<W>>, limits: Limits) -> Done
where
    W: Fn(&Path, &Entry, &Ticks<'_>) -> io::Result<()> + Send + Sync + 'static,
{
    let (ticks, heard) = mpsc::channel();
    let spawned = {
        let (work, ticks) = (Arc::clone(work), ticks.clone());
        thread::Builder::new()
            .name("oso-emergency".into())
            .spawn(move || work.run(&ticks))
    };
    if spawned.is_err() {
        work.run(&ticks);
    }
    // The thread holds the only other sender, so its end is heard even if it dies.
    drop(ticks);
    let mut done = Done {
        written: Vec::new(),
        problem: None,
    };
    let mut wait = limits.stall;
    loop {
        let tick = heard.recv_timeout(wait);
        wait = match tick {
            Ok(Tick::Syncing) => limits.sync,
            _ => limits.stall,
        };
        match tick {
            Ok(Tick::Progress | Tick::Syncing) => {}
            Ok(Tick::Wrote(buffer, content)) => done.written.push((buffer, content)),
            Ok(Tick::Failed(problem)) => {
                done.problem.get_or_insert(problem);
            }
            Ok(Tick::Finished) | Err(RecvTimeoutError::Disconnected) => return done,
            Err(RecvTimeoutError::Timeout) => {
                done.problem
                    .get_or_insert("writing emergency copies stopped getting anywhere".into());
                return done;
            }
        }
    }
}

/// Where the copies go: `emergency` under oso's directory of per-user state. Asks the
/// environment and not the disk.
pub fn default_directory() -> Option<PathBuf> {
    Some(state_home()?.join("oso").join("emergency"))
}

#[cfg(test)]
pub(crate) mod tests;
