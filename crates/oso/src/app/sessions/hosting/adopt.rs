//! `session-adopt` in the app (ADR 0121, "Adopting a session started outside oso"): keeping the
//! list it completes over, made on a thread of its own when the person may want it, and
//! resuming the conversation picked through the same start thread as `session-resume`, whose
//! check refuses a live one and offers a fork.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - An answer is looked up in the list its prompt showed: a list made while the prompt is
//!   open waits until it closes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use oso_core::project::ProjectId;

use super::fork::ForkFrom;
use super::{Baseline, base_name};
use crate::app::{App, UserEvent};
use crate::commands::agent_sessions::{ADOPT_LIST, is_adopt_prompt};
use crate::review::TrustedRoot;
use crate::sessions::adopt::{self, Found, Offered, Order as ListOrder, RegistryNames};
use crate::sessions::identity::cleaned;
use crate::sessions::registry::{is_plain_directory, shown_directory};
use crate::sessions::start::{Kind, Order};
use crate::sessions::transcripts::Cache;
use crate::sessions::worker::SessionKey;
use crate::sessions::worker::externals::External;

/// The least time between two lists made because an oso window came to the front.
const FOCUS_EVERY: Duration = Duration::from_secs(5);

/// What the app keeps for `session-adopt`.
#[derive(Debug)]
pub(in crate::app) struct Adopting {
    /// What earlier lists read of the transcripts; `None` while a list is being made.
    cache: Option<Cache>,
    /// Another list was asked for while one was being made.
    again: bool,
    /// When a list was last asked for because a window came to the front.
    focused: Option<Instant>,
    /// The record of oso's own sessions was read, or there is none: a list made before would
    /// read their transcripts for nothing.
    pub(in crate::app) recalled: bool,
    /// The registry's names this oso saw.
    names: RegistryNames,
    /// What the editor's list stands for, and a list made while a `session-adopt` prompt was
    /// open, given to the editor once it closes.
    offered: Offered,
    /// The conversations being adopted, by their key, until the start thread reports.
    pending: HashMap<SessionKey, Adoption>,
}

impl Default for Adopting {
    fn default() -> Self {
        Self {
            cache: Some(Cache::default()),
            again: false,
            focused: None,
            recalled: false,
            names: RegistryNames::default(),
            offered: Offered::default(),
            pending: HashMap::new(),
        }
    }
}

/// A conversation being adopted.
#[derive(Debug)]
pub(super) struct Adoption {
    /// What a fork copies, if its start is refused as it runs elsewhere.
    pub(super) from: ForkFrom,
    /// The registry said it was busy, the last time this oso saw it.
    busy: bool,
}

/// The name an adopted session asks for: its own, if any of it is left once cleaned, else its
/// directory's.
fn base_of(name: Option<&str>, directory: &Path) -> String {
    name.and_then(cleaned)
        .unwrap_or_else(|| base_name(directory))
}

impl App {
    /// Makes the list `session-adopt` completes over again, on a thread of its own, or once
    /// more after the one being made.
    pub(in crate::app) fn list_adoptable(&mut self) {
        let Some(cache) = self.hosting.adopting.cache.take() else {
            self.hosting.adopting.again = true;
            return;
        };
        let table = self.hosting.table();
        let left_out = table
            .iter()
            .map(|session| session.id.clone())
            .chain(table.known().iter().cloned())
            .collect();
        let order = ListOrder {
            cache,
            left_out,
            now: SystemTime::now(),
        };
        let waker = std::sync::Mutex::new(self.waker.clone());
        let spawned = adopt::spawn(order, move |found| {
            waker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .send(UserEvent::Adoptable(Box::new(found)));
        });
        if let Err(e) = spawned {
            log::warn!("the sessions to adopt cannot be listed: {e}");
            self.hosting.adopting.cache = Some(Cache::default());
        }
    }

    /// An oso window came to the front: the person may have stopped a session elsewhere.
    pub(in crate::app) fn list_adoptable_on_focus(&mut self) {
        let now = Instant::now();
        let recalled = self.hosting.adopting.recalled || self.hosting.state_directory().is_none();
        let adopting = &mut self.hosting.adopting;
        if !recalled {
            return;
        }
        if adopting
            .focused
            .is_some_and(|last| now.duration_since(last) < FOCUS_EVERY)
        {
            return;
        }
        adopting.focused = Some(now);
        self.list_adoptable();
    }

    /// The registry's live sessions as step 3's poll found them: their names are kept, and a
    /// list is made again when one of them ended, as it may be adopted now.
    pub(in crate::app) fn adoptable_externals(&mut self, before: &[u32], live: &[External]) {
        for external in live {
            if let Some(id) = &external.session_id {
                self.hosting
                    .adopting
                    .names
                    .note(id, external.name.as_deref(), external.working);
            }
        }
        if before
            .iter()
            .any(|pid| !live.iter().any(|external| external.pid == *pid))
        {
            self.list_adoptable();
        }
    }

    /// The list thread found the conversations oso may adopt. While a `session-adopt` prompt
    /// is open, the editor keeps the list it shows, and the answer is looked up in that one.
    pub(in crate::app) fn adoptable_found(&mut self, found: Found) {
        let adopting = &mut self.hosting.adopting;
        for running in &found.running {
            adopting
                .names
                .note(&running.id, running.name.as_deref(), running.busy);
        }
        let choices = adopt::choices(&found, &adopting.names, SystemTime::now());
        adopting.cache = Some(found.cache);
        let shown = self.editor.prompts().iter().any(is_adopt_prompt);
        if let Some(candidates) = self.hosting.adopting.offered.list(choices, shown) {
            self.editor.set_listed(ADOPT_LIST, candidates);
        }
        if std::mem::take(&mut self.hosting.adopting.again) {
            self.list_adoptable();
        }
    }

