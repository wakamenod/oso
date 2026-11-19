//! What a new hosted session starts with (ADR 0121): its name, directory, brief, parent, how
//! it starts, whether to ask about a shared directory, its trust, and for an issue space's
//! orchestrator what it is given (ADR 0136, step 3).
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::path::PathBuf;

use crate::git::TrustedRoot;
use crate::sessions::identity::{Origin, SessionId};

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
    /// The trust of its directory, when it is no open project's: a worktree just made.
    pub trusted: Option<TrustedRoot>,
    /// An issue space's orchestrator: what it starts with (ADR 0136, step 3).
    pub space: Option<crate::sessions::start::SpaceStart>,
    /// Its terminal is not shown though the person started it: an orchestrator `space-new`
    /// started once the person had left its space (ADR 0159).
    pub behind: bool,
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
            space: None,
            behind: false,
        }
    }
}
