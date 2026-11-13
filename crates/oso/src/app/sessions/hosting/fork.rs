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
use crate::sessions::hosted::Hosted;
use crate::sessions::identity::SessionId;
use crate::sessions::registry::shown_directory;
use crate::sessions::start::{Kind, Order};
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
        let ForkFrom {
            id: from,
            directory,
            root,
            name: base,
        } = from;
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
            state: crate::app::reviews::state(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            terminal,
        });
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