    /// After the requests of an event: a list made while a `session-adopt` prompt was open is
    /// given to the editor once no such prompt is.
    pub(in crate::app) fn show_waiting_adoptable(&mut self) {
        if self.hosting.adopting.offered.waiting()
            && !self.editor.prompts().iter().any(is_adopt_prompt)
            && let Some(candidates) = self.hosting.adopting.offered.unshown()
        {
            self.editor.set_listed(ADOPT_LIST, candidates);
        }
    }

    /// `session-adopt`: resumes the conversation the candidate `text` stands for, in a new
    /// buffer.
    pub(in crate::app) fn adopt_session(&mut self, text: &str) {
        // Looked up in the list the prompt showed, before a list made since replaces it.
        let pick = self.hosting.adopting.offered.pick(text).cloned();
        self.show_waiting_adoptable();
        let Some(pick) = pick else {
            self.editor
                .message("No such session to adopt now: run session-adopt again");
            self.list_adoptable();
            return;
        };
        if let Some(session) = self
            .hosting
            .table()
            .iter()
            .find(|session| session.id == pick.id)
        {
            let (name, buffer) = (session.name.clone(), session.buffer);
            if let Some(buffer) = buffer {
                self.editor.show(buffer);
            }
            self.editor
                .message(format!("Session {name} is oso's already"));
            return;
        }
        let Some(directory) = pick.directory.clone().filter(|dir| is_plain_directory(dir)) else {
            self.editor
                .message("oso cannot tell where that session ran, so it cannot resume it");
            return;
        };
        let base = base_of(pick.name.as_deref(), &directory);
        let (key, name) = self.hosting.table.reserve(&directory, &base);
        // The open project that holds the directory: the session is placed in its space, and
        // `claude` is never looked for on `PATH` inside it.
        let project = self.project_holding(&directory);
        let root = project
            .and_then(|id| self.editor.project(id))
            .map(|project| project.root().to_path_buf())
            .unwrap_or_else(|| directory.clone());
        // Its own baseline, for ready for review, only at a trusted project's root: what it
        // changed before oso hosted it stays in the project's review.
        let trusted = project
            .filter(|_| root == directory)
            .and_then(|id| Some((id, TrustedRoot::of(&self.editor, id)?)));
        if let Some((project, root)) = &trusted {
            let baseline = Baseline {
                project: *project,
                root: root.clone(),
                to_review: false,
            };
            self.hosting.baselines.insert(key, baseline);
        }
        let adoption = Adoption {
            from: ForkFrom {
                id: pick.id.clone(),
                directory: directory.clone(),
                root: Some(root.clone()),
                name: name.clone(),
            },
            busy: pick.busy,
        };
        self.hosting.adopting.pending.insert(key, adoption);
        self.turn_agent_channel_on_for_a_session();
        let terminal = self.session_terminal(key, &name, None);
        self.order_session(Order {
            token: key,
            kind: Kind::Resume(pick.id),
            name,
            directory,
            project: Some(root),
            trusted: trusted.map(|(_, root)| root),
            state: crate::app::reviews::state(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            terminal,
            brief: None,
            steer_worktrees: self.configuration.steer_worktrees(),
            check_shared: false,
        });
    }

    /// The deepest open project whose root holds `directory`.
    fn project_holding(&self, directory: &Path) -> Option<ProjectId> {
        let mut seen = HashSet::new();
        let roots: Vec<(ProjectId, PathBuf)> = self
            .editor
            .buffers()
            .filter_map(|(_, buffer)| buffer.project())
            .filter(|id| seen.insert(*id))
            .filter_map(|id| Some((id, self.editor.project(id)?.root().to_path_buf())))
            .collect();
        adopt::holding(
            directory,
            roots.iter().map(|(id, root)| (*id, root.as_path())),
        )
    }

    /// The adoption started with `key`, if it is one, as its start was refused or it is
    /// ready: what it forks if offered, and what the echo area says.
    pub(super) fn take_adoption(&mut self, key: SessionKey) -> Option<Adoption> {
        self.hosting.adopting.pending.remove(&key)
    }

    /// The adopted session `adopted` started: the echo area says so, and the list leaves it
    /// out.
    pub(super) fn session_adopted(&mut self, adopted: &Adoption) {
        let name = &adopted.from.name;
        let said = if adopted.busy {
            format!(
                "Adopted {name}: it was working when it stopped, so its last turn may be cut \
                 short; ask it to go on"
            )
        } else {
            format!(
                "Adopted {name} from {}",
                shown_directory(&adopted.from.directory)
            )
        };
        self.editor.message(said);
        self.list_adoptable();
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::base_of;

    #[test]
    fn an_adopted_session_is_named_after_itself_else_its_directory() {
        let dir = Path::new("/p/oso");
        assert_eq!(base_of(Some("fix-tests"), dir), "fix-tests");
        assert_eq!(base_of(Some("fix tests!"), dir), "fix-tests");
        assert_eq!(base_of(Some("保険の手続き"), dir), "oso");
        assert_eq!(base_of(Some("---"), dir), "oso");
        assert_eq!(base_of(Some("..."), dir), "oso");
        assert_eq!(base_of(None, dir), "oso");
    }
}
