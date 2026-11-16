//! A language server's life (ADR 0138, step 3): [`Server::start`] gives it a session thread,
//! which checks the gate, spawns the program in a process group of its own, and runs
//! `initialize` to `exit`, starting it again after a crash and stopping for the session after
//! three. The UI thread only asks, through [`Server`]'s methods, and takes what the session
//! thread reported ([`Server::take_report`]); it never reads, writes, parses or waits on a
//! server.
//!
//! Must not depend on: the tip crate, the configuration or the editor; the caller says what
//! to start ([`Start`]), whether it may ([`Gate`]), and how to wake the UI.
//!
//! Step 6 reads its diagnostics reports: the session thread drops those for a file outside the
//! project or whose name may hold secrets, keeps each document's last within the bounds, and
//! hands the UI a set for each open document, placed against the text the report names and
//! mapped forward, and each other file's report as it is kept, shared ([`Diagnosed`], 6b).
//!
//! Step 8 asks it about documents ([`Server::ask`]) and hands back its answers ([`Answer`]).

mod asks;
mod diagnosed;
mod documents;
mod outbox;
mod pipes;
mod policy;
mod session;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use oso_core::diagnostic::Diagnostics;
use oso_core::rope::Rope;
use oso_core::syntax::Splice;
use oso_platform::process::{Child, Executable, Signal};
use serde_json::Value;

pub use asks::{
    Answer, Answered, Ask, AskContext, AskKind, Completion, Completions, Location, Resolved, Target,
};

use crate::framing::{Frame, FrameError};
use crate::position::Encoding;
use crate::protocol::{Published, Severity};
use outbox::Outbox;
use session::Session;

/// What reaches a session thread: the UI's asks, and its run's pipe threads' news, which
/// name the run so that an earlier run's are told apart.
enum Event {
    Restart(Box<(Start, Gate)>),
    ShutDown,
    Quit,
    Open(Box<Document>),
    Change(Box<Change>),
    Close(u64),
    Save(u64),
    Ask(Ask),
    Frame(u64, Frame),
    ReadEnd(u64, Option<FrameError>),
    WriteFailed(u64, io::ErrorKind),
}

/// What to start, and what to tell it.
#[derive(Debug, Clone)]
pub struct Start {
    /// The entry's name, which names the server in `*LSP*` and the echo area.
    pub name: String,
    /// A number of its own among the servers, for its threads' names (`oso-lsp-1`).
    pub number: usize,
    /// The project's root: where it runs, its only workspace folder, and what is left out of
    /// its `PATH`.
    pub root: PathBuf,
    /// The names of variables it takes from oso's environment, beyond the process module's
    /// allowlist.
    pub inherit: Vec<String>,
    /// The section of the settings its `workspace/configuration` asks for (`rust-analyzer`).
    pub section: Option<String>,
    /// Its settings, sent as `initializationOptions` and in answer to
    /// `workspace/configuration`.
    pub options: Option<Value>,
}

/// Whether the server may start now, and the program to start, run on its session thread
/// before each start: `Err` says why not, for a person. The tip crate checks the project's
/// trust and root here, and finds the program, which can differ from one start to the next.
pub type Gate = Box<dyn Fn() -> Result<Found, String> + Send>;

/// The program a [`Gate`] found, with its arguments, and lines for `*LSP*` on what it passed
/// over, such as a program inside the project that did not pass its checks.
#[derive(Debug)]
pub struct Found {
    pub executable: Executable,
    pub args: Vec<OsString>,
    pub notes: Vec<String>,
}

/// Wakes the UI thread, which then calls [`Server::take_report`].
pub type Wake = Box<dyn Fn() + Send>;

/// A document to open with the server (ADR 0138, Document sync).
#[derive(Debug, Clone)]
pub struct Document {
    /// The caller's id for it, which the other asks and the report name.
    pub id: u64,
    /// Its `file:` URI.
    pub uri: String,
    /// Its language, as the protocol names it (`rust`).
    pub language_id: String,
    pub text: Rope,
    /// The caller's revision of `text`, which a set of diagnostics mapped to it names.
    pub revision: u64,
}

/// A batch of edits to an open document: the text as last given, the text now, and the
/// splices from one to the other, if the caller has them; without them the whole text goes.
#[derive(Debug, Clone)]
pub struct Change {
    pub id: u64,
    pub before: Rope,
    pub after: Rope,
    pub splices: Option<Vec<Splice>>,
    /// The caller's revision of `after`, which a set of diagnostics mapped to it names.
    pub revision: u64,
}

/// A document's diagnostics, as a report gave them (step 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnosed {
    /// Those of open document `id`, mapped to the text of the caller's revision `revision`:
    /// the caller maps them through its edits since, and swaps them in.
    Open {
        id: u64,
        revision: u64,
        diagnostics: Diagnostics,
    },
    /// Those of the file at `path`, which no document open with the server holds, as the server
    /// reported them, its positions counted in `encoding` (step 6b). The report is the one the
    /// session keeps, shared; one with no diagnostics forgets the file's, as does one the
    /// session's bounds let go.
    File {
        path: PathBuf,
        report: Arc<Published>,
        encoding: Encoding,
    },
    /// The run ended: every file's diagnostics from it go, as the next run reports again.
    Forgotten,
}

