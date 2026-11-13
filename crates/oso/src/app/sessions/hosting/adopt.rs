//! `session-adopt` in the app (ADR 0121, "Adopting a session started outside oso"): keeping the
//! list it completes over, made on a thread of its own when the person may want it, and
//! resuming the conversation picked through the same start thread as `session-resume`, whose
//! check refuses a live one and offers a fork.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use oso_core::project::ProjectId;

use super::{ForkFrom, base_name};
use crate::app::{App, UserEvent};
use crate::commands::agent_sessions::ADOPT_LIST;
use crate::review::TrustedRoot;
use crate::sessions::adopt::{self, Found, Order as ListOrder, Pick, Seen};
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
    /// The registry's names this oso saw.
    seen: Seen,
    /// What each candidate of the last list stands for, by its text.
    picks: HashMap<String, Pick>,
    /// The conversations being adopted, by their key, until the start thread reports.
    pending: HashMap<SessionKey, ForkFrom>,
    /// The sessions whose baseline is their own, not their project's review's.
    pub(super) own_baselines: HashSet<SessionKey>,
}

impl Default for Adopting {
    fn default() -> Self {
        Self {
            cache: Some(Cache::default()),
            again: false,
            focused: None,
            seen: Seen::default(),
            picks: HashMap::new(),
            pending: HashMap::new(),
            own_baselines: HashSet::new(),
        }
    }
}

/// The name an adopted session asks for: its own, if any of it is left once cleaned, else its
/// directory's.
fn base_of(name: Option<&str>, directory: &Path) -> String {
    match name {
        Some(name) if name.chars().any(|c| c.is_ascii_alphanumeric()) => name.to_owned(),
        _ => base_name(directory),
    }
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
        let adopting = &mut self.hosting.adopting;
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
            // A registry entry with no name is listed as `claude PID`, which is no name.
            let named = external.name != format!("claude {}", external.pid);
            if let Some(id) = &external.session_id {
                self.hosting.adopting.seen.note(
                    id,
                    named.then_some(external.name.as_str()),
                    external.working,
                );
            }
        }
        if before
            .iter()
            .any(|pid| !live.iter().any(|external| external.pid == *pid))
        {
            self.list_adoptable();
        }
    }

    /// The list thread found the conversations oso may adopt.
    pub(in crate::app) fn adoptable_found(&mut self, found: Found) {
        let adopting = &mut self.hosting.adopting;
        for running in &found.running {
            adopting
                .seen
                .note(&running.id, running.name.as_deref(), running.busy);
        }
        let choices = adopt::choices(&found, &adopting.seen, SystemTime::now());
        adopting.picks = choices.picks;
        adopting.cache = Some(found.cache);
        self.editor.set_listed(ADOPT_LIST, choices.candidates);
        if std::mem::take(&mut self.hosting.adopting.again) {
            self.list_adoptable();
        }
    }

    /// `session-adopt`: resumes the conversation the candidate `text` stands for, in a new
    /// buffer.
    pub(in crate::app) fn adopt_session(&mut self, text: &str) {
        let Some(pick) = self.hosting.adopting.picks.get(text).cloned() else {
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
        let Some(directory) = pick.directory.clone().filter(|dir| dir.is_absolute()) else {
            self.editor
                .message("oso cannot tell where that session ran, so it cannot resume it");
            return;
        };
        let base = base_of(pick.name.as_deref(), &directory);
        let (key, name) = self.hosting.table.reserve(&directory, &base);
        // The open project that holds the directory: the session is placed in its space, and
        // `claude` is never looked for inside it, nor inside the directory itself.
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
        if let Some((id, root)) = &trusted {
            self.hosting.baselines.insert(key, (*id, root.clone()));
            self.hosting.adopting.own_baselines.insert(key);
        }
        self.hosting.adopting.pending.insert(
            key,
            ForkFrom {
                id: pick.id.clone(),
                directory: directory.clone(),
                root: Some(root.clone()),
                name: name.clone(),
                busy: pick.busy,
            },
        );
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
        });
    }

    /// The deepest open project whose root holds `directory`.
    fn project_holding(&self, directory: &Path) -> Option<ProjectId> {
        let mut seen = HashSet::new();
        self.editor
            .buffers()
            .filter_map(|(_, buffer)| buffer.project())
            .filter(|id| seen.insert(*id))
            .filter_map(|id| Some((id, self.editor.project(id)?.root().to_path_buf())))
            .filter(|(_, root)| directory.starts_with(root))
            .max_by_key(|(_, root): &(ProjectId, PathBuf)| root.components().count())
            .map(|(id, _)| id)
    }

    /// The adoption started with `key`, if it is one, as its start was refused or it is
    /// ready: what it forks if offered, and what the echo area says.
    pub(super) fn take_adoption(&mut self, key: SessionKey) -> Option<ForkFrom> {
        self.hosting.adopting.pending.remove(&key)
    }

    /// The adopted session `from` started: the echo area says so, and the list leaves it out.
    pub(super) fn session_adopted(&mut self, from: &ForkFrom) {
        let name = &from.name;
        let said = if from.busy {
            format!(
                "Adopted {name}: it was working when it stopped, so its last turn may be cut \
                 short; ask it to go on"
            )
        } else {
            format!(
                "Adopted {name} from {}",
                super::shown_directory(&from.directory)
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
        assert_eq!(base_of(Some("保険の手続き"), dir), "oso");
        assert_eq!(base_of(Some("---"), dir), "oso");
        assert_eq!(base_of(None, dir), "oso");
    }
}
