//! The language servers the app runs (ADR 0138, steps 3 and 4), on the UI thread: which server a
//! buffer's open would start, starting one for a project's root and entry, what the commands
//! ask, what each server's thread reports, the modeline's marks, and the ends that trust, the
//! configuration and a quit bring. One server runs per root and entry. The UI thread never
//! reads, writes or waits on a server: each has its own threads in `oso-lsp`, and the gate
//! they run before each start checks the trust again and finds the program.
//!
//! Must not depend on: the window system or the drawing; this keeps the servers and tells the
//! editor what to show.

mod diagnostics;
mod documents;
mod find;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use oso_core::buffer::{BufferId, ServerMark};
use oso_core::editor::ServerRequest;
use oso_core::project::ProjectId;
use oso_lsp::server::{Gate, Server, Start, State};

use super::{App, UserEvent};
use crate::config::{LspEntry, StartWhen};
use crate::git::TrustedRoot;

/// The servers running, and what the person stopped.
#[derive(Debug, Default)]
pub(super) struct Servers {
    running: Vec<Running>,
    /// The roots and entries the person stopped with `lsp-shutdown`: a visit does not start
    /// them again until `lsp-start` or `lsp-restart` (step 3's Q2).
    shut_down: HashSet<(PathBuf, String)>,
    /// The projects trusted when the record was last read, to tell what trusting starts.
    trusted: HashSet<ProjectId>,
    /// The number the next server's threads are named with.
    next: usize,
    /// When the echo area says the diagnostic at point (step 6).
    at_point: diagnostics::AtPoint,
}

/// One server, as the app knows it.
#[derive(Debug)]
struct Running {
    root: PathBuf,
    project: ProjectId,
    entry: LspEntry,
    trust: TrustedRoot,
    server: Server,
    state: State,
    /// Asked to stop, so that it is not started again before it has.
    stopping: bool,
    /// `lsp-start` or `lsp-restart` came while it stopped: once it has ended, this buffer's
    /// server starts again, as the ask cannot reach a thread that is ending.
    start_again: Option<BufferId>,
    /// The documents opened with it, by buffer (step 4).
    documents: HashMap<BufferId, documents::Open>,
}

impl Servers {
    fn find(&self, root: &std::path::Path, name: &str) -> Option<usize> {
        self.running
            .iter()
            .position(|running| running.root == root && running.entry.name == name)
    }
}

impl App {
    /// Whether opening `id` would start a server for the person: its mode has one, its project
    /// has none running, the person did not stop it, and servers start on a visit.
    pub(super) fn would_start_server(&self, id: BufferId) -> bool {
        if self.configuration.lsp().start != StartWhen::Trusted {
            return false;
        }
        let Some((root, _, entry)) = self.server_key(id) else {
            return false;
        };
        self.servers.find(root, &entry.name).is_none()
            && !self
                .servers
                .shut_down
                .iter()
                .any(|(stopped, name)| stopped == root && *name == entry.name)
    }

    /// The root, project and entry of the server `id` would have, if any, borrowed.
    fn server_key(&self, id: BufferId) -> Option<(&std::path::Path, ProjectId, &LspEntry)> {
        let buffer = self.editor.buffer(id)?;
        buffer.path()?;
        if buffer.is_internal() || buffer.is_terminal() {
            return None;
        }
        let project = buffer.project()?;
        let root = self.editor.project(project)?.root();
        let entry = self.configuration.lsp().for_mode(buffer.mode())?;
        Some((root, project, entry))
    }

    /// As [`App::server_key`], owned, for a start.
    fn server_of(&self, id: BufferId) -> Option<(PathBuf, ProjectId, LspEntry)> {
        let (root, project, entry) = self.server_key(id)?;
        Some((root.to_owned(), project, entry.clone()))
    }

    /// Starts the server `id` would have, in a trusted project, unless one runs.
    pub(super) fn start_server_for(&mut self, id: BufferId) {
        let Some((root, project, entry)) = self.server_of(id) else {
            return;
        };
        if self.servers.find(&root, &entry.name).is_some() {
            return;
        }
        let Some(trust) = TrustedRoot::of(&self.editor, project) else {
            return;
        };
        self.servers
            .shut_down
            .remove(&(root.clone(), entry.name.clone()));
        self.servers.next += 1;
        let number = self.servers.next;
        let waker = self.waker.clone();
        let started = Server::start(
            start_of(&root, &entry, number),
            gate(&trust, &entry, &root),
            Box::new(move || {
                waker.send(UserEvent::Servers);
            }),
        );
        match started {
            Ok(server) => {
                self.servers.running.push(Running {
                    root,
                    project,
                    entry,
                    trust,
                    server,
                    state: State::Starting,
                    stopping: false,
                    start_again: None,
                    documents: HashMap::new(),
                });
                // Its project's buffers go to it, once it is ready, but the agent's
                // (ADR 0118).
                self.open_documents_of(self.servers.running.len() - 1);
            }
            Err(error) => self
                .editor
                .message(format!("{}: could not start: {error}", entry.name)),
        }
        self.mark_servers();
    }

