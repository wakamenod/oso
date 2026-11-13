//! Hosted sessions in the app (ADR 0121): starting, resuming and forking them at the person's
//! command, the start thread's reports, the remembered set, and the bridge connections that
//! hold a session. The work that touches files and processes is on the start thread and the
//! sessions worker; this only decides and carries out.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use oso_core::buffer::BufferId;
use oso_platform::control::Peer;

use super::super::{App, UserEvent};
use crate::channel::Connection;
use crate::review::TrustedRoot;
use crate::sessions::hosted::{New, Run, Table};
use crate::sessions::identity::SessionId;
use crate::sessions::start::{self, Kind, Order, Ready, Report};
use crate::sessions::worker::{Job, SessionKey};

/// What the app keeps of its hosted sessions.
#[derive(Debug, Default)]
pub(in crate::app) struct Hosting {
    table: Table,
    /// oso's own state directory, where the record is kept.
    directory: Option<PathBuf>,
    /// The internal `*Session explain*` buffer, once there is one. Kept by its id, never found
    /// by its name, which anyone may give a buffer of their own.
    pub(in crate::app) explain: Option<BufferId>,
}

impl Hosting {
    /// The hosted sessions.
    pub(in crate::app) fn table(&self) -> &Table {
        &self.table
    }

    /// Sessions remembered in `directory`, oso's state directory.
    pub(in crate::app) fn new(directory: Option<PathBuf>) -> Self {
        Self {
            table: Table::default(),
            directory,
            explain: None,
        }
    }
}

