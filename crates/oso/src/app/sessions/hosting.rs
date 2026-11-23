//! Hosted sessions in the app (ADR 0121): starting, resuming and forking them at the person's
//! command, or at an agent's tool's (ADR 0153), the start thread's reports and the remembered
//! set; `bridge`: the connections that hold a session; `naming`: a rename the person was asked
//! about, and what Claude Code's registry calls oso's running sessions (ADR 0155); `shared`:
//! the questions asked where another session runs; `switch`: `session-switch-mode` (ADR 0165,
//! part 4d). The work that touches files and processes
//! is on the start thread and the sessions worker; this only decides and carries out.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::buffer::{BufferId, Mode};
use oso_core::minibuffer::PromptId;
use oso_core::project::ProjectId;

use super::super::App;
use crate::git::TrustedRoot;
use crate::sessions::hosted::{New, Run, Table};
use crate::sessions::identity::Origin;
use crate::sessions::identity::SessionId;
use crate::sessions::start::{self, Kind, NativeOrder, Order, Ready, Report, Runs};
use crate::sessions::worker::SessionKey;

mod adopt;
mod begin;
mod bridge;
mod carried;
mod fork;
mod held;
mod moving;
mod naming;
mod native;
mod record;
mod resume;
mod shared;
mod switch;
mod terminal;
mod trust;
pub(in crate::app) use terminal::buffer_name;

use fork::ForkFrom;

pub(in crate::app) use adopt::Adopting;
pub(in crate::app) use begin::{Begin, Then};
pub(in crate::app) use moving::{Moves, Moving};
pub(in crate::app) use native::NativeSessions;

/// What the app keeps of its hosted sessions.
#[derive(Debug, Default)]
pub(in crate::app) struct Hosting {
    pub(in crate::app) table: Table,
    /// oso's own state directory, where the record is kept.
    pub(in crate::app) directory: Option<PathBuf>,
    /// The buffer each session being started has, by its key, until the start thread reports.
    starting: HashMap<SessionKey, BufferId>,
    /// The baseline each new session takes, by its key, until it comes or the start is
    /// refused.
    baselines: HashMap<SessionKey, Baseline>,
    /// A native resume's, fork's or adoption's trust until it starts, taken back if its
    /// project's is.
    native_trusts: HashMap<SessionKey, crate::sessions::start::trust::Kept>,
    /// The sessions changed since the worker was last given them to remember.
    pub(in crate::app) unsaved: bool,
    /// The "fork it instead?" questions of refused resumes, and the conversation each is
    /// about: a `y` forks the one whose question it closed, whichever buffer is selected then.
    fork_offered: Vec<(PromptId, ForkFrom)>,
    /// What `session-adopt` lists, and the adoptions being started.
    pub(in crate::app) adopting: Adopting,
    /// What `session-search`'s rows are of (ADR 0145).
    pub(in crate::app) searching: super::conversations::Searching,
    /// What the space commands wait for (ADR 0136).
    pub(in crate::app) spaces: super::spaces::SpaceWork,
    /// The internal `*Session explain*` buffer, once there is one. Kept by its id, never found
    /// by its name, which anyone may give a buffer of their own.
    pub(in crate::app) explain: Option<BufferId>,
    /// The sessions being started by another session's hand-off, by key: their parent.
    parents: HashMap<SessionKey, SessionId>,
    /// How sessions being started by an agent's tool start, with the brief of one that starts
    /// without asking (ADR 0135).
    origins: HashMap<SessionKey, (Origin, Option<Arc<str>>)>,
    /// The questions asked as another session runs in a start's directory (ADR 0121).
    shared: shared::Shared,
    /// Worktree sessions: briefs, the jobs, `*Worktrees*` and a removal (ADR 0121 step 4).
    pub(in crate::app) worktrees: super::worktrees::Worktrees,
    /// The trust each session started under, for its agent's worktree tools (step 6), until
    /// it is forgotten.
    pub(in crate::app) trusts: HashMap<SessionKey, TrustedRoot>,
    /// The agents' session tools (ADR 0121 step 6).
    pub(in crate::app) tools: super::tools::Tools,
    /// Sessions started outside oso that move into a space (ADR 0151).
    pub(in crate::app) moves: Moves,
    /// A rename asked, and what Claude Code calls oso's running sessions (ADR 0155).
    pub(in crate::app) naming: naming::Naming,
    /// The native sessions (ADR 0165).
    pub(in crate::app) natives: NativeSessions,
}