    /// Carries out what `lsp-start`, `lsp-restart` and `lsp-shutdown` asked. After each event.
    pub(super) fn sync_servers(&mut self) {
        self.stop_untrusted_servers();
        if self.sync_documents() {
            self.mark_servers();
        }
        for request in self.editor.take_server_requests() {
            match request {
                ServerRequest::Start(id) => self.lsp_start(id, false),
                ServerRequest::Restart(id) => self.lsp_start(id, true),
                ServerRequest::ShutDown(id) => self.lsp_shutdown(id),
            }
        }
    }

    /// `lsp-start`, or `lsp-restart` if `again`.
    fn lsp_start(&mut self, id: BufferId, again: bool) {
        let Some((root, project, entry)) = self.server_of(id) else {
            self.editor
                .message("No language server for this buffer's mode and project");
            return;
        };
        if !self.editor.project_trusted(project) {
            self.editor.message(format!(
                "{} is restricted: trust-project lets its language server run",
                root.display()
            ));
            return;
        }
        // The person's ask lifts an `lsp-shutdown` (Q2).
        self.servers
            .shut_down
            .remove(&(root.clone(), entry.name.clone()));
        let Some(index) = self.servers.find(&root, &entry.name) else {
            self.start_server_for(id);
            return;
        };
        let running = &mut self.servers.running[index];
        if running.stopping {
            running.start_again = Some(id);
            return;
        }
        if !again && !matches!(running.state, State::Stopped) {
            self.editor.message(format!(
                "{} already runs for {}",
                entry.name,
                root.display()
            ));
            return;
        }
        // Started again with the configuration as it is now.
        let Some(trust) = TrustedRoot::of(&self.editor, project) else {
            return;
        };
        running.trust.revoke();
        running.trust = trust;
        running.entry = entry;
        self.servers.next += 1;
        let number = self.servers.next;
        let running = &mut self.servers.running[index];
        running.server.restart(
            start_of(&running.root, &running.entry, number),
            gate(&running.trust, &running.entry, &running.root),
        );
    }

    /// `lsp-shutdown`.
    fn lsp_shutdown(&mut self, id: BufferId) {
        let Some((root, _, entry)) = self.server_of(id) else {
            self.editor
                .message("No language server follows this buffer");
            return;
        };
        let Some(index) = self.servers.find(&root, &entry.name) else {
            self.editor
                .message("No language server follows this buffer");
            return;
        };
        let running = &mut self.servers.running[index];
        running.server.shut_down();
        running.stopping = true;
        self.servers
            .shut_down
            .insert((root.clone(), entry.name.clone()));
        self.editor
            .message(format!("{} stops for {}", entry.name, root.display()));
    }

    /// What the servers' threads reported: their states, `*LSP*`'s lines, their messages and
    /// diagnostics.
    pub(super) fn servers_reported(&mut self) {
        let mut ended = Vec::new();
        let (mut changed, mut said, mut logged, mut diagnosed) = (false, false, false, false);
        for (index, running) in self.servers.running.iter_mut().enumerate() {
            let Some(report) = running.server.take_report() else {
                continue;
            };
            changed |= running.state != report.state;
            running.state = report.state;
            if !report.log.is_empty() {
                let label = format!(
                    "{} {}",
                    running.entry.name,
                    running
                        .root
                        .file_name()
                        .map(|name| name.to_string_lossy())
                        .unwrap_or_default()
                );
                // One edit for the whole report, however many lines it has.
                self.editor.server_log(
                    &label,
                    report.log.iter().map(String::as_str),
                    oso_lsp::limits::LOG_BYTES_MAX,
                );
                logged = true;
            }
            for (id, closed) in report.documents {
                if let Some((buffer, open)) = running
                    .documents
                    .iter_mut()
                    .find(|(buffer, _)| buffer.get() == id)
                {
                    open.closed = closed;
                    changed = true;
                    if closed.is_some() {
                        diagnosed |= diagnostics::clear(&mut self.editor, *buffer);
                    }
                }
            }
            diagnosed |=
                diagnostics::take(&mut self.editor, &running.documents, report.diagnostics);
            for shown in report.shown {
                // The server's name first, so that no server passes as oso. Its text may
                // quote what it read, so the log gets only its size.
                log::info!(
                    "{}: a message of {} bytes",
                    running.entry.name,
                    shown.text.len()
                );
                self.editor
                    .message_unlogged(format!("{}: {}", running.entry.name, shown.text));
                said = true;
            }
            if report.state == State::Ended {
                ended.push(index);
            }
        }
        let mut again = Vec::new();
        for index in ended.into_iter().rev() {
            let running = self.servers.running.remove(index);
            again.extend(running.start_again);
            self.unfollow(running.documents.into_keys());
        }
        for id in again {
            self.start_server_for(id);
        }
        if changed {
            self.mark_servers();
        }
        if changed || said || diagnosed || (logged && self.editor.server_log_shown()) {
            self.request_redraw();
        }
    }

