//! When a session wants the person (ADR 0121 step 3): the notices of states, said in the echo
//! area and asked of the system as attention; the marks' blink; the person's look at a
//! session, held 2 s before it counts; and the look for changes after a turn ends, which makes
//! a session ready for review.
//!
//! Must not depend on: how a frame is drawn, key presses, or the worker's thread. Git runs
//! on a review's thread (ADR 0122), never here.

use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;
use oso_core::project::ProjectId;

use crate::app::{App, UserEvent};
use crate::review::{self, Reviewer, TrustedRoot};
use crate::sessions::attention::Seen as Where;
use crate::sessions::state::State;
use crate::sessions::worker::{Job, SessionKey};

/// How long a session must stay in front of the person before it counts as seen (ADR 0121).
pub(in crate::app) const SEEN_AFTER: Duration = Duration::from_secs(2);

/// The most looks for changes at once, each a review's thread running git.
const LOOKS_AT_MOST: usize = 2;

/// The session in front of the person, and when that counts as seen, until it is told.
#[derive(Debug, Default)]
pub(super) struct Seen {
    watched: Option<(SessionKey, Option<Instant>)>,
}

impl Seen {
    pub(super) fn due(&self) -> Option<Instant> {
        self.watched.and_then(|(_, due)| due)
    }
}

/// A look for changes under way.
struct Look {
    key: SessionKey,
    project: ProjectId,
    root: TrustedRoot,
    _reviewer: Reviewer,
}

/// The looks for changes under way, and those waiting for one to end.
#[derive(Default)]
pub(super) struct Looks {
    /// Each look's session, project, root and thread: an untrusted project's root is revoked
    /// and its thread dropped.
    running: Vec<Look>,
    waiting: Vec<SessionKey>,
}

impl std::fmt::Debug for Looks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Looks")
            .field("running", &self.running.len())
            .field("waiting", &self.waiting)
            .finish()
    }
}

impl App {
    /// The session in front of the person: its terminal in the selected window of a focused
    /// oso window, with no prompt open.
    pub(super) fn watched_session(&self) -> Option<SessionKey> {
        if !self.terminals.focused || self.editor.prompt().is_some() {
            return None;
        }
        let buffer = self.editor.window().buffer();
        self.hosting.table().by_buffer(buffer).map(|s| s.key)
    }

    /// Whether session `key`'s terminal is in a window of oso's.
    fn session_shown(&self, key: SessionKey) -> bool {
        let Some(buffer) = self.hosting.table().get(key).and_then(|s| s.buffer) else {
            return false;
        };
        shown(&self.editor, buffer)
    }

    /// Session `key` became `state`: the attention's timers start, and a session in front of
    /// the person is looked at again.
    pub(super) fn attend(&mut self, key: SessionKey, state: State) {
        let Some(name) = self.hosting.table().get(key).map(|s| s.name.clone()) else {
            return;
        };
        let watched = self.watched_session() == Some(key);
        let shown = self.session_shown(key);
        let now = Instant::now();
        self.watch
            .attention
            .changed(key, &name, state, shown, watched, now);
        if watched {
            self.watch.seen.watched = Some((key, Some(now + SEEN_AFTER)));
        }
    }

    /// After each event: the session in front of the person, whose look counts once it has
    /// stayed [`SEEN_AFTER`].
    pub(in crate::app) fn sync_seen(&mut self) {
        let watched = self.watched_session();
        if self.watch.seen.watched.map(|(key, _)| key) != watched {
            self.watch.seen.watched = watched.map(|key| (key, Some(Instant::now() + SEEN_AFTER)));
        }
    }

    /// The look counts: the worker is told, once.
    pub(super) fn session_seen(&mut self) {
        let Some((key, Some(_))) = self.watch.seen.watched else {
            return;
        };
        self.watch.seen.watched = Some((key, None));
        if self.watched_session() != Some(key) {
            return;
        }
        if let Some(worker) = &self.sessions {
            worker.send(Job::Seen(key));
        }
    }

