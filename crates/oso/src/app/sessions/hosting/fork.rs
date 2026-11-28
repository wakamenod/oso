//! `session-fork` in the app (ADR 0121): a hosted session's conversation copied under a new id,
//! and the "fork it instead?" question a refused resume or adoption asks.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - Each question is kept with the conversation it is about, by its prompt: with two stacked,
//!   a `y` forks the one whose question it closed, never the selected buffer's.

use std::path::{Path, PathBuf};

use oso_core::buffer::BufferId;

use super::adopt::Adoption;
use crate::app::App;
use crate::git::TrustedRoot;
use crate::sessions::hosted::Hosted;
use crate::sessions::identity::SessionId;
use crate::sessions::registry::shown_directory;
use crate::sessions::start::{Kind, NativeOrder, Order, Runs};
use crate::sessions::worker::SessionKey;

/// The conversation a fork copies: a hosted session's, or one being adopted, whose name is only
/// held while its start thread runs.
#[derive(Debug, Clone)]
pub(super) struct ForkFrom {
    pub(super) id: SessionId,
    pub(super) directory: PathBuf,
    pub(super) root: Option<PathBuf>,
    pub(super) name: String,
}

impl ForkFrom {
    fn of(session: &Hosted) -> Self {
        Self {
            id: session.id.clone(),
            directory: session.directory.clone(),
            root: session.root.clone(),
            name: session.name.clone(),
        }
    }
}

impl App {
    /// `session-fork`: the conversation of the session whose terminal is `buffer`, copied
    /// under a new id, in a new buffer; or, as the `y` of a refused resume's question, that
    /// session's.
    pub(in crate::app) fn fork_session(&mut self, buffer: BufferId) {
        let from = match self.answered_fork_offer() {
            Some(from) => from,
            None => match self.hosting.table.by_buffer(buffer) {
                Some(session) => ForkFrom::of(session),
                None => {
                    self.editor.message("This buffer holds no session");
                    return;
                }
            },
        };
        // A conversation that moved to an id oso does not follow forks from before it
        // (ADR 0165, step 2d).
        let moved = (self.hosting.table.iter())
            .find(|session| session.id == from.id)
            .is_some_and(|session| self.hosting.natives.moved(session.key));
        if moved {
            return self.editor.message(
                "This conversation moved to a session id oso does not follow: a fork would copy \
                 the conversation before, so start a new session",
            );
        }
        self.fork_from(from);
    }

    /// Forks the conversation of `id` that `session-search` found (ADR 0145, step 2): a hosted
    /// session's as `session-fork` forks it, else in `directory`, where its transcript is found,
    /// placed by the open project that holds it, and named after `name` or the directory.
    pub(in crate::app) fn fork_conversation(
        &mut self,
        id: SessionId,
        directory: PathBuf,
        name: Option<&str>,
    ) {
        let from = match self.hosting.table.iter().find(|session| session.id == id) {
            Some(session) => ForkFrom::of(session),
            None => {
                let root = self
                    .project_holding(&directory)
                    .and_then(|project| self.editor.project(project))
                    .map(|project| project.root().to_path_buf());
                ForkFrom {
                    id,
                    name: super::adopt::base_of(name, &directory),
                    directory,
                    root,
                }
            }
        };
        // A conversation that moved to an id oso does not follow forks from before it
        // (ADR 0165, step 2d).
        let moved = (self.hosting.table.iter())
            .find(|session| session.id == from.id)
            .is_some_and(|session| self.hosting.natives.moved(session.key));
        if moved {
            return self.editor.message(
                "This conversation moved to a session id oso does not follow: a fork would copy \
                 the conversation before, so start a new session",
            );
        }
        self.fork_from(from);
    }

