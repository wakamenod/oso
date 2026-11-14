//! Sending to a session, carried out (ADR 0121, Q17, step 5): the one delivery path,
//! [`App::send_to_session`], which refuses a session that cannot take a paste, pastes as the
//! person's bracketed paste, not submitted, and shows the session; the completions of
//! `session-send` and `session-mention`; and who last messaged each session, for its row and
//! `session-follow-message`.
//!
//! Must not depend on: how a frame is drawn, key presses, or the worker's thread. The text is
//! put together by pure functions over the parts, and the paste is the terminal's own.
//!
//! A sender is read from the screen, which any program in the terminal can print: it only
//! sets the row's note and the session `session-follow-message` shows, and authorises nothing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use oso_core::complete::Candidate;
use oso_core::display_map::cell::is_invisible;
use oso_core::editor::{SendPart, SendTo};

use crate::app::App;
use crate::commands::agent_sessions::{MENTION_LIST, SEND_LIST, too_large};
use crate::sessions::list::{Entry, Key};
use crate::sessions::send::{self, Standing, TooLarge};
use crate::sessions::state::State;
use crate::sessions::worker::SessionKey;

/// What sending keeps of the hosted sessions.
#[derive(Debug, Default)]
pub(in crate::app) struct Sending {
    /// Each session's newest sender, and whether the person has not seen the session since.
    from: HashMap<SessionKey, (String, bool)>,
    /// The sessions the worker has looked at since they started, each with whether a hold
    /// rule matched the newest look, `None` while that look ran out of time: a send waits for
    /// a look judged in time, and a view that holds refuses it.
    judged: HashMap<SessionKey, Option<bool>>,
}

impl App {
    /// `session-send` or `session-mention` asked to paste `parts` into the session `to`.
    pub(in crate::app) fn send_request(&mut self, to: &SendTo, parts: &[SendPart]) {
        let table = self.hosting.table();
        let session = match to {
            SendTo::Named(name) => self.session_named(name).and_then(|key| table.get(key)),
            SendTo::Buffer(buffer) => table.by_buffer(*buffer),
        };
        let Some(session) = session else {
            return self.editor.message(match to {
                SendTo::Named(name) => format!("No session {name}"),
                SendTo::Buffer(_) => "This terminal holds no agent session".to_owned(),
            });
        };
        let (key, name) = (session.key, session.name.clone());
        let text = match send::compose(parts, &session.directory) {
            Ok(text) => text,
            Err(TooLarge { bytes }) => return self.editor.message(too_large(bytes)),
        };
        if let Err(why) = self.send_to_session(key, text) {
            return self.editor.message(why);
        }
        // A command's excerpts all come from one buffer: the first without text tells.
        let placed = parts.iter().find_map(|part| match part {
            SendPart::Excerpt(excerpt) if excerpt.text.is_none() => Some(&excerpt.buffer),
            _ => None,
        });
        let withheld = placed.is_some_and(|placed| {
            self.editor
                .buffers()
                .any(|(_, buffer)| buffer.name() == placed && buffer.is_secret())
        });
        self.editor.message(if withheld {
            format!(
                "Pasted into {name}: press Enter there. Only the place went, as the buffer may \
                 hold secrets"
            )
        } else {
            format!("Pasted into {name}: press Enter there")
        });
    }

    /// Pastes `text` into hosted session `key`'s terminal as the person's bracketed paste,
    /// not submitted, then shows and selects the session in its space (Q2). Refused, with
    /// why, while the session cannot take a paste: not running, not judged yet since it
    /// started, asking for input, or held by a full-screen view.
    ///
    /// The one delivery path: what is sent to a session goes through here.
    pub(in crate::app) fn send_to_session(
        &mut self,
        key: SessionKey,
        text: String,
    ) -> Result<(), String> {
        let session = self
            .hosting
            .table()
            .get(key)
            .ok_or_else(|| "That session is gone".to_owned())?;
        let name = session.name.clone();
        let buffer = session
            .buffer
            .filter(|buffer| self.editor.buffer(*buffer).is_some())
            .ok_or_else(|| format!("{name} has no terminal: session-resume first"))?;
        // Bracketed paste off, a paste of several lines asks first and is no paste yet; the
        // TUI keeps it on, so a program that turned it off is not one a send is for.
        let bracketed = self
            .terminals
            .map
            .get(buffer)
            .is_some_and(|hosted| hosted.grid.modes().bracketed_paste);
        if !bracketed && session.runs() {
            return Err(format!(
                "{name}'s terminal takes no bracketed paste now; nothing is pasted"
            ));
        }
        let judged = self.watch.sending.judged.get(&key).copied().flatten();
        let standing = Standing {
            running: session.runs(),
            state: judged.map(|_| self.state_of(session, Instant::now()).0),
            // A password prompt, as ADR 0120's step 4b tells it: echo off with line editing
            // on, not the raw mode Claude Code's TUI runs in.
            echo_off: self
                .editor
                .buffer(buffer)
                .is_some_and(crate::app::terminals::reads_a_password),
            held: judged == Some(true),
        };
        if let Some(why) = send::refusal(&name, standing) {
            return Err(why);
        }
        self.paste(buffer, text);
        self.show_session(key);
        Ok(())
    }