impl Hosting {
    /// How many sessions `parent` started without asking that are being got ready, before the
    /// table has them (ADR 0135).
    pub(in crate::app) fn readying_unasked(&self, parent: &SessionId) -> usize {
        self.origins
            .iter()
            .filter(|(key, (origin, _))| {
                *origin == Origin::Unasked && self.parents.get(key) == Some(parent)
            })
            .count()
    }

    /// Whether start `key` is under way: ordered, held or not, until its start thread reports.
    pub(in crate::app) fn start_under_way(&self, key: SessionKey) -> bool {
        self.starting.contains_key(&key)
    }

    /// The buffer start `key`, under way, starts in.
    pub(in crate::app) fn start_buffer(&self, key: SessionKey) -> Option<BufferId> {
        self.starting.get(&key).copied()
    }

    /// The hosted sessions.
    pub(in crate::app) fn table(&self) -> &Table {
        &self.table
    }

    /// oso's own state directory, where reviews make their index copies.
    pub(in crate::app) fn state_directory(&self) -> Option<PathBuf> {
        self.directory.clone()
    }

    /// Sessions remembered in `directory`, oso's state directory.
    pub(in crate::app) fn new(directory: Option<PathBuf>) -> Self {
        Self {
            table: Table::default(),
            directory,
            starting: HashMap::new(),
            baselines: HashMap::new(),
            native_trusts: HashMap::new(),
            unsaved: false,
            fork_offered: Vec::new(),
            adopting: Adopting::default(),
            searching: super::conversations::Searching::default(),
            spaces: super::spaces::SpaceWork::default(),
            explain: None,
            parents: HashMap::new(),
            origins: HashMap::new(),
            shared: shared::Shared::default(),
            worktrees: super::worktrees::Worktrees::default(),
            trusts: HashMap::new(),
            tools: super::tools::Tools::default(),
            moves: Moves::default(),
            naming: naming::Naming::default(),
            natives: NativeSessions::default(),
        }
    }
}

/// The trusted project a new session takes a baseline of, and the root its baseline thread
/// runs git in. The root is revoked with the project's trust.
#[derive(Debug)]
struct Baseline {
    project: ProjectId,
    root: TrustedRoot,
    /// The project's review takes the baseline as its own: not an adopted session's, whose
    /// changes before oso hosted it stay in the review.
    to_review: bool,
}

/// Now, in ms since the epoch, for a session's last change.
pub(in crate::app) fn now_ms() -> u64 {
    crate::sessions::spaces::stored::now_ms()
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
    baselines: &mut HashMap<SessionKey, Baseline>,
    trusted: impl Fn(ProjectId) -> bool,
) {
    baselines.retain(|_, baseline| {
        let kept = trusted(baseline.project);
        if !kept {
            baseline.root.revoke();
        }
        kept
    });
}

impl App {
    /// The sessions an earlier oso remembered: each that is not archived comes back stopped,
    /// with nothing started. Those a worktree removal left behind are archived first.
    pub(in crate::app) fn sessions_recalled(
        &mut self,
        recalled: crate::sessions::worker::Recalled,
    ) {
        let crate::sessions::worker::Recalled {
            sessions: mut remembered,
            spaces,
            migrated,
            readable,
            fresh,
            stranded,
        } = recalled;
        // Spaces made from the sessions are kept at once, with the sessions' spaces, so that
        // they are made once; with none made, nothing is written.
        let made = migrated && !spaces.is_empty();
        self.watch.spaces_mut().recall(spaces, fresh, made);
        self.watch.spaces_mut().set_readable(readable);
        let archived = self.archive_stranded(&mut remembered, &stranded);
        let back = self.hosting.table.recall(remembered);
        self.hosting.unsaved |= made || archived > 0;
        self.prune_sessions();
        for key in back {
            if self.hosting.table.get(key).is_some() {
                self.bring_back_stopped(key);
            }
        }
    }

