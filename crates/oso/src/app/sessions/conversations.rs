//! `session-search` in the app (ADR 0145): the search its prompt asked for, started on the
//! `oso-history` thread, the rows it sends, labelled with what oso knows of each session and
//! handed to the editor, the preview's text, and `RET`'s conversation: shown if oso hosts it,
//! else adopted as `session-adopt` adopts one.
//!
//! Must not depend on: key presses or how a frame is drawn. Transcripts are read on the
//! history thread, never here, and nothing here logs what it found.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use oso_core::editor::{ConversationRow, ConversationScope};

use crate::app::{App, Loop, UserEvent, Waker};
use crate::sessions::adopt::{ANNOTATION_DOTS, Pick};
use crate::sessions::history::{self, About, History, Hit, Report};
use crate::sessions::hosted::Run;
use crate::sessions::identity::SessionId;
use crate::sessions::list::{age_of, cut};
use crate::sessions::worker::externals::External;

/// The most cells a row's name takes.
const NAME: usize = 40;

/// The most cells a row's place takes.
const PLACE: usize = 32;

/// The searcher for the app, waking the event loop with [`UserEvent::Conversations`].
pub(in crate::app) fn conversations(waker: &Waker) -> History {
    let waker = Mutex::new(waker.clone());
    History::new(crate::sessions::transcripts::directory(), move |report| {
        waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(UserEvent::Conversations(report));
    })
}

/// What the app keeps of the search shown. `Debug` gives counts only.
#[derive(Default)]
pub(in crate::app) struct Searching {
    /// The conversations its rows are of, by their session id, for `RET`.
    abouts: HashMap<String, Arc<About>>,
    /// The ids of sessions Claude Code's registry said run in other programs, as oso last
    /// looked.
    pub(in crate::app) elsewhere: HashSet<SessionId>,
    /// The space in front the editor was last given the scope of, if any was.
    given: Option<Option<crate::sessions::spaces::SpaceId>>,
}

impl std::fmt::Debug for Searching {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Searching")
            .field("abouts", &self.abouts.len())
            .field("elsewhere", &self.elsewhere.len())
            .finish_non_exhaustive()
    }
}

/// Text of a transcript made fit for a row, as `session-adopt` shows a name: escaped onto one
/// row, without the dots a row's parts are split by, cut to `cells`.
fn shown(text: &str, cells: usize) -> String {
    let one = oso_core::editor::row_text(text.trim()).replace(ANNOTATION_DOTS, " ");
    cut(one.trim(), cells)
}

impl App {
    /// Starts the search of conversations a prompt asked for, if any, and the preview's text
    /// its selected row asks for.
    pub(in crate::app) fn start_conversation_search(&mut self) {
        if let Some(request) = self.editor.take_conversation_search_request() {
            self.hosting.searching.abouts.clear();
            self.conversations.search(request);
        }
        if let Some(request) = self.editor.take_conversation_preview_request() {
            self.conversations.preview(request);
        }
    }

    /// The history thread reported: rows go to the prompt, labelled, and a preview's text to
    /// the preview.
    pub(in crate::app) fn conversations_reported(&mut self, event_loop: &Loop<'_>, report: Report) {
        match report {
            Report::Found(found) => {
                let now = SystemTime::now();
                let rows = found
                    .hits
                    .into_iter()
                    .map(|hit| self.conversation_row(hit, now))
                    .collect();
                self.editor
                    .conversation_rows_found(found.generation, rows, found.end);
                self.start_completions(event_loop);
            }
            Report::Previewed(previewed) => {
                self.editor
                    .preview_read(previewed.generation, previewed.read);
            }
        }
        self.request_redraw();
    }

    /// The row `hit` shows: its conversation's mark, name, place and age, and the message.
    fn conversation_row(&mut self, hit: Hit, now: SystemTime) -> ConversationRow {
        let about = &hit.about;
        let hosted = self
            .hosting
            .table()
            .iter()
            .find(|session| session.id == about.id);
        let (mark, name) = match hosted {
            Some(session) => {
                let running = matches!(session.run, Run::Running { .. } | Run::Starting);
                (if running { '●' } else { '○' }, session.name.clone())
            }
            None => {
                let mark = if self.hosting.searching.elsewhere.contains(&about.id) {
                    '⊘'
                } else {
                    '◌'
                };
                (mark, conversation_name(about))
            }
        };
        let place = about.shown_directory().map_or_else(
            || "?".to_owned(),
            |directory| {
                let place = shown(&history::place(directory), PLACE);
                if about.gone {
                    format!("{place} (gone)")
                } else {
                    place
                }
            },
        );
        let age = now
            .duration_since(about.changed)
            .map_or_else(|_| "now".to_owned(), age_of);
        let said = if hit.prompt { '›' } else { '‹' };
        let label = format!("{mark} {} · {place} · {age} {said}", shown(&name, NAME));
        let session: Box<str> = about.id.as_str().into();
        self.hosting
            .searching
            .abouts
            .entry(session.to_string())
            .or_insert_with(|| Arc::clone(&hit.about));
        ConversationRow {
            session,
            label: label.into(),
            message: hit.message,
            line: hit.line,
            column: hit.column,
            text: hit.text,
        }
    }