    /// Starts a new hosted session that resumes `from`'s conversation under a new id: native
    /// as a new start would be (ADR 0165, step 2), else in a terminal. Whole, once the
    /// configuration's first look is done, as `[sessions] mode` chooses which (ADR 0173).
    fn fork_from(&mut self, from: ForkFrom) {
        if self.starts_wait() || self.native_waits_for_trust(&from.directory, from.root.as_deref())
        {
            return self.hold_start(move |app| app.fork_from(from));
        }
        let ForkFrom {
            id: from,
            directory,
            root,
            name: base,
        } = from;
        // A connected session is turned off before a fork of it, which is not connected (ADR
        // 0181); unless Claude Code's settings connect the fork too (ADR 0188).
        if !self.remote_at_startup_on() {
            self.remote_off_for_fork(&from);
        }
        let (key, name) = self.hosting.table.reserve(&directory, &base);
        self.note_fork(&from, key);
        // A fork in a space's directory passes its checks (ADR 0136).
        let space = self.space_checks(&directory);
        self.turn_agent_channel_on_for_a_session();
        // Native in a trusted project unless `mode = "tui"`, as a new start (ADR 0165), in
        // the permission mode, model and effort of the native session it copies (parts 3b and
        // 3c).
        let source = (self.hosting.table.iter()).find(|session| session.id == from);
        let forked = source.and_then(|session| self.hosting.natives.forked(session.key));
        // It starts with its source's own model too, and keeps it as its own (ADR 0121, "A
        // start's model, as amended").
        let model = source.and_then(|session| session.model.clone());
        if let Some(model) = &model {
            self.hosting.models.insert(key, model.clone());
        }
        // It shows the turns it copies (ADR 0165, step 5).
        let native = self
            .start_native_fork(key, &from, &name, &directory, root.as_deref(), forked)
            .map(|(order, gate)| (self.native_fork_order(key, &from, &base, order), gate));
        let (runs, _, trusted) = self.native_or_terminal(key, &name, true, native);
        self.order_session(Order {
            token: key,
            kind: Kind::Fork(from),
            name,
            directory,
            project: root,
            // Its trust, for a native fork; a fork takes no baseline.
            trusted,
            baseline: false,
            state: crate::app::reviews::state(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            runs,
            brief: None,
            steer_worktrees: self.configuration.steer_worktrees(),
            check_shared: false,
            space,
            model,
        });
    }

    /// What new session `key`, `name`, a fork or an adoption, runs in, shown if `shown`, with
    /// its buffer and its gate: `native`'s start, whose gate its tools take too, else a
    /// terminal.
    pub(super) fn native_or_terminal(
        &mut self,
        key: SessionKey,
        name: &str,
        shown: bool,
        native: Option<(NativeOrder, Option<TrustedRoot>)>,
    ) -> (Runs, BufferId, Option<TrustedRoot>) {
        if let Some((order, gate)) = native {
            if let Some(gate) = &gate {
                self.hosting.trusts.insert(key, gate.clone());
            }
            let buffer = order.buffer;
            return (Runs::Native(order), buffer, gate);
        }
        let terminal = self.session_terminal(key, name, None, shown);
        let buffer = terminal.buffer;
        (Runs::Terminal(terminal), buffer, None)
    }

    /// The start `token` was refused as its conversation may run elsewhere, as process `pid`
    /// in `directory` if the registry says: the person is asked whether to fork it instead.
    pub(super) fn offer_fork(
        &mut self,
        token: SessionKey,
        adopted: Option<Adoption>,
        pid: Option<u32>,
        directory: Option<&Path>,
    ) {
        let adopting = adopted.is_some();
        let from = adopted
            .map(|adopted| adopted.from)
            .or_else(|| self.hosting.table.get(token).map(ForkFrom::of));
        let Some(from) = from else {
            return;
        };
        let name = &from.name;
        let place = directory
            .map(|directory| format!(" in {}", shown_directory(directory)))
            .unwrap_or_default();
        let question = match pid {
            // A live process cannot move into oso: it stops where it runs first.
            Some(pid) if adopting => format!(
                "Session {name} runs as process {pid}{place}: stop it there and adopt it again, \
                 or fork it instead?"
            ),
            Some(pid) => {
                format!("Session {name} runs already as process {pid}{place}; fork it instead?")
            }
            None => format!(
                "Session {name} may run already: oso cannot tell from Claude Code's registry; fork it instead?"
            ),
        };
        match self.editor.ask(question, "session-fork") {
            Ok(asked) => self.hosting.fork_offered.push((asked, from)),
            Err(e) => self.editor.message(e.to_string()),
        }
    }

    /// The conversation of the refused start whose question just closed, once: the question
    /// was answered `y` if `session-fork` runs now. Offers whose questions are still open stay.
    fn answered_fork_offer(&mut self) -> Option<ForkFrom> {
        let open: Vec<_> = self.editor.prompts().iter().map(|p| p.id()).collect();
        let mut closed = None;
        self.hosting.fork_offered.retain(|(asked, from)| {
            let kept = open.contains(asked);
            if !kept {
                closed = Some(from.clone());
            }
            kept
        });
        closed
    }

    /// After the requests of an event: a fork offer whose question closed without a `y` goes,
    /// so that a later `session-fork` forks the selected buffer's session.
    pub(in crate::app) fn drop_unanswered_fork_offer(&mut self) {
        if !self.hosting.fork_offered.is_empty() {
            let _ = self.answered_fork_offer();
        }
    }
}