    /// What the attention has due at `now`: a notice, said in the echo area and asked of the
    /// system, and the marks' blink.
    pub(super) fn tell_attention(&mut self, now: Instant) {
        let watched = self.watched_session();
        let states: Vec<(SessionKey, State)> = self
            .watch
            .known
            .iter()
            .map(|(key, known)| (*key, known.state))
            .collect();
        let notice = self.watch.attention.tick(now, |key| {
            states
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, state)| Where {
                    state: *state,
                    watched: watched == Some(key),
                })
        });
        if let Some(notice) = notice {
            self.editor.message(notice.text.clone());
            oso_platform::notify::attention(notice.kind, "oso", &notice.text);
            #[cfg(feature = "dev")]
            {
                self.watch.asked += 1;
            }
            if let Some(view) = &self.view {
                view.window.request_session_attention(notice.kind);
            }
        }
        // The blink's phase may have turned.
        self.watch.stale = true;
    }

    /// Session `key`'s turn ended: in a trusted project, with a baseline, a review's thread
    /// takes the tree as it is now, and the worker is told whether it differs (ADR 0122).
    pub(super) fn look_for_changes(&mut self, key: SessionKey) {
        if self.watch.looks.running.iter().any(|look| look.key == key) {
            if !self.watch.looks.waiting.contains(&key) {
                self.watch.looks.waiting.push(key);
            }
            return;
        }
        if self.watch.looks.running.len() >= LOOKS_AT_MOST {
            if !self.watch.looks.waiting.contains(&key) {
                self.watch.looks.waiting.push(key);
            }
            return;
        }
        let Some(session) = self.hosting.table().get(key) else {
            return;
        };
        let (Some(baseline), Some(root)) = (session.baseline.clone(), session.root.clone()) else {
            return;
        };
        let Some((project, trusted)) = self.trusted_root(&root) else {
            return;
        };
        let kept = trusted.clone();
        let waker = std::sync::Mutex::new(self.waker.clone());
        let started = Reviewer::start(trusted, self.hosting.state_directory(), move |report| {
            let changed = match report {
                review::Report::Baseline { tree, .. } => Some(tree.as_str() != baseline),
                review::Report::Failed { .. } => None,
                _ => return true,
            };
            waker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .send(UserEvent::SessionChanges(key, changed));
            false
        });
        match started {
            Ok(reviewer) if reviewer.ask(review::Request::Baseline) => {
                self.watch.looks.running.push(Look {
                    key,
                    project,
                    root: kept,
                    _reviewer: reviewer,
                });
            }
            Ok(_) => {}
            Err(e) => log::warn!("cannot look for a session's changes: {e}"),
        }
    }

    /// A look for session `key`'s changes ended: `None` if git failed, which leaves its
    /// state as it is.
    pub(in crate::app) fn session_changes(&mut self, key: SessionKey, changed: Option<bool>) {
        self.watch.looks.running.retain(|look| look.key != key);
        if let (Some(changed), Some(worker)) = (changed, &self.sessions) {
            worker.send(Job::Changes(key, changed));
        }
        if let Some(next) =
            (!self.watch.looks.waiting.is_empty()).then(|| self.watch.looks.waiting.remove(0))
        {
            self.look_for_changes(next);
        }
    }

    /// A review of the session scope opened in the project at `root`: its sessions have been
    /// reviewed (ADR 0122).
    pub(in crate::app) fn session_review_opened(&mut self, root: &std::path::Path) {
        let Some(worker) = &self.sessions else {
            return;
        };
        for session in self.hosting.table().iter() {
            if session.root.as_deref() == Some(root) {
                worker.send(Job::ReviewOpened(session.key));
            }
        }
    }

    /// The trusted root of the project at `root`, if a buffer of that project is open and
    /// the project is trusted.
    fn trusted_root(&self, root: &std::path::Path) -> Option<(ProjectId, TrustedRoot)> {
        let id = self
            .editor
            .buffers()
            .filter_map(|(_, buffer)| buffer.project())
            .find(|id| {
                self.editor
                    .project(*id)
                    .is_some_and(|project| project.root() == root)
            })?;
        Some((id, TrustedRoot::of(&self.editor, id)?))
    }

    /// Stops the looks of projects no longer trusted: git runs no more for them (ADR 0122).
    pub(in crate::app) fn revoke_untrusted_looks(&mut self) {
        let editor = &self.editor;
        self.watch.looks.running.retain(|look| {
            let kept = editor.project_trusted(look.project);
            if !kept {
                look.root.revoke();
            }
            kept
        });
    }
}

/// Whether a tiled window of `editor` shows `buffer`.
fn shown(editor: &oso_core::editor::Editor, buffer: BufferId) -> bool {
    // A prompt's window shows the minibuffer, never a terminal.
    editor.windows().any(|window| window.buffer() == buffer)
}
