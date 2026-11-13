//! Hosted sessions in the app (ADR 0121): starting, resuming and forking them at the person's
//! command, the start thread's reports, the remembered set, and the bridge connections that
//! hold a session. The work that touches files and processes is on the start thread and the
//! sessions worker; this only decides and carries out.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use oso_core::buffer::BufferId;
use oso_core::minibuffer::PromptId;
use oso_core::project::ProjectId;
use oso_platform::control::Peer;

use super::super::{App, UserEvent};
use crate::channel::Connection;
use crate::review::TrustedRoot;
use crate::sessions::hosted::{New, Run, Table};
use crate::sessions::identity::SessionId;
use crate::sessions::start::{self, Kind, Order, Ready, Report};
use crate::sessions::worker::{Job, SessionKey};

mod terminal;

/// What the app keeps of its hosted sessions.
#[derive(Debug, Default)]
pub(in crate::app) struct Hosting {
    pub(in crate::app) table: Table,
    /// oso's own state directory, where the record is kept.
    pub(in crate::app) directory: Option<PathBuf>,
    /// The buffer each session being started has, by its key, until the start thread reports.
    starting: HashMap<SessionKey, BufferId>,
    /// The trusted project each new session takes a baseline of, and the root its baseline
    /// thread runs git in, by its key, until the baseline comes or the start is refused: the
    /// review takes it as the project's, and the root is revoked with the project's trust.
    baselines: HashMap<SessionKey, (ProjectId, TrustedRoot)>,
    /// The sessions changed since the worker was last given them to remember.
    unsaved: bool,
    /// The "fork it instead?" question of a refused resume, and the session it is about: its
    /// `y` forks that session, whichever buffer is selected then.
    fork_offered: Option<(PromptId, SessionKey)>,
    /// The internal `*Session explain*` buffer, once there is one. Kept by its id, never found
    /// by its name, which anyone may give a buffer of their own.
    pub(in crate::app) explain: Option<BufferId>,
    /// The sessions being started by another session's hand-off, by key: their parent.
    parents: HashMap<SessionKey, SessionId>,
    /// A start the person was asked about, as another session runs in its directory.
    shared: Option<Begin>,
    /// Worktree sessions: briefs, the jobs, `*Worktrees*` and a removal (ADR 0121 step 4).
    pub(in crate::app) worktrees: super::worktrees::Worktrees,
    /// The trust each session started under, for its agent's worktree tools (step 6), until
    /// it is forgotten.
    pub(in crate::app) trusts: HashMap<SessionKey, TrustedRoot>,
    /// The agents' session tools (ADR 0121 step 6).
    pub(in crate::app) tools: super::tools::Tools,
    /// The spaces (ADR 0121, Q8): step 4 opens a worktree session's, nested under its
    /// repository's.
    pub(in crate::app) spaces: crate::sessions::spaces::Spaces,
}

/// What a new session starts with.
#[derive(Debug, Clone, Default)]
pub(in crate::app) struct Begin {
    /// Its name's base, else its directory's name.
    pub name: Option<String>,
    /// Its directory, else the selected buffer's project root, else its file's directory, else
    /// the directory oso started in.
    pub directory: Option<PathBuf>,
    /// Its first prompt (ADR 0121 step 4).
    pub brief: Option<String>,
    /// The session that started it.
    pub parent: Option<SessionId>,
    /// Whether to ask first when another live session runs in its directory.
    pub check_shared: bool,
    /// The trust of its directory, when it is no open project's: a worktree just made.
    pub trusted: Option<TrustedRoot>,
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
            starting: HashMap::new(),
            baselines: HashMap::new(),
            unsaved: false,
            fork_offered: None,
            explain: None,
            parents: HashMap::new(),
            shared: None,
            worktrees: super::worktrees::Worktrees::default(),
            trusts: HashMap::new(),
            tools: super::tools::Tools::default(),
            spaces: crate::sessions::spaces::Spaces::default(),
        }
    }
}