    /// Sets each buffer's modeline mark from the server that has its document, open and not
    /// kept from it (step 4's Q3): a buffer held for the person shows `waits: server` instead.
    pub(super) fn mark_servers(&mut self) {
        let running = &self.servers.running;
        let marks: Vec<(BufferId, Option<ServerMark>)> = self
            .editor
            .buffers()
            .filter_map(|(id, buffer)| {
                let mark = running
                    .iter()
                    .find(|running| {
                        running
                            .documents
                            .get(&id)
                            .is_some_and(|open| open.closed.is_none())
                    })
                    .and_then(|running| mark(running.state));
                (buffer.server() != mark).then_some((id, mark))
            })
            .collect();
        for (id, mark) in marks {
            if let Some(buffer) = self.editor.buffer_mut(id) {
                buffer.set_server(mark);
            }
        }
    }

    /// Stops the servers of projects no longer trusted, at once, before the record of trusted
    /// projects is written and read back: a crashed server waiting to start again then finds
    /// its gate closed. After each event; a few servers, so it costs nothing.
    fn stop_untrusted_servers(&mut self) {
        let editor = &self.editor;
        for running in &mut self.servers.running {
            if !running.stopping && !editor.project_trusted(running.project) {
                running.trust.revoke();
                running.server.shut_down();
                running.stopping = true;
            }
        }
    }

    /// After the record of trusted projects was read: an untrusted project's servers stop, and
    /// a project trusted now settles its buffers' opens again, so that those the person visited
    /// start their servers (Q5).
    pub(super) fn servers_follow_trust(&mut self) {
        self.stop_untrusted_servers();
        let editor = &self.editor;
        let trusted: HashSet<ProjectId> = editor
            .buffers()
            .filter_map(|(_, buffer)| buffer.project())
            .filter(|id| editor.project_trusted(*id))
            .collect();
        let newly: Vec<ProjectId> = trusted.difference(&self.servers.trusted).copied().collect();
        self.servers.trusted = trusted;
        for project in newly {
            self.unsettle_opens_in(project);
        }
    }

    /// After the configuration changed: a server whose entry went is stopped, and one stopped
    /// for the session whose entry changed starts again, as it may be found now. A change to a
    /// running server applies at `lsp-restart`.
    pub(super) fn servers_follow_configuration(&mut self) {
        let lsp = self.configuration.lsp().clone();
        let mut again = Vec::new();
        for (index, running) in self.servers.running.iter_mut().enumerate() {
            let now = lsp
                .servers
                .iter()
                .find(|entry| entry.name == running.entry.name);
            match now {
                None if !running.stopping => {
                    running.trust.revoke();
                    running.server.shut_down();
                    running.stopping = true;
                }
                Some(entry) if *entry != running.entry && running.state == State::Stopped => {
                    running.entry = entry.clone();
                    again.push(index);
                }
                _ => {}
            }
        }
        for index in again {
            self.servers.next += 1;
            let number = self.servers.next;
            let running = &self.servers.running[index];
            running.server.restart(
                start_of(&running.root, &running.entry, number),
                gate(&running.trust, &running.entry, &running.root),
            );
        }
    }

    /// As oso quits: each server's group gets `SIGTERM`, with no wait.
    pub(super) fn quit_servers(&self) {
        for running in &self.servers.running {
            running.server.quit();
        }
    }
}

/// What the modeline says of a server in `state`.
fn mark(state: State) -> Option<ServerMark> {
    match state {
        State::Starting | State::Waiting => Some(ServerMark::Starting),
        State::Ready => Some(ServerMark::Ready),
        State::Working(percent) => Some(ServerMark::Working(percent)),
        State::Stopped => Some(ServerMark::Stopped),
        State::Ended => None,
    }
}

/// What the server of `entry` for `root` starts with.
fn start_of(root: &std::path::Path, entry: &LspEntry, number: usize) -> Start {
    Start {
        name: entry.name.clone(),
        number,
        args: entry.command.iter().skip(1).map(Into::into).collect(),
        root: root.to_owned(),
        inherit: entry.env.clone(),
        section: entry.section.clone(),
        options: entry.options.clone(),
    }
}

/// The gate the server's thread runs before each start: the trust, then the program.
fn gate(trust: &TrustedRoot, entry: &LspEntry, root: &std::path::Path) -> Gate {
    let trust = trust.clone();
    let program = entry.command.first().cloned().unwrap_or_default();
    let in_cargo_bin = entry.in_cargo_bin;
    let root = root.to_owned();
    Box::new(move || {
        trust.check()?;
        find::program(&program, in_cargo_bin, &root)
    })
}
