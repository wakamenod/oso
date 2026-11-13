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
use oso_core::editor::{SendPart, SendTo};

use crate::app::App;
use crate::commands::agent_sessions::{MENTION_LIST, SEND_LIST};
use crate::sessions::list::{Entry, Key};
use crate::sessions::send::{self, MAX_BYTES, Standing, TooLarge};
use crate::sessions::state::State;
use crate::sessions::worker::SessionKey;

/// What sending keeps of the hosted sessions.
#[derive(Debug, Default)]
pub(in crate::app) struct Sending {
    /// Each session's newest sender, and whether the person has not seen the session since.
    from: HashMap<SessionKey, (String, bool)>,
    /// The sessions the worker has judged in time since they started, each with whether a
    /// hold rule matched last: a send waits for the first, and a view that holds refuses it.
    judged: HashMap<SessionKey, bool>,
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
            Err(TooLarge { bytes }) => {
                return self.editor.message(format!(
                    "That is {} KiB; at most {} KiB goes to a session. C-u C-x a m sends the \
                     path and lines only",
                    bytes.div_ceil(1024),
                    MAX_BYTES >> 10
                ));
            }
        };
        if let Err(why) = self.send_to_session(key, text) {
            return self.editor.message(why);
        }
        let withheld = parts.iter().any(|part| match part {
            SendPart::Excerpt(excerpt) => {
                excerpt.text.is_none()
                    && self
                        .editor
                        .buffers()
                        .any(|(_, buffer)| buffer.name() == excerpt.buffer && buffer.is_secret())
            }
            SendPart::Note(_) => false,
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
        let judged = self.watch.sending.judged.get(&key).copied();
        let standing = Standing {
            running: session.runs(),
            state: judged.map(|_| self.state_of(session, Instant::now()).0),
            // ADR 0120's step 4b tells a password prompt; Claude Code's TUI keeps echo off
            // in raw mode, so the PTY's own flag would refuse every session.
            echo_off: false,
            held: judged == Some(true),
        };
        if let Some(why) = send::refusal(&name, standing) {
            return Err(why);
        }
        self.paste(buffer, text);
        self.show_session(key);
        Ok(())
    }

    /// The worker judged session `key`'s screen in time, a hold rule matching it or not.
    pub(in crate::app) fn session_judged(&mut self, key: SessionKey, held: bool) {
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
            None if self.watch.externals.iter().any(|entry| entry.name == from) => {
                self.editor.message(format!("{from} runs outside oso"));
            }
            None => self.editor.message(format!("No session {from}")),
        }
    }

    /// The row's note for session `key`, before its recap, while the person has not seen it
    /// since a message came.
    pub(super) fn message_note(&self, key: SessionKey) -> Option<String> {
        let (from, unseen) = self.watch.sending.from.get(&key)?;
        unseen.then(|| format!("message from {from}"))
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