    /// The worker judged session `key`'s newest look in time, a hold rule matching it or not,
    /// or, `None`, ran out of time on it.
    pub(in crate::app) fn session_judged(&mut self, key: SessionKey, held: Option<bool>) {
        self.watch.sending.judged.insert(key, held);
    }

    /// Session `key` changed to `state`: one that ended is judged afresh when it runs again.
    pub(in crate::app) fn sending_changed(&mut self, key: SessionKey, state: State) {
        if matches!(state, State::Stopped | State::Failed) {
            self.watch.sending.judged.remove(&key);
        }
    }

    /// Session `key`'s screen shows a message from `from`. Its row says so until the person
    /// sees it; at once if they look at it now.
    pub(in crate::app) fn message_from(&mut self, key: SessionKey, from: String) {
        let unseen = self.watched_session() != Some(key);
        self.watch.sending.from.insert(key, (from, unseen));
        self.watch.stale = true;
    }

    /// The person saw session `key`: its row no longer says who messaged it.
    pub(super) fn message_seen(&mut self, key: SessionKey) {
        if let Some((_, unseen)) = self.watch.sending.from.get_mut(&key)
            && std::mem::take(unseen)
        {
            self.watch.stale = true;
        }
    }

    /// `session-follow-message`: shows the session that last messaged the session whose
    /// terminal `buffer` is.
    pub(in crate::app) fn follow_message(&mut self, buffer: oso_core::buffer::BufferId) {
        let Some(session) = self.hosting.table().by_buffer(buffer) else {
            return self.editor.message("This buffer holds no agent session");
        };
        let name = session.name.clone();
        let Some((from, _)) = self.watch.sending.from.get(&session.key).cloned() else {
            return self.editor.message(format!("No message to {name} yet"));
        };
        let hosted = self
            .hosting
            .table()
            .iter()
            .filter(|session| !session.archived && session.buffer.is_some())
            .find(|session| session.name == from)
            .map(|session| session.key);
        match hosted {
            Some(key) => self.show_session(key),
            // The name is the screen's text: never logged, nor said where the channel reads.
            None if self.watch.externals.iter().any(|entry| entry.name == from) => {
                self.editor
                    .message("The sender runs outside oso, which shows no such session");
            }
            None => self.editor.message("The sender is no session oso knows"),
        }
    }

    /// What session `key`'s row says under its name: who messaged it, while the person has
    /// not seen it since, then its recap.
    pub(super) fn row_recap(&self, key: SessionKey) -> String {
        let recap = self.watch.known.get(&key).map_or("", |known| &known.recap);
        match self.watch.sending.from.get(&key) {
            Some((from, true)) if recap.is_empty() => format!("message from {from}"),
            Some((from, true)) => format!("message from {from} · {recap}"),
            _ => recap.to_owned(),
        }
    }

    /// Forgets what sending kept of sessions not in `live`.
    pub(super) fn forget_sending(&mut self, live: &std::collections::HashSet<SessionKey>) {
        self.watch.sending.from.retain(|key, _| live.contains(key));
        self.watch
            .sending
            .judged
            .retain(|key, _| live.contains(key));
    }

    /// Makes the completions of `session-send` and `session-mention` from `entries`.
    pub(super) fn sending_lists(&mut self, entries: &[Entry]) {
        let send = self.send_candidates();
        self.editor.set_listed(SEND_LIST, send);
        self.editor
            .set_listed(MENTION_LIST, mention_candidates(entries));
    }

    /// The running hosted sessions with a terminal, by its buffer's name: the session this
    /// space showed last first, so that `RET` takes it.
    fn send_candidates(&self) -> Arc<[Candidate]> {
        let last = self
            .watch
            .spaces
            .current()
            .and_then(|at| self.watch.spaces.get(at))
            .and_then(|space| space.last_session);
        let mut sessions: Vec<_> = self
            .hosting
            .table()
            .iter()
            .filter(|session| !session.archived && session.runs())
            .filter_map(|session| {
                let buffer = session.buffer?;
                let name = self.editor.buffer(buffer)?.name().to_owned();
                let space = self.watch.placed.get(&session.key).cloned();
                Some((Some(buffer) != last, name, space.unwrap_or_default()))
            })
            .collect();
        sessions.sort();
        sessions
            .into_iter()
            .map(|(_, name, space)| Candidate::new(name, "", space))
            .collect()
    }
}

/// The live sessions' names, hosted and other programs', each annotated with its space and
/// where it runs; an agent addresses one by its name with Claude Code's own `SendMessage`.
fn mention_candidates(entries: &[Entry]) -> Arc<[Candidate]> {
    entries
        .iter()
        .filter(|entry| !entry.archived && !matches!(entry.state, State::Stopped | State::Failed))
        // Another program's registry entry names it: a name that could hide or reorder text
        // is left out, as `@NAME` goes into buffers and terminals.
        .filter(|entry| {
            !entry
                .name
                .chars()
                .any(|c| c.is_control() || is_invisible(c) || c.is_whitespace())
        })
        .map(|entry| {
            let place = match entry.key {
                Key::Hosted(_) => "in oso",
                Key::External(_) => "outside oso",
            };
            Candidate::new(entry.name.clone(), "", format!("{} · {place}", entry.space))
        })
        .collect()
}

#[cfg(test)]
mod tests;