    /// Keeps the ids of the sessions in `live`, Claude Code's registry's, that oso does not
    /// host, which `session-search` marks as running elsewhere.
    pub(in crate::app) fn note_elsewhere(&mut self, live: &[External]) {
        let table = self.hosting.table();
        let elsewhere = live
            .iter()
            .filter_map(|external| external.session_id.clone())
            .filter(|id| table.iter().all(|session| session.id != *id))
            .collect();
        self.hosting.searching.elsewhere = elsewhere;
    }

    /// Goes to the conversation of `id`, `RET`'s on a row of `session-search`: shows it if oso
    /// hosts it, else adopts it, unless its directory is gone.
    pub(in crate::app) fn go_to_conversation(&mut self, id: &str) {
        let Some(id) = SessionId::parse(id) else {
            return self.editor.message("No such conversation");
        };
        let hosted = self
            .hosting
            .table()
            .iter()
            .find(|session| session.id == id)
            .map(|session| session.key);
        if let Some(key) = hosted {
            return self.show_session(key);
        }
        let Some(about) = self.hosting.searching.abouts.get(id.as_str()).cloned() else {
            return self
                .editor
                .message("No such conversation now: run session-search again");
        };
        // Text a transcript gave is never logged, and agents read `*Messages*`.
        if about.gone {
            return self.editor.message_unlogged(
                "Its directory is gone, so this conversation cannot resume there",
            );
        }
        if about.directory.is_none() {
            return self.editor.message_unlogged(
                "oso cannot tell where that conversation ran, so it cannot resume it",
            );
        }
        self.adopt_conversation(Pick {
            id,
            directory: about.directory.clone(),
            name: about.title.clone(),
            running: None,
            busy: false,
        });
    }

    /// Gives the editor the scope of the space in front for `session-search` if another space
    /// is in front than when it was last given, or with `changed`, as what the spaces hold
    /// changed.
    pub(in crate::app) fn keep_space_scope(&mut self, changed: bool) {
        let spaces = self.watch.spaces();
        let front = spaces
            .current()
            .and_then(|at| spaces.get(at))
            .map(|space| space.id());
        if !changed && self.hosting.searching.given.as_ref().map(Option::as_ref) == Some(front) {
            return;
        }
        self.hosting.searching.given = Some(front.cloned());
        self.give_space_scope();
    }

    /// The scope of the space in front for `session-search`: its repositories, its directory
    /// and worktrees, and its orchestrators' and hosted sessions' ids.
    fn give_space_scope(&mut self) {
        let spaces = self.watch.spaces();
        let scope = spaces.current().and_then(|at| spaces.get(at)).map(|space| {
            let stored = &space.stored;
            let mut roots: Vec<_> = stored
                .members
                .iter()
                .map(|member| member.root.clone())
                .collect();
            roots.extend(stored.directory.clone());
            roots.extend(stored.worktrees.iter().cloned());
            let mut sessions: Vec<Box<str>> = stored
                .orchestrator
                .iter()
                .chain(&stored.earlier)
                .map(|id| id.as_str().into())
                .collect();
            sessions.extend(
                self.hosting
                    .table()
                    .iter()
                    .filter(|session| session.space.as_ref() == Some(&stored.id))
                    .map(|session| session.id.as_str().into()),
            );
            let label = format!("space {}", shown(&stored.name, NAME));
            ConversationScope::within(label, roots, sessions)
        });
        self.editor.set_space_scope(scope);
    }
}

/// The name a row gives a conversation oso does not host: its title, else the first line of
/// its first prompt, else its id's first 8 digits.
fn conversation_name(about: &About) -> String {
    about
        .title
        .clone()
        .or_else(|| {
            about
                .first_prompt
                .clone()
                .filter(|prompt| !prompt.trim().is_empty())
        })
        .unwrap_or_else(|| about.id.as_str().chars().take(8).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_text_is_cut_and_put_on_one_row() {
        assert!(
            shown(&"あ".repeat(40), 10).chars().count() <= 10,
            "cut by cells"
        );
        assert_eq!(shown("abc", 3), "abc");
        let said = shown("first · line\nsecond", 40);
        assert!(!said.contains('·'), "{said}");
        assert!(!said.contains('\n'), "{said}");
        let spoof = shown("fix\u{202e}gol\r\u{2066}x", 40);
        assert!(!spoof.contains(['\u{202e}', '\r', '\u{2066}']), "{spoof}");
    }
}