/// Why a document is kept from the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closed {
    /// It holds a carriage return that ends no line, which oso and the server would count
    /// differently.
    LoneCarriageReturn,
    /// It is larger than `limits::DOCUMENT_BYTES_MAX`.
    TooLarge,
}

/// Where a server is in its life, for the modeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Being started, until `initialize` is answered.
    Starting,
    /// Running, with no work reported.
    Ready,
    /// Running, reporting work, done to this percentage if it says.
    Working(Option<u8>),
    /// Waiting to start again after a crash.
    Waiting,
    /// Stopped for the session after three crashes. `lsp-restart` tries again
    /// ([`Server::restart`]).
    Stopped,
    /// Never started: the gate said no, as for a program not installed, or the spawn failed.
    /// The report said why once; [`Server::restart`] tries again (`lsp-start` and
    /// `lsp-restart`).
    NotStarted,
    /// Shut down: the session thread has ended.
    Ended,
}

/// A message for the person, for the echo area and `*Messages*`, without the server's name,
/// which the caller puts first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub severity: Severity,
    pub text: String,
}

/// What the session thread reported since the last [`Server::take_report`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub state: State,
    /// Lines for `*LSP*`, oldest first: method names, ids, sizes, timings and the standard
    /// error, never a document's text.
    pub log: Vec<String>,
    /// Messages for the person.
    pub shown: Vec<Shown>,
    /// Documents kept from the server since the last report, and why, or opened with it again
    /// (`None`), in order.
    pub documents: Vec<(u64, Option<Closed>)>,
    /// Diagnostics reported since the last report, the latest of each document.
    pub diagnostics: Vec<Diagnosed>,
    /// Answers to [`Server::ask`], the latest of each document and kind (step 8).
    pub answers: Vec<Answer>,
    /// What the server said of its completions in its run's `initialize`, until the UI took
    /// it: nothing of it for a server that does not complete (step 9b).
    pub completing: Option<crate::protocol::Completing>,
}

/// A server's session thread, and what the UI shares with it.
#[derive(Debug)]
pub struct Server {
    events: Sender<Event>,
    outbox: Arc<Outbox>,
    /// The running process, shared so that a quit can end it at once.
    child: Arc<Mutex<Option<Child>>>,
}

impl Server {
    /// Starts the session thread, which starts the server. Never waits on the server.
    ///
    /// # Errors
    ///
    /// When the thread cannot be started.
    pub fn start(start: Start, gate: Gate, wake: Wake) -> io::Result<Server> {
        let (events, receiver) = mpsc::channel();
        let outbox = Arc::new(Outbox::default());
        let child = Arc::new(Mutex::new(None));
        let name = format!("oso-lsp-{}", start.number);
        let session = Session::new(
            start,
            gate,
            outbox::Reporter::new(Arc::clone(&outbox), wake),
            receiver,
            events.clone(),
            Arc::clone(&child),
        );
        thread::Builder::new()
            .name(name)
            .spawn(move || session.run())?;
        Ok(Server {
            events,
            outbox,
            child,
        })
    }

    /// What was reported since the last call, `None` if nothing.
    pub fn take_report(&self) -> Option<Report> {
        self.outbox.take()
    }

    /// Opens `document` with the server, now or once it is ready, and with each run after.
    /// One that holds a lone carriage return, or is too large, is kept from it, and the report
    /// says so.
    pub fn open(&self, document: Document) {
        let _ = self.events.send(Event::Open(Box::new(document)));
    }

    /// Sends a batch of edits to an open document, worked out on the session thread.
    pub fn change(&self, change: Change) {
        let _ = self.events.send(Event::Change(Box::new(change)));
    }

    /// Closes document `id`.
    pub fn close(&self, id: u64) {
        let _ = self.events.send(Event::Close(id));
    }

    /// Tells the server the person saved document `id`, or opens it again if it was kept from
    /// the server for a reason that no longer holds.
    pub fn save(&self, id: u64) {
        let _ = self.events.send(Event::Save(id));
    }

    /// Asks the server about an open document: answered in a later report, at once if the
    /// server does not run, or the document's text there is not the one `ask` names.
    pub fn ask(&self, ask: Ask) {
        let _ = self.events.send(Event::Ask(ask));
    }

    /// Stops the server as [`Server::shut_down`] does, if it runs, forgets its crashes, and
    /// starts it again with `start` and `gate`: `lsp-restart`.
    pub fn restart(&self, start: Start, gate: Gate) {
        let _ = self.events.send(Event::Restart(Box::new((start, gate))));
    }

    /// Stops the server: `shutdown`, `exit`, then `SIGTERM` and `SIGKILL` to its group, each
    /// after a wait, on the session thread, which then ends: `lsp-shutdown`, or an untrust.
    pub fn shut_down(&self) {
        let _ = self.events.send(Event::ShutDown);
    }

    /// Ends the server as oso quits: `SIGTERM` to its group now, and its standard input
    /// closed, with no wait.
    pub fn quit(&self) {
        if let Some(child) = self
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            let _ = child.signal_group(Signal::Terminate);
        }
        let _ = self.events.send(Event::Quit);
    }
}

impl Drop for Server {
    /// A server no one holds is stopped as [`Server::shut_down`] stops it, on its thread.
    fn drop(&mut self) {
        let _ = self.events.send(Event::ShutDown);
    }
}