/// Now, in ms since the epoch, for a session's last change.
pub(in crate::app) fn now_ms() -> u64 {
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

/// Revokes and forgets each of `baselines` whose project `trusted` no longer says is trusted.
fn revoke_untrusted(
    baselines: &mut HashMap<SessionKey, (ProjectId, TrustedRoot)>,
    trusted: impl Fn(ProjectId) -> bool,
) {
    baselines.retain(|_, (project, root)| {
        let kept = trusted(*project);
        if !kept {
            root.revoke();
        }
        kept
    });
}

/// The most bytes of a registry's directory a question shows.
const MAX_SHOWN_DIRECTORY: usize = 200;

/// A directory from Claude Code's registry, which any process of the person's may write, as a
/// question shows it: escaped so that it stays on one row and cannot reorder it, and cut short.
fn shown_directory(directory: &Path) -> String {
    let mut shown = oso_core::editor::row_text(&directory.to_string_lossy());
    if shown.len() > MAX_SHOWN_DIRECTORY {
        shown.truncate(shown.floor_char_boundary(MAX_SHOWN_DIRECTORY));
        shown.push('…');
    }
    shown
}

impl App {
    /// The sessions an earlier oso remembered: each that is not archived comes back stopped,
    /// with nothing started.
    pub(in crate::app) fn sessions_recalled(
        &mut self,
        remembered: Vec<crate::sessions::record::Remembered>,
    ) {
        let back = self.hosting.table.recall(remembered);
        self.prune_sessions();
        for key in back {
            if self.hosting.table.get(key).is_some() {
                self.bring_back_stopped(key);
            }
        }
    }

    /// Notes that the sessions changed: [`App::save_sessions`] hands them to the worker once
    /// for every change an event made.
    fn remember_sessions(&mut self) {
        self.hosting.unsaved = true;
    }

    /// After an event: hands the sessions to the worker to remember, keeping another
    /// instance's, if they changed.
    pub(in crate::app) fn save_sessions(&mut self) {
        if !std::mem::take(&mut self.hosting.unsaved) {
            return;
        }
        if let (Some(worker), Some(directory)) = (&self.sessions, &self.hosting.directory) {
            worker.send(Job::Remember {
                directory: directory.clone(),
                sessions: self.hosting.table.remembered(),
                known: self.hosting.table.known().clone(),
            });
        }
    }

    /// `claude`: a new session in `directory`, else the selected buffer's project root, else
    /// its file's directory, as a new terminal, else the directory oso started in, named
    /// `name`, else after its directory. It turns the agent channel on if it is off (Q20).
    pub(in crate::app) fn start_session(
        &mut self,
        name: Option<String>,
        directory: Option<PathBuf>,
    ) {
        self.begin_session(Begin {
            name,
            directory,
            check_shared: true,
            ..Begin::default()
        });
    }

    /// Starts a new session as `begin` says, and returns the key it starts under, unless it
    /// was refused at once or waits for the person's answer.
    pub(in crate::app) fn begin_session(&mut self, begin: Begin) -> Option<SessionKey> {
        let directory = match begin.directory.clone() {
            Some(directory) if !directory.is_absolute() => {
                self.editor.message("Name the session's directory whole");
                return None;
            }
            Some(directory) => directory,
            None => self
                .buffer_directory()
                .unwrap_or_else(|| self.editor.start_directory().to_path_buf()),
        };
        if begin.check_shared && self.hosting.table.runs_in(&directory) {
            self.ask_shared(begin);
            return None;
        }
        // The selected buffer's project, if the session runs at its root.
        let project = self.editor.current().1.project().filter(|id| {
            self.editor
                .project(*id)
                .is_some_and(|project| project.root() == directory)
        });
        let base = begin.name.clone().unwrap_or_else(|| base_name(&directory));
        let (key, name) = self.hosting.table.reserve(&directory, &base);
        if let Some(parent) = begin.parent.clone() {
            self.hosting.parents.insert(key, parent);
        }
        if begin.check_shared {
            self.hosting.shared = Some(begin.clone());
        }
        self.turn_agent_channel_on_for_a_session();
        let terminal = self.session_terminal(key, &name, None);
        let (trusted, root) = match begin.trusted.clone() {
            // A worktree just made, which no buffer opened yet: its own root and trust.
            Some(trusted) => (Some(trusted), Some(directory.clone())),
            None => {
                let trusted = project.and_then(|id| TrustedRoot::of(&self.editor, id));
                if let (Some(id), Some(root)) = (project, &trusted) {
                    self.hosting.baselines.insert(key, (id, root.clone()));
                }
                let root = project
                    .and_then(|id| self.editor.project(id))
                    .map(|project| project.root().to_path_buf());
                (trusted, root)
            }
        };
        if let Some(trusted) = &trusted {
            self.hosting.trusts.insert(key, trusted.clone());
        }
        self.order_session(Order {
            token: key,
            kind: Kind::New,
            name,
            directory,
            project: root,
            trusted,
            state: super::super::reviews::state(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            terminal,
            brief: begin.brief,
            steer_worktrees: self.configuration.steer_worktrees(),
            check_shared: begin.check_shared,
        });
        Some(key)
    }

    /// Asks before `begin` starts a session where another live one runs (ADR 0121).
    fn ask_shared(&mut self, begin: Begin) {
        self.hosting.shared = Some(begin);
        let question = "Another session runs in this directory. A worktree keeps their changes \
                        apart (C-x a w). Start here anyway?";
        if let Err(e) = self.editor.ask(question, "session-start-shared") {
            self.hosting.shared = None;
            self.editor.message(e.to_string());
        }
    }

    /// The person's `y` to starting where another session runs.
    pub(in crate::app) fn start_shared(&mut self) {
        match self.hosting.shared.take() {
            Some(begin) => {
                self.begin_session(Begin {
                    check_shared: false,
                    ..begin
                });
            }
            None => self.editor.message("No session waits to start"),
        }
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
        let (key, kind, name, directory, root) = (
            session.key,
            Kind::Resume(session.id.clone()),
            session.name.clone(),
            session.directory.clone(),
            session.root.clone(),
        );
        self.turn_agent_channel_on_for_a_session();
        if let Some(session) = self.hosting.table.get_mut(key) {
            session.run = Run::Starting;
        }
        let terminal = self.session_terminal(key, &name, Some(buffer));
        self.order_session(Order {
            token: key,
            kind,
            name,
            directory,
            project: root,
            // A resume takes no baseline: the session keeps the one it had.
            trusted: None,
            state: super::super::reviews::state(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            terminal,
            brief: None,
            steer_worktrees: self.configuration.steer_worktrees(),
            check_shared: false,
        });
    }

    /// `session-fork`: the conversation of the session whose terminal is `buffer`, copied
    /// under a new id, in a new buffer; or, as the `y` of a refused resume's question, that
    /// session's.
    pub(in crate::app) fn fork_session(&mut self, buffer: BufferId) {
        let offered = self.answered_fork_offer();
        let session = match offered {
            Some(key) => self.hosting.table.get(key),
            None => self.hosting.table.by_buffer(buffer),
        };
        let Some(session) = session else {
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
        let terminal = self.session_terminal(key, &name, None);
        self.order_session(Order {
            token: key,
            kind: Kind::Fork(from),
            name,
            directory,
            project: root,
            trusted: None,
            state: super::super::reviews::state(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            terminal,
            brief: None,
            steer_worktrees: self.configuration.steer_worktrees(),
            check_shared: false,
        });
    }

    /// The session of the refused resume whose question just closed, once: the question was
    /// answered `y` if `session-fork` runs now.
    fn answered_fork_offer(&mut self) -> Option<SessionKey> {
        let (asked, _) = self.hosting.fork_offered?;
        if self
            .editor
            .prompts()
            .iter()
            .any(|prompt| prompt.id() == asked)
        {
            return None;
        }
        self.hosting.fork_offered.take().map(|(_, key)| key)
    }

    /// After the requests of an event: a fork offer whose question closed without a `y` goes,
    /// so that a later `session-fork` forks the selected buffer's session.
    pub(in crate::app) fn drop_unanswered_fork_offer(&mut self) {
        let _ = self.answered_fork_offer();
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
                self.hosting.parents.remove(&token);
                self.session_tool_started(token, Err(why.clone()));
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
                    .map(|directory| format!(" in {}", shown_directory(&directory)))
                    .unwrap_or_default();
                let question = match pid {
                    Some(pid) => format!(
                        "Session {name} runs already as process {pid}{place}; fork it instead?"
                    ),
                    None => format!(
                        "Session {name} may run already: oso cannot tell from Claude Code's registry; fork it instead?"
                    ),
                };
                match self.editor.ask(question, "session-fork") {
                    Ok(asked) => self.hosting.fork_offered = Some((asked, token)),
                    Err(e) => self.editor.message(e.to_string()),
                }
            }
            Report::Baseline { token, tree } => {
                self.session_baseline_to_review(token, &tree);
                if let Some(session) = self.hosting.table.get_mut(token) {
                    session.baseline = Some(tree);
                    self.remember_sessions();
                }
            }
            Report::Shared { token } => {
                self.session_tool_started(token, Err("another session runs there".into()));
                self.session_not_started(token);
                self.hosting.parents.remove(&token);
                if let Some(begin) = self.hosting.shared.take() {
                    self.ask_shared(begin);
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

    /// Session `key`'s start baseline goes to its project's review, once.
    fn session_baseline_to_review(&mut self, key: SessionKey, tree: &str) {
        if let Some((project, _)) = self.hosting.baselines.remove(&key) {
            self.session_baseline_taken(project, tree);
        }
    }

    /// Revokes the root of each start baseline whose project is no longer trusted, which
    /// stops its git run between requests, and forgets it.
    pub(in crate::app) fn revoke_untrusted_session_baselines(&mut self) {
        let editor = &self.editor;
        revoke_untrusted(&mut self.hosting.baselines, |project| {
            editor.project_trusted(project)
        });
    }

    /// A start that did not happen: a held name goes, and a resumed session is stopped again.
    /// The agent channel goes off again if this start turned it on and nothing else needs it.
    fn session_not_started(&mut self, token: SessionKey) {
        self.session_terminal_not_started(token);
        self.hosting.baselines.remove(&token);
        if self.hosting.table.release(token).is_some() {
            self.hosting.trusts.remove(&token);
        } else if let Some(session) = self.hosting.table.get_mut(token)
            && session.run == Run::Starting
        {
            session.run = Run::Stopped;
        }
        let needed = !self.hosting.starting.is_empty()
            || self.hosting.table.iter().any(|session| session.runs());
        self.turn_agent_channel_off_unless_a_session_needs_it(needed);
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
                    parent: self.hosting.parents.remove(&key),
                    changed: now_ms(),
                },
            );
            if added.is_none() {
                return;
            }
            self.prune_sessions();
        }
        if let Some(tree) = &ready.baseline {
            self.session_baseline_to_review(key, tree);
        }
        if let Some(session) = self.hosting.table.get_mut(key) {
            if ready.baseline.is_some() {
                session.baseline.clone_from(&ready.baseline);
            }
            session.changed = now_ms();
        }
        let started = self.session_terminal_started(key, ready);
        // An agent that asked for it hears how it went, while the session is still listed.
        self.session_tool_started(
            key,
            if started {
                Ok(())
            } else {
                Err("its buffer went while it started".into())
            },
        );
        if !started {
            // Its buffer went while it started: the terminal is dropped, which hangs it up.
            if new {
                self.hosting.table.forget(key);
                self.hosting.trusts.remove(&key);
            } else if let Some(session) = self.hosting.table.get_mut(key) {
                session.run = Run::Stopped;
            }
        }
        self.remember_sessions();
    }

    /// Whether the connection of `peer` that claims the session `claim` is that hosted
    /// session's bridge (ADR 0121), answered with `answer`, which says whether the answer
    /// reached the server: then it holds the session, and its name is given.
    pub(in crate::app) fn session_bridge(
        &mut self,
        connection: Connection,
        peer: &Peer,
        claim: &str,
        answer: impl FnOnce(bool) -> bool,
    ) -> Option<String> {
        let Some(id) = SessionId::parse(claim) else {
            answer(false);
            return None;
        };
        // Each running `claude`'s id as its terminal has it now: set once the child started,
        // cleared before it is reaped.
        let children: Vec<(SessionKey, Option<u32>)> = self
            .hosting
            .table
            .iter()
            .filter(|session| matches!(session.run, Run::Running { .. }))
            .map(|session| {
                let child = session
                    .buffer
                    .and_then(|buffer| self.terminals.map.get(buffer))
                    .and_then(|hosted| hosted.terminal.as_ref())
                    .and_then(oso_term::Terminal::pid);
                (session.key, child)
            })
            .collect();
        for (key, child) in children {
            if let Some(session) = self.hosting.table.get_mut(key) {
                session.run = Run::Running { child };
            }
        }
        self.hosting
            .table
            .answer_claim(connection, peer, Some(&id), answer)
    }

    /// The bridge connection `connection` went, or every one did, when it is `None`.
    pub(in crate::app) fn session_bridge_gone(&mut self, connection: Option<Connection>) {
        match connection {
            Some(connection) => self.hosting.table.bridge_gone(connection),
            None => self.hosting.table.bridges_gone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use std::collections::HashMap;

    use super::{MAX_SHOWN_DIRECTORY, revoke_untrusted, shown_directory};
    use crate::review::TrustedRoot;

    #[test]
    fn a_start_baseline_of_a_project_no_longer_trusted_is_revoked_and_forgotten() {
        let (_editor, project) = crate::commands::review::tests::in_project(true);
        let root = TrustedRoot::for_tests(std::env::temp_dir());
        let mut baselines = HashMap::from([(1, (project, root.clone()))]);
        revoke_untrusted(&mut baselines, |_| true);
        assert!(!root.revoked());
        assert_eq!(baselines.len(), 1);
        revoke_untrusted(&mut baselines, |_| false);
        assert!(root.revoked(), "its git run stops between requests");
        assert!(baselines.is_empty());
    }

    #[test]
    fn a_registry_directory_is_shown_escaped_on_one_row_and_cut_short() {
        let shown = shown_directory(Path::new("/p/a\nb\u{202e}c"));
        assert!(!shown.contains('\n'), "{shown}");
        assert!(!shown.contains('\u{202e}'), "{shown}");
        let long = format!("/{}", "é".repeat(400));
        let shown = shown_directory(Path::new(&long));
        assert!(
            shown.len() <= MAX_SHOWN_DIRECTORY + '…'.len_utf8(),
            "{}",
            shown.len()
        );
        assert!(shown.ends_with('…'));
        assert_eq!(shown_directory(Path::new("/p/oso")), "/p/oso");
    }
}
