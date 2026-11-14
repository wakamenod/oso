//! What the app keeps of agent sessions for ADR 0121 step 3, and what follows a change: the
//! screen feed to the sessions worker, each session's state as the worker told it, and from
//! that `*Sessions*`, the modeline's marks, the completions over sessions and spaces, the
//! session buffers' modelines and the person's attention. A change costs the UI thread one
//! pass over the sessions, made once after the event that brought it (ADR 0121: 0.5 ms).
//!
//! Must not depend on: how a frame is drawn, key presses, or the worker's thread.

mod alerts;
#[cfg(feature = "dev")]
mod dev_state;
mod requests;
mod view;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oso_core::buffer::BufferId;
use oso_core::complete::Candidate;
use oso_core::editor::SessionMarks;
use oso_core::minibuffer::PromptId;

use super::feed::{self, Feed, Now};
use crate::app::App;
use crate::commands::agent_sessions::{SESSIONS_LIST, SPACES_LIST};
use crate::sessions::attention::{Attention, Toggles};
use crate::sessions::hosted::{Hosted, Run};
use crate::sessions::identity::SessionId;
use crate::sessions::list::{Entry, Key, List};
use crate::sessions::screen::Evidence;
use crate::sessions::spaces::Spaces;
use crate::sessions::state::{Change, State};
use crate::sessions::worker::SessionKey;
use crate::sessions::worker::externals::External;

/// What the worker last told of a hosted session.
#[derive(Debug, Clone)]
struct Known {
    state: State,
    since: Instant,
    recap: String,
}

/// Step 3's state of agent sessions.
#[derive(Debug, Default)]
pub(in crate::app) struct Watch {
    feed: Feed,
    known: HashMap<SessionKey, Known>,
    /// Other programs' sessions, read from Claude Code's registry: listed, never shown.
    externals: Vec<Entry>,
    attention: Attention,
    list: List,
    spaces: Spaces,
    view: view::View,
    seen: alerts::Seen,
    looks: alerts::Looks,
    /// A stop that asked first: the question, and the session it stops on a yes.
    stop: Option<(PromptId, SessionKey)>,
    /// Sessions `k` hung up, whose end is then a stop and not a failure.
    hung_up: HashSet<SessionKey>,
    /// Something the views show changed since they were last made.
    stale: bool,
    /// When the views were last made, for their ages.
    made: Option<Instant>,
    /// The worker looks at Claude Code's registry: asked once oso has a session.
    polling: bool,
    /// The marks the modeline was last given.
    marks: SessionMarks,
    /// The table changed since the views were made: spaces and the poll are looked at again.
    touched: bool,
    /// The views are made again for their ages only.
    aging: bool,
    /// Each hosted session's space's name.
    placed: HashMap<SessionKey, String>,
    /// How many notices asked for attention, for end-to-end flows: the Dock cannot be seen.
    #[cfg(feature = "dev")]
    asked: u32,
}

impl Watch {
    /// How many sessions the modeline last counted as needing input.
    #[cfg(feature = "dev")]
    pub(in crate::app) fn needs_input(&self) -> u16 {
        self.marks.needs_input
    }

    /// Whether `k` hung session `key` up, which its end then answers.
    pub(in crate::app) fn take_hung_up(&mut self, key: SessionKey) -> bool {
        self.hung_up.remove(&key)
    }

    /// `[sessions.notify]` in force.
    pub(in crate::app) fn set_toggles(&mut self, toggles: Toggles) {
        self.attention.set_toggles(toggles);
    }
}

impl App {
    /// Sends each hosted session's screen that changed to the worker, as the feed paces them
    /// (ADR 0121, Q6). Runs after snapshots are merged, and when a change that waited is due.
    pub(in crate::app) fn feed_sessions(&mut self) {
        let Some(worker) = &self.sessions else {
            return;
        };
        let now = Instant::now();
        // By the table, whose sessions each name their buffer: one lookup each.
        for session in self.hosting.table().iter() {
            let Some(terminal) = session.buffer.and_then(|id| self.terminals.map.get(id)) else {
                continue;
            };
            let progress = terminal.progress.and_then(feed::progress);
            let screen = Now {
                generation: terminal.grid.generation(),
                title: &terminal.title,
                progress,
                scrolled: terminal.grid.scrolled(),
            };
            if self.watch.feed.look(session.key, screen, now) {
                let rows = terminal.grid.row_texts();
                let evidence =
                    Evidence::new(rows.iter().map(String::as_str), &terminal.title, progress);
                worker.send_screen(session.key, evidence);
            }
        }
        // A change still due was of a session or terminal that went: it is forgotten, so
        // that the loop does not wake for it again and again.
        if self.watch.feed.due().is_some_and(|due| due <= now) {
            let live: std::collections::HashSet<SessionKey> = self
                .hosting
                .table()
                .iter()
                .filter(|s| {
                    s.buffer
                        .is_some_and(|id| self.terminals.map.get(id).is_some())
                })
                .map(|s| s.key)
                .collect();
            self.watch.feed.keep(|key| live.contains(&key));
        }
    }

