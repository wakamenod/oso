//! What a new hosted session starts with (ADR 0121): its name, directory, brief, parent, how
//! it starts, whether to ask about a shared directory, its trust, and for an issue space's
//! orchestrator what it is given (ADR 0136, step 3). And starting it: its directory is found
//! as the person presses the key, and the rest waits for the configuration's first look
//! (ADR 0173); what its caller does with its key follows (`Then`).
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::path::PathBuf;

use crate::app::App;
use crate::app::sessions::tools::Pending;
use crate::git::TrustedRoot;
use crate::sessions::identity::{Origin, SessionId};
use crate::sessions::spaces::SpaceId;
use crate::sessions::worker::SessionKey;

/// What a new session starts with. Its default is the person's start, by a key.
#[derive(Debug, Clone)]
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
    /// How it starts: by the person, unless an agent's tool starts it (ADR 0135). One an agent's
    /// tool starts, asked or not, starts behind what the person is doing: its terminal is not
    /// shown (ADR 0144).
    pub origin: Origin,
    /// Whether to ask first when another live session runs in its directory.
    pub check_shared: bool,
    /// The trust it starts under, when its directory is no open project's root: a worktree
    /// just made (`worktree`), or the trust of the agent's session that starts it.
    pub trusted: Option<TrustedRoot>,
    /// Its directory is a worktree just made, whose own trust `trusted` is: the one start
    /// that runs natively with no open buffer to say its project is trusted (ADR 0165, Q5).
    pub worktree: bool,
    /// An issue space's orchestrator: what it starts with (ADR 0136, step 3). A worker's
    /// reads ride here too.
    pub space: Option<crate::sessions::start::SpaceStart>,
    /// The space it is the new orchestrator of: only `launch_orchestrator` says so. It runs
    /// natively there under that space's gate (ADR 0176, part O-3).
    pub orchestrator_of: Option<SpaceId>,
    /// Its terminal is not shown though the person started it: an orchestrator `space-new`
    /// started once the person had left its space (ADR 0159).
    pub behind: bool,
    /// The model an agent's tool named, its own `--model` from now on (ADR 0121, "A start's
    /// model, as amended"); `None` for the person's settings' own.
    pub model: Option<String>,
    /// How an agent's tool asked it to run (ADR 0121, "A start's mode, as amended"); `None`
    /// as `[sessions] mode` says.
    pub mode: Option<crate::channel::protocol::sessions::RunMode>,
}

impl Default for Begin {
    fn default() -> Self {
        Self {
            name: None,
            directory: None,
            brief: None,
            parent: None,
            origin: Origin::Person,
            check_shared: false,
            trusted: None,
            worktree: false,
            space: None,
            orchestrator_of: None,
            behind: false,
            model: None,
            mode: None,
        }
    }
}

impl Begin {
    /// Its brief as the session is given it: an agent's is text oso was given, not the
    /// person's, escaped once, here, on both routes (`sessions::send::for_claude`; ADR 0121,
    /// "An agent's brief, as amended"). The person's own, from `*Brief*`, goes as typed; one
    /// oso composed for the person's start escapes itself (an orchestrator's successor's).
    pub(in crate::app) fn brief_to_send(&self) -> Option<String> {
        let brief = self.brief.as_deref()?;
        Some(match self.origin {
            Origin::Person => brief.to_owned(),
            _ => crate::sessions::send::for_claude(brief, None).into_owned(),
        })
    }
}

/// What the caller of a new session's start does with the key it starts under, or with none.
pub(in crate::app) enum Then {
    /// Nothing more: the echo area says why a start did not happen.
    Nothing,
    /// An agent's tool's call, whose reply waits for the session to start. `said` goes to the
    /// echo area once the start is under way, and `refused` is the reply if it did not start.
    Tool {
        pending: Pending,
        said: Option<String>,
        refused: &'static str,
    },
    /// An issue space's orchestrator: the space it starts for (ADR 0136).
    Orchestrator(SpaceId),
}

impl App {
    /// Starts a new session as `begin` says, then does what `then` says with its key. Its
    /// directory is found now, where the person is; the rest waits for the configuration's
    /// first look, so that `[sessions]` and `[terminal]` are the person's (ADR 0173).
    pub(in crate::app) fn begin_session(&mut self, mut begin: Begin, then: Then) {
        let directory = match begin.directory.clone() {
            Some(directory) if !directory.is_absolute() => {
                self.editor.message("Name the session's directory whole");
                return self.began(None, then);
            }
            Some(directory) => directory,
            None => match self.session_directory_here() {
                Some(directory) => directory,
                None => return self.began(None, then),
            },
        };
        if self.starts_wait() {
            begin.directory = Some(directory);
            // One the person started shows where they are when it goes, unless they moved
            // meanwhile: then it starts behind, so that what they type stays where they are.
            let here = self.selected_place();
            return self.hold_start(move |app| {
                begin.behind |= app.selected_place() != here;
                app.begin_session(begin, then);
            });
        }
        let key = self.begin_in(begin, directory);
        self.began(key, then);
    }

    /// The selected window and the buffer it shows.
    fn selected_place(&self) -> (oso_core::window::WindowId, oso_core::buffer::BufferId) {
        let window = self.editor.current().0;
        (window.id(), window.buffer())
    }

    /// Does what `then` says with `key`, the key a new session starts under, or with none.
    fn began(&mut self, key: Option<SessionKey>, then: Then) {
        match then {
            Then::Nothing => {}
            Then::Tool {
                pending,
                said,
                refused,
            } => self.tool_began(key, pending, said, refused),
            Then::Orchestrator(id) => {
                if let Some(key) = key {
                    self.hosting.spaces.starting.insert(key, id);
                }
            }
        }
    }
}