    /// Notes that the sessions changed: [`App::save_sessions`] hands them to the worker once
    /// for every change an event made.
    /// The views of sessions are made again too (ADR 0121, step 3).
    pub(in crate::app) fn remember_sessions(&mut self) {
        self.hosting.unsaved = true;
        self.sessions_touched();
    }

    /// `claude`: a new session in `directory`, else the selected buffer's project root, else
    /// its file's directory, as a new terminal, else the directory oso started in, named
    /// `name`, else after its directory. It turns the agent channel on if it is off (Q20).
    pub(in crate::app) fn start_session(
        &mut self,
        name: Option<String>,
        directory: Option<PathBuf>,
    ) {
        let begin = Begin {
            name,
            directory,
            check_shared: true,
            ..Begin::default()
        };
        self.begin_session(begin, Then::Nothing);
    }

    /// Starts a new session as `begin` says in `directory`, whole, and returns the key it
    /// starts under, unless it was refused at once or waits for the person's answer.
    fn begin_in(&mut self, begin: Begin, directory: PathBuf) -> Option<SessionKey> {
        if begin.check_shared && self.hosting.table.runs_in(&directory) {
            // Where the person was when they pressed the key, though they answer elsewhere.
            let directory = Some(directory);
            self.ask_shared(Begin { directory, ..begin });
            return None;
        }
        let base = begin.name.clone().unwrap_or_else(|| base_name(&directory));
        let (key, name) = self.hosting.table.reserve(&directory, &base);
        // A space's new orchestrator: under its space's gate, never a project's (ADR 0176, O-3).
        let gate = self.new_orchestrator_gate(key, &name, &begin, &directory);
        let project = gate
            .is_none()
            .then(|| self.project_at_root(&directory))
            .flatten();
        let native = self.native_start(&begin, &directory, project, gate.is_some());
        if let Some(parent) = begin.parent.clone() {
            self.hosting.parents.insert(key, parent);
        }
        if begin.origin != Origin::Person {
            // Empty for a start without asking that had no brief.
            let brief = (begin.origin == Origin::Unasked)
                .then(|| Arc::from(begin.brief.as_deref().unwrap_or_default()));
            self.hosting.origins.insert(key, (begin.origin, brief));
        }
        if begin.check_shared {
            let directory = Some(directory.clone());
            let asked = Begin {
                directory,
                ..begin.clone()
            };
            self.hosting.shared.under_way(key, asked);
        }
        self.turn_agent_channel_on_for_a_session();
        // An agent's tool's start stays behind what the person is doing (ADR 0144).
        let shown = begin.origin == Origin::Person && !begin.behind;
        let mut brief = begin.brief_to_send();
        let runs = if native {
            let from = (self.hosting.table).brief_from(begin.origin, begin.parent.as_ref());
            let at = directory.clone();
            let order =
                self.native_start_buffer(key, &name, at, shown, &mut brief, from.as_deref());
            Runs::Native(NativeOrder {
                gate: gate.unwrap_or_default(),
                ..order
            })
        } else {
            Runs::Terminal(self.session_terminal(key, &name, None, shown))
        };
        let (trusted, root) = self.start_trust(key, &begin, &directory, project, native);
        if let Some(trusted) = &trusted {
            self.hosting.trusts.insert(key, trusted.clone());
        }
        // A start in a space's directory passes its checks (ADR 0136).
        let space = begin.space.or_else(|| self.space_checks(&directory));
        self.note_reads(key, space.as_ref());
        self.order_session(Order {
            token: key,
            kind: Kind::New,
            name,
            directory,
            project: root,
            trusted,
            baseline: true,
            state: super::super::reviews::state(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            runs,
            brief,
            steer_worktrees: self.configuration.steer_worktrees(),
            check_shared: begin.check_shared,
            space,
        });
        Some(key)
    }

    /// Where `claude` starts with no directory given: in `*Spaces*`, where the row at point
    /// is (ADR 0131, step 2b), and none on a row with no place, which the echo area says; else
    /// [`App::buffer_directory`], else the directory oso started in.
    fn session_directory_here(&mut self) -> Option<PathBuf> {
        match self.sessions_row_place() {
            Some(Ok(place)) => {
                self.enter_row_space(&place);
                Some(place.directory)
            }
            Some(Err(why)) => {
                self.editor.message(why);
                None
            }
            None => Some(
                self.buffer_directory()
                    .unwrap_or_else(|| self.editor.start_directory().to_path_buf()),
            ),
        }
    }

    /// The project whose root `directory` is: the selected buffer's, or in `*Spaces*` any
    /// buffer's, as no buffer there is in one.
    fn project_at_root(&self, directory: &Path) -> Option<ProjectId> {
        let at_root = |id: &ProjectId| {
            self.editor
                .project(*id)
                .is_some_and(|project| project.root() == directory)
        };
        let current = self.editor.current().1;
        current.project().filter(at_root).or_else(|| {
            (current.mode() == Mode::Sessions)
                .then(|| {
                    self.editor
                        .buffers()
                        .filter_map(|(_, buffer)| buffer.project())
                        .find(at_root)
                })
                .flatten()
        })
    }

    /// What a session's start thread found.
    pub(in crate::app) fn session_start_reported(&mut self, report: Report) {
        if let Report::Ready(ready) = &report {
            self.native_start_reported(ready.token);
            self.switch_start_reported(ready.token, true);
        } else if let Report::Refused { token, .. } | Report::Live { token, .. } = &report {
            self.native_start_reported(*token);
            self.switch_start_reported(*token, false);
        }
        match report {
            Report::Ready(ready) => {
                let key = ready.token;
                self.hosting.shared.take_under_way(key);
                self.session_ready(*ready);
                if let Some(adopted) = self.take_adoption(key)
                    && self.hosting.table.get(key).is_some()
                {
                    self.session_adopted(&adopted);
                }
            }
            Report::Refused { token, why } => {
                self.take_adoption(token);
                self.session_not_started(token);
                self.hosting.parents.remove(&token);
                self.hosting.origins.remove(&token);
                self.session_tool_started(token, Err(why.clone()));
                self.editor.message(why);
            }
            Report::Live {
                token,
                pid,
                directory,
            } => {
                let adopted = self.take_adoption(token);
                self.session_not_started(token);
                // An agent's resume is refused: only the person's word forks a conversation.
                if !self.session_tool_live(token, pid, directory.as_deref()) {
                    self.offer_fork(token, adopted, pid, directory.as_deref());
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
                self.hosting.origins.remove(&token);
                if let Some(begin) = self.hosting.shared.take_under_way(token) {
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
        if let Some(baseline) = self.hosting.baselines.remove(&key)
            && baseline.to_review
        {
            self.session_baseline_taken(baseline.project, tree);
        }
    }

    /// Revokes the root of each start baseline whose project is no longer trusted, which
    /// stops its git run between requests, and forgets it.
    pub(in crate::app) fn revoke_untrusted_session_baselines(&mut self) {
        let editor = &self.editor;
        revoke_untrusted(&mut self.hosting.baselines, |project| {
            editor.project_trusted(project)
        });
        self.revoke_untrusted_native_starts();
        // Those in spaces' directories, by their spaces (ADR 0176).
        self.revoke_space_gates();
    }

    /// The trust and root new session `key` starts under in `directory`, natively if
    /// `native`: a native start's as `native_start` judged it, a worktree's just made, an
    /// agent's terminal start under its session's, else an open trusted project's, whose
    /// baseline it takes.
    fn start_trust(
        &mut self,
        key: SessionKey,
        begin: &Begin,
        directory: &Path,
        project: Option<ProjectId>,
        native: bool,
    ) -> (Option<TrustedRoot>, Option<PathBuf>) {
        match begin.trusted.clone() {
            // A native start under an agent's trust runs under the one `native_start` judged
            // where it runs, which the start thread looks at again (ADR 0165, Q5).
            Some(_) if native && !begin.worktree => {
                let judged = self.native_trust_now(directory, Some(directory)).ok();
                (judged, Some(directory.to_path_buf()))
            }
            // A worktree just made, which no buffer opened yet: its own root and trust; or an
            // agent's start in a terminal, under its session's trust.
            Some(trusted) => (Some(trusted), Some(directory.to_path_buf())),
            None => {
                let trusted = project.and_then(|id| TrustedRoot::of(&self.editor, id));
                if let (Some(project), Some(root)) = (project, &trusted) {
                    let baseline = Baseline {
                        project,
                        root: root.clone(),
                        to_review: true,
                    };
                    self.hosting.baselines.insert(key, baseline);
                }
                let root = project
                    .and_then(|id| self.editor.project(id))
                    .map(|project| project.root().to_path_buf());
                (trusted, root)
            }
        }
    }

    /// Forgets session `key`, which started and was written in the record but leaves nothing
    /// to keep: the table, its trust, the worker and the record's line (ADR 0165, step 2).
    pub(super) fn forget_started_session(&mut self, key: SessionKey) {
        self.hosting.starting.remove(&key);
        self.hosting.table.drop_started(key);
        self.hosting.trusts.remove(&key);
        if let Some(worker) = &self.sessions {
            worker.send(crate::sessions::worker::Job::Forget(key));
        }
        self.remember_sessions();
    }

    /// A start that did not happen: a held name goes, and a resumed session is stopped again.
    /// The agent channel goes off again if this start turned it on and nothing else needs it.
    fn session_not_started(&mut self, token: SessionKey) {
        self.forget_agent_resume(token);
        self.hosting.shared.take_under_way(token);
        if self.hosting.natives.has(token) {
            self.session_native_not_started(token);
        } else {
            self.session_terminal_not_started(token);
        }
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
            // Only an agent's tool puts an origin here; a start of the person's has none.
            let (origin, unasked_brief) =
                (self.hosting.origins.remove(&key)).unwrap_or((Origin::Person, None));
            let added = self.hosting.table.add(
                key,
                New {
                    id: ready.id.clone(),
                    root: ready.project.clone(),
                    parent: self.hosting.parents.remove(&key),
                    origin,
                    unasked_brief,
                    changed: now_ms(),
                    native: matches!(ready.started, start::Started::Native(_)),
                },
            );
            if added.is_none() {
                return;
            }
            self.space_session_started(key);
            self.prune_sessions();
        }
        if let Some(tree) = &ready.baseline {
            self.session_baseline_to_review(key, tree);
        }
        // An orchestrator's repository left out, or its instructions not read (ADR 0136).
        for note in &ready.notes {
            self.editor.message(super::spaces::printable(note));
        }
        if let Some(session) = self.hosting.table.get_mut(key) {
            if ready.baseline.is_some() {
                session.baseline.clone_from(&ready.baseline);
            }
            session.changed = now_ms();
        }
        let started = match ready.started {
            start::Started::Native(_) => self.session_native_started(key, ready),
            start::Started::Terminal(_) => self.session_terminal_started(key, ready),
        };
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
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{Baseline, revoke_untrusted};
    use crate::git::TrustedRoot;

    #[test]
    fn a_start_baseline_of_a_project_no_longer_trusted_is_revoked_and_forgotten() {
        let (_editor, project) = crate::commands::review::tests::in_project(true);
        let root = TrustedRoot::for_tests(std::env::temp_dir());
        let baseline = Baseline {
            project,
            root: root.clone(),
            to_review: true,
        };
        let mut baselines = HashMap::from([(1, baseline)]);
        revoke_untrusted(&mut baselines, |_| true);
        assert!(!root.revoked());
        assert_eq!(baselines.len(), 1);
        revoke_untrusted(&mut baselines, |_| false);
        assert!(root.revoked(), "its git run stops between requests");
        assert!(baselines.is_empty());
    }
}
