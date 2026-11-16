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
use crate::sessions::adopt::Pick;
use crate::sessions::history::{self, About, History, Hit, Report};
use crate::sessions::hosted::Run;
use crate::sessions::identity::{SessionId, cleaned};
use crate::sessions::list::age_of;
use crate::sessions::registry::shown_directory;
use crate::sessions::worker::externals::External;

/// The longest name a row shows, in characters.
const NAME: usize = 40;

/// The longest place a row shows, in characters.
const PLACE: usize = 32;

/// The searcher for the app, waking the event loop with [`UserEvent::History`].
pub(in crate::app) fn history(waker: &Waker) -> History {
    let waker = Mutex::new(waker.clone());
    History::new(crate::sessions::transcripts::directory(), move |report| {
        waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(UserEvent::History(report));
    })
}

/// What the app keeps of the search shown.
#[derive(Debug, Default)]
pub(in crate::app) struct Searching {
    /// The conversations its rows are of, by their session id, for `RET`.
    abouts: HashMap<String, Arc<About>>,
    /// The ids of sessions Claude Code's registry said run in other programs, as oso last
    /// looked.
    pub(in crate::app) elsewhere: HashSet<SessionId>,
    /// The space in front the editor was last given the scope of, if any was.
    given: Option<Option<crate::sessions::spaces::SpaceId>>,
}

/// `text` cut to `chars` characters, with `…` where it was cut.
fn cut(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// Text of a transcript made fit for a row: one line, with no `·`, which splits a row's parts,
/// cut to `chars`.
fn shown(text: &str, chars: usize) -> String {
    let one = oso_core::editor::row_text(text).replace('·', " ");
    cut(one.trim(), chars)
}

impl App {
    /// Starts the search of conversations a prompt asked for, if any, and the preview's text
    /// its selected row asks for.
    pub(in crate::app) fn start_conversation_search(&mut self) {
        if let Some(request) = self.editor.take_conversation_search_request() {
            self.hosting.searching.abouts.clear();
            self.history.search(request);
        }
        if let Some(request) = self.editor.take_conversation_preview_request() {
            self.history.preview(request);
        }
    }

    /// The history thread reported: rows go to the prompt, labelled, and a preview's text to
    /// the preview.
    pub(in crate::app) fn history_reported(&mut self, event_loop: &Loop<'_>, report: Report) {
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
        let place = about.directory.as_deref().map_or_else(
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
        if about.gone {
            let directory = about
                .directory
                .as_deref()
                .map_or_else(String::new, shown_directory);
            return self.editor.message(format!(
                "{directory} is gone, so this conversation cannot resume there"
            ));
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
            .map(|space| space.id().clone());
        if !changed && self.hosting.searching.given.as_ref() == Some(&front) {
            return;
        }
        self.hosting.searching.given = Some(front);
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
        .as_deref()
        .and_then(cleaned)
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
        assert_eq!(cut("abcdef", 3), "abc…");
        assert_eq!(cut("abc", 3), "abc");
        let said = shown("first · line\nsecond", 40);
        assert!(!said.contains('·'), "{said}");
        assert!(!said.contains('\n'), "{said}");
    }
}