    /// When a session's moment comes next: a screen change that waited for the feed's pace,
    /// a notice or a blink of the marks, or the person's look at a session.
    pub(in crate::app) fn sessions_deadline(&self) -> Option<Instant> {
        [
            self.watch.feed.due(),
            self.watch.attention.due(),
            self.watch.seen.due(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Before the event loop waits: does what is due for sessions.
    pub(in crate::app) fn sessions_due(&mut self) {
        let now = Instant::now();
        if self.watch.feed.due().is_some_and(|due| now >= due) {
            self.feed_sessions();
        }
        if self.watch.attention.due().is_some_and(|due| now >= due) {
            self.tell_attention(now);
        }
        if self.watch.seen.due().is_some_and(|due| now >= due) {
            self.session_seen();
        }
        self.refresh_sessions();
    }

    /// The worker found that session `key` changed state.
    pub(in crate::app) fn session_changed(&mut self, key: SessionKey, change: Change) {
        let recap = self
            .watch
            .known
            .remove(&key)
            .map(|known| known.recap)
            .unwrap_or_default();
        self.watch.known.insert(
            key,
            Known {
                state: change.state,
                since: change.since,
                recap,
            },
        );
        self.attend(key, change.state);
        if change.turn_ended {
            self.look_for_changes(key);
        }
        self.watch.stale = true;
    }

    /// The worker found session `key`'s recap changed.
    pub(in crate::app) fn session_recapped(&mut self, key: SessionKey, recap: String) {
        if let Some(known) = self.watch.known.get_mut(&key) {
            known.recap = recap;
        } else {
            self.watch.known.insert(
                key,
                Known {
                    state: State::Idle,
                    since: Instant::now(),
                    recap,
                },
            );
        }
        self.watch.stale = true;
    }

    /// The sessions changed in a way the worker does not tell: one was added, recalled,
    /// pinned or archived.
    pub(in crate::app) fn sessions_touched(&mut self) {
        self.watch.stale = true;
        self.watch.touched = true;
    }

    /// Makes the views of sessions again if something they show changed, and asks for a
    /// frame: the one pass a change costs.
    pub(in crate::app) fn refresh_sessions(&mut self) {
        if self.remake_sessions() {
            self.request_redraw();
        }
    }

    /// Makes the views of sessions again if something they show changed. Returns whether
    /// something visible changed, which needs a frame.
    fn remake_sessions(&mut self) -> bool {
        let person_hid = self.watch.view.shown && self.editor.side_window().is_none();
        if !self.watch.stale && !person_hid {
            return false;
        }
        self.watch.stale = false;
        let aging = std::mem::take(&mut self.watch.aging);
        let now = Instant::now();
        self.watch.made = Some(now);
        if std::mem::take(&mut self.watch.touched) {
            self.place_sessions();
            self.forget_gone_sessions();
            self.poll_externals();
        }
        let entries = self.session_entries(now);
        let marks = marks(&entries, self.watch.attention.dark(now));
        let mut visible = marks != self.watch.marks;
        if visible {
            self.watch.marks = marks;
            self.editor.set_session_marks(marks);
        }
        // Ages alone change no candidate.
        if !aging {
            let candidates = self.session_candidates(&entries);
            self.editor.set_listed(SESSIONS_LIST, candidates);
            self.editor
                .set_listed(SPACES_LIST, self.watch.spaces.candidates());
        }
        visible |= self.note_sessions(&entries, now);
        visible |= self.show_sessions_list(&entries, now);
        visible
    }

    /// A frame is drawn anyway: the ages shown are made again once they may have moved on,
    /// a second later (ADR 0121, Q4: no timer of their own).
    pub(in crate::app) fn age_sessions(&mut self) {
        let shows = self.watch.view.shown || self.hosting.table().iter().next().is_some();
        if shows
            && self
                .watch
                .made
                .is_some_and(|made| made.elapsed() >= Duration::from_secs(1))
        {
            // The frame being drawn shows them.
            self.watch.stale = true;
            self.watch.aging = true;
            self.remake_sessions();
        }
    }

    /// Asks the worker to look at Claude Code's registry while a session of oso's runs, and
    /// to stop when none does.
    fn poll_externals(&mut self) {
        let runs = self.hosting.table().iter().any(Hosted::runs);
        if self.watch.polling == runs {
            return;
        }
        if let Some(worker) = &self.sessions {
            worker.send(crate::sessions::worker::Job::Externals(runs));
            self.watch.polling = runs;
            if !runs {
                self.watch.externals.clear();
            }
        }
    }

    /// The worker found the live sessions of Claude Code's registry: those that are not
    /// oso's own are listed, never shown.
    pub(in crate::app) fn externals_found(&mut self, live: Vec<External>) {
        // A look the worker made before it was told to stop: the list keeps none.
        if !self.watch.polling {
            return;
        }
        let now = Instant::now();
        let table = self.hosting.table();
        let ids: HashSet<&SessionId> = table.iter().map(|session| &session.id).collect();
        let children: HashSet<u32> = table
            .iter()
            .filter_map(|session| match session.run {
                Run::Running { child } => child,
                _ => None,
            })
            .collect();
        let ours = |external: &External| {
            external
                .session_id
                .as_ref()
                .is_some_and(|id| ids.contains(id))
                || children.contains(&external.pid)
        };
        let before: HashMap<u32, (State, Instant)> = self
            .watch
            .externals
            .iter()
            .filter_map(|kept| match kept.key {
                Key::External(pid) => Some((pid, (kept.state, kept.since))),
                Key::Hosted(_) => None,
            })
            .collect();
        let mut externals = Vec::new();
        for external in live.iter().filter(|external| !ours(external)) {
            let space = external
                .directory
                .as_deref()
                .and_then(|dir| self.watch.spaces.of_path(dir))
                .and_then(|at| self.watch.spaces.get(at))
                .map_or_else(
                    || crate::sessions::list::EXTERNAL.to_owned(),
                    |space| space.name.clone(),
                );
            let state = if external.working {
                State::Working
            } else {
                State::Idle
            };
            // Its age counts from when oso first saw it in this state.
            let since = before
                .get(&external.pid)
                .filter(|(kept, _)| *kept == state)
                .map_or(now, |(_, since)| *since);
            externals.push(Entry {
                key: Key::External(external.pid),
                name: external
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("claude {}", external.pid)),
                space,
                branch: None,
                state,
                since,
                recap: String::new(),
                pinned: false,
                archived: false,
                parent: None,
            });
        }
        let gone_from: Vec<u32> = before.keys().copied().collect();
        self.watch.externals = externals;
        self.watch.stale = true;
        self.adoptable_externals(&gone_from, &live);
    }

    /// What is kept of sessions no longer in the table goes.
    fn forget_gone_sessions(&mut self) {
        let keys: HashSet<SessionKey> = self.hosting.table().iter().map(|s| s.key).collect();
        let gone: Vec<SessionKey> = self
            .watch
            .known
            .keys()
            .filter(|key| !keys.contains(key))
            .copied()
            .collect();
        for key in gone {
            self.watch.known.remove(&key);
            self.watch.attention.forget(key);
        }
    }

    /// Each hosted session's space, added if new; the frame's tree is the first space's.
    fn place_sessions(&mut self) {
        let roots: Vec<_> = self
            .hosting
            .table()
            .iter()
            .map(|session| place(session).to_path_buf())
            .collect();
        if self.watch.spaces.current().is_none()
            && !roots.is_empty()
            && let Ok((root, _)) = self.editor.project_files_root()
        {
            let at = self.watch.spaces.add(&root, None);
            self.watch.spaces.adopt(at);
        }
        for root in roots {
            self.watch.spaces.add(&root, None);
        }
        // Each session's space's name, worked out once a change of the table.
        let placed: HashMap<SessionKey, String> = self
            .hosting
            .table()
            .iter()
            .map(|session| {
                let name = self
                    .watch
                    .spaces
                    .of_path(place(session))
                    .and_then(|at| self.watch.spaces.get(at))
                    .map_or_else(String::new, |space| space.name.clone());
                (session.key, name)
            })
            .collect();
        self.watch.placed = placed;
    }

    /// What the list shows of each session: hosted ones, then other programs'.
    fn session_entries(&self, now: Instant) -> Vec<Entry> {
        let table = self.hosting.table();
        let by_id: HashMap<&SessionId, SessionKey> = table
            .iter()
            .map(|session| (&session.id, session.key))
            .collect();
        let mut entries: Vec<Entry> = table
            .iter()
            .map(|session| {
                let (state, since) = self.state_of(session, now);
                let space = self
                    .watch
                    .placed
                    .get(&session.key)
                    .cloned()
                    .unwrap_or_default();
                let parent = session
                    .parent
                    .as_ref()
                    .and_then(|id| by_id.get(id))
                    .map(|key| Key::Hosted(*key));
                Entry {
                    key: Key::Hosted(session.key),
                    name: session.name.clone(),
                    space,
                    branch: None,
                    state,
                    since,
                    recap: self
                        .watch
                        .known
                        .get(&session.key)
                        .map_or_else(String::new, |known| known.recap.clone()),
                    pinned: session.pinned,
                    archived: session.archived,
                    parent,
                }
            })
            .collect();
        entries.extend(self.watch.externals.iter().cloned());
        entries
    }

    /// Session `session`'s state and since when: as the worker told it, while that agrees
    /// with whether its process runs.
    fn state_of(&self, session: &Hosted, now: Instant) -> (State, Instant) {
        let ended = !session.runs();
        let known = self.watch.known.get(&session.key);
        match known {
            Some(known) if ended == matches!(known.state, State::Stopped | State::Failed) => {
                (known.state, known.since)
            }
            _ => {
                let since = since_ms(session.changed, now);
                let state = match session.run {
                    Run::Failed => State::Failed,
                    Run::Stopped => State::Stopped,
                    Run::Starting | Run::Running { .. } => State::Idle,
                };
                (state, since)
            }
        }
    }

    /// The candidates of `session-switch`: each hosted session with a buffer, by its buffer's
    /// name, annotated with its state, space, branch and recap, so that `&needs` finds those
    /// that need input.
    fn session_candidates(&self, entries: &[Entry]) -> Arc<[Candidate]> {
        let buffers = self.session_buffers();
        entries
            .iter()
            .filter(|entry| !entry.archived)
            .filter_map(|entry| {
                let Key::Hosted(key) = entry.key else {
                    return None;
                };
                let buffer = *buffers.get(&key)?;
                let name = self.editor.buffer(buffer)?.name();
                let mut annotation = format!("{} · {}", entry.state, entry.space);
                if let Some(branch) = &entry.branch {
                    annotation.push_str(" · ");
                    annotation.push_str(branch);
                }
                if !entry.recap.is_empty() {
                    annotation.push_str(" · ");
                    annotation.push_str(&entry.recap);
                }
                Some(Candidate::new(name, "", annotation))
            })
            .collect()
    }

    /// Puts each hosted session's state and age in its terminal's modeline, beside the title
    /// (`needs input (permission) 3m`).
    /// Returns whether a note changed.
    fn note_sessions(&mut self, entries: &[Entry], now: Instant) -> bool {
        let buffers = self.session_buffers();
        let mut changed = false;
        for entry in entries {
            let Key::Hosted(key) = entry.key else {
                continue;
            };
            let Some(&buffer) = buffers.get(&key) else {
                continue;
            };
            let Some(hosted) = self.terminals.map.get_mut(buffer) else {
                continue;
            };
            // A stopped or failed session's modeline says how to go on already.
            let note = if matches!(entry.state, State::Stopped | State::Failed) {
                String::new()
            } else {
                format!(
                    "{} {}",
                    entry.state,
                    crate::sessions::list::age(entry.since, now)
                )
            };
            if hosted.session_note != note {
                hosted.session_note = note;
                hosted.note_stale = true;
                changed = true;
            }
        }
        if changed {
            self.note_terminals();
        }
        changed
    }

    /// Each hosted session's terminal buffer, by its key.
    fn session_buffers(&self) -> HashMap<SessionKey, BufferId> {
        self.hosting
            .table()
            .iter()
            .filter_map(|session| Some((session.key, session.buffer?)))
            .collect()
    }
}

/// Where session `session` runs, for its space: its project root, else its directory.
fn place(session: &Hosted) -> &std::path::Path {
    session.root.as_deref().unwrap_or(&session.directory)
}

/// The instant `changed`, in ms since the epoch, was, as of `now`.
fn since_ms(changed: u64, now: Instant) -> Instant {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    now.checked_sub(Duration::from_millis(now_ms.saturating_sub(changed)))
        .unwrap_or(now)
}

/// The modeline's marks of `entries`: those that need input, and those done or ready for
/// review, archived ones aside.
fn marks(entries: &[Entry], dark: bool) -> SessionMarks {
    let count = |wanted: fn(State) -> bool| {
        let n = entries
            .iter()
            .filter(|entry| !entry.archived && wanted(entry.state))
            .count();
        u16::try_from(n).unwrap_or(u16::MAX)
    };
    SessionMarks {
        needs_input: count(|state| matches!(state, State::NeedsInput(_))),
        done: count(|state| matches!(state, State::Done | State::ReadyForReview)),
        dark,
    }
}

#[cfg(test)]
mod tests;