/// Now, in ms since the epoch, for a session's last change.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// A session's name from its directory's.
fn base_name(directory: &Path) -> String {
    directory.file_name().map_or_else(
        || "session".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl App {
    /// The sessions an earlier oso remembered: each that is not archived comes back stopped,
    /// with nothing started.
    pub(in crate::app) fn sessions_recalled(
        &mut self,
        remembered: Vec<crate::sessions::record::Remembered>,
    ) {
        let back = self.hosting.table.recall(remembered);
        for key in back {
            self.bring_back_stopped(key);
        }
    }

    /// Hands the sessions to the worker to remember, keeping another instance's.
    fn remember_sessions(&self) {
        if let (Some(worker), Some(directory)) = (&self.sessions, &self.hosting.directory) {
            worker.send(Job::Remember {
                directory: directory.clone(),
                sessions: self.hosting.table.remembered(),
                known: self.hosting.table.known().clone(),
            });
        }
    }

    /// `claude`: a new session in `directory`, else the selected window's project root, else
    /// the buffer's directory, named `name`, else after its directory. It turns the agent
    /// channel on if it is off (Q20).
    pub(in crate::app) fn start_session(
        &mut self,
        name: Option<String>,
        directory: Option<PathBuf>,
    ) {
        let current = self.editor.current().1.project();
        let (directory, project) = match directory {
            Some(directory) if !directory.is_absolute() => {
                self.editor.message("Name the session's directory whole");
                return;
            }
            Some(directory) => {
                let project = current.filter(|id| {
                    self.editor
                        .project(*id)
                        .is_some_and(|project| project.root() == directory)
                });
                (directory, project)
            }
            None => match self.editor.project_files_root() {
                Ok((root, _)) => (root, current),
                Err(_) => (self.editor.start_directory().to_path_buf(), None),
            },
        };
        let base = name.unwrap_or_else(|| base_name(&directory));
        let (key, name) = self.hosting.table.reserve(&directory, &base);
        self.turn_agent_channel_on_for_a_session();
        let trusted = project.and_then(|id| TrustedRoot::of(&self.editor, id));
        let root = project
            .and_then(|id| self.editor.project(id))
            .map(|project| project.root().to_path_buf());
        self.order_session(Order {
            token: key,
            kind: Kind::New,
            name,
            directory,
            project: root,
            trusted,
            state: self.hosting.directory.clone(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            shell: None,
        });
    }

    /// `session-resume`: the stopped session whose terminal is `buffer`, in the same buffer.
    pub(in crate::app) fn resume_session(&mut self, buffer: BufferId) {
        let Some(session) = self.hosting.table.by_buffer(buffer) else {
            self.editor.message("This buffer holds no session");
            return;
        };
        if session.runs() {
            self.editor
                .message(format!("Session {} runs", session.name));
            return;
        }
        let order = Order {
            token: session.key,
            kind: Kind::Resume(session.id.clone()),
            name: session.name.clone(),
            directory: session.directory.clone(),
            project: session.root.clone(),
            // A resume takes no baseline: the session keeps the one it had.
            trusted: None,
            state: self.hosting.directory.clone(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            shell: None,
        };
        let key = session.key;
        self.turn_agent_channel_on_for_a_session();
        if let Some(session) = self.hosting.table.get_mut(key) {
            session.run = Run::Starting;
        }
        self.order_session(order);
    }

    /// `session-fork`: the conversation of the session whose terminal is `buffer`, copied
    /// under a new id, in a new buffer.
    pub(in crate::app) fn fork_session(&mut self, buffer: BufferId) {
        let Some(session) = self.hosting.table.by_buffer(buffer) else {
            self.editor.message("This buffer holds no session");
            return;
        };
        let (from, directory, root, base) = (
            session.id.clone(),
            session.directory.clone(),
            session.root.clone(),
            session.name.clone(),
        );
        let (key, name) = self.hosting.table.reserve(&directory, &base);
        self.turn_agent_channel_on_for_a_session();
        self.order_session(Order {
            token: key,
            kind: Kind::Fork(from),
            name,
            directory,
            project: root,
            trusted: None,
            state: self.hosting.directory.clone(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            shell: None,
        });
    }

    fn order_session(&self, order: Order) {
        let waker = std::sync::Mutex::new(self.waker.clone());
        start::spawn(order, move |report| {
            waker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .send(UserEvent::SessionStart(report));
        });
    }

    /// What a session's start thread found.
    pub(in crate::app) fn session_start_reported(&mut self, report: Report) {
        match report {
            Report::Ready(ready) => self.session_ready(ready),
            Report::Refused { token, why } => {
                self.session_not_started(token);
                self.editor.message(why);
            }
            Report::Live {
                token,
                pid,
                directory,
            } => {
                self.session_not_started(token);
                let name = self
                    .hosting
                    .table
                    .get(token)
                    .map_or_else(String::new, |session| session.name.clone());
                let place = directory
                    .map(|directory| format!(" in {}", directory.display()))
                    .unwrap_or_default();
                let question = format!(
                    "Session {name} runs already as process {pid}{place}; fork it instead?"
                );
                if let Err(e) = self.editor.ask(question, "session-fork") {
                    self.editor.message(e.to_string());
                }
            }
            Report::Baseline { token, tree } => {
                if let Some(session) = self.hosting.table.get_mut(token) {
                    session.baseline = Some(tree);
                    self.remember_sessions();
                }
            }
            Report::Twice { token } => {
                if let Some(session) = self.hosting.table.get(token) {
                    let name = session.name.clone();
                    self.editor.message(format!(
                        "Two processes run session {name} now; one may overwrite the other's conversation"
                    ));
                }
            }
        }
        self.request_redraw();
    }

    /// A start that did not happen: a held name goes, and a resumed session is stopped again.
    fn session_not_started(&mut self, token: SessionKey) {
        if self.hosting.table.release(token).is_some() {
            return;
        }
        if let Some(session) = self.hosting.table.get_mut(token)
            && session.run == Run::Starting
        {
            session.run = Run::Stopped;
        }
    }

    /// The start thread got the session ready: it starts in its terminal.
    fn session_ready(&mut self, ready: Ready) {
        let key = ready.token;
        let new = self.hosting.table.get(key).is_none();
        if new {
            let added = self.hosting.table.add(
                key,
                New {
                    id: ready.id.clone(),
                    root: ready.project.clone(),
                    parent: None,
                    changed: now_ms(),
                },
            );
            if added.is_none() {
                return;
            }
        }
        if let Some(session) = self.hosting.table.get_mut(key) {
            if ready.baseline.is_some() {
                session.baseline.clone_from(&ready.baseline);
            }
            session.changed = now_ms();
        }
        if let Err(why) = self.start_session_terminal(key, &ready) {
            if new {
                self.hosting.table.forget(key);
            } else {
                self.session_not_started(key);
            }
            self.editor.message(why);
        }
        self.remember_sessions();
    }

    /// Starts the terminal of session `key` as `ready` says.
    fn start_session_terminal(&mut self, _key: SessionKey, _ready: &Ready) -> Result<(), String> {
        Err("Sessions run in a terminal, which this build of oso does not have yet".into())
    }

    /// Brings session `key` back as a terminal buffer with no process, which says
    /// `stopped: session-resume to continue`.
    fn bring_back_stopped(&mut self, key: SessionKey) {
        if let Some(session) = self.hosting.table.get(key) {
            log::debug!("session {} remembered, stopped", session.name);
        }
    }

    /// Whether the connection of `peer` that claims the session `claim` is that hosted
    /// session's bridge (ADR 0121): then it holds the session, and its name is given.
    pub(in crate::app) fn session_bridge(
        &mut self,
        connection: Connection,
        peer: &Peer,
        claim: &str,
    ) -> Option<String> {
        let id = SessionId::parse(claim)?;
        self.hosting
            .table
            .admit(connection, peer, Some(&id))
            .map(|session| session.name.clone())
    }

    /// The bridge connection `connection` went, or every one did, when it is `None`.
    pub(in crate::app) fn session_bridge_gone(&mut self, connection: Option<Connection>) {
        match connection {
            Some(connection) => self.hosting.table.bridge_gone(connection),
            None => self.hosting.table.bridges_gone(),
        }
    }
}
