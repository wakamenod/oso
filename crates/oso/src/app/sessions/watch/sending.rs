//! Sending to a session, carried out (ADR 0121, Q17, step 5): the one delivery path,
//! [`App::paste_into_session`], which takes what is sent as [`Outgoing`], escapes it once
//! (`sessions::send::for_claude`, ADR 0121's "Composed text, as amended"), refuses a session
//! that cannot take it, and pastes it as the person's bracketed paste, not submitted, or puts it
//! into a native session's composer at point, not sent (ADR 0165, part 2f), showing the session
//! when asked; [`App::send_to_session`] is it, shown. The submits, for a review's question the
//! person confirmed in oso's prompt (ADR 0163): [`App::submit_paste`] for a terminal, and a
//! native session's `send_confirmed` (ADR 0165, part 2f). Also the completions of
//! `session-send` and `session-mention`, and who last messaged each session, for its row and
//! `session-follow-message`. `images` keeps its own state here: the session an image is read
//! for, and the thread that keeps the images (ADR 0142).
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

/// What is sent to a session, which says how it is escaped.
#[derive(Clone, Copy, Debug)]
pub(in crate::app) enum Outgoing<'a> {
    /// Text oso composed, from what others wrote too: escaped (`sessions::send::for_claude`).
    Composed(&'a str),
    /// oso's own reference, a kept image's `@PATH` (ADR 0142): as it is, but that it makes no
    /// `@` after it the start of a word (`sessions::send::reference`).
    Reference(&'a str),
}

impl<'a> Outgoing<'a> {
    /// The text as it goes in before `next`, what follows it where it goes.
    pub(in crate::app) fn escaped(self, next: Option<char>) -> std::borrow::Cow<'a, str> {
        match self {
            Self::Composed(text) => send::for_claude(text, next),
            Self::Reference(text) => send::reference(text, next),
        }
    }

    /// Its length before it is escaped, which escaping never shortens.
    pub(in crate::app) fn len(self) -> usize {
        match self {
            Self::Composed(text) | Self::Reference(text) => text.len(),
        }
    }
}

/// Whether a send shows its session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Shown {
    /// Shown and selected in its space.
    Session,
    /// Left where it is, as a review's question is shown in the review layout's slot.
    Nothing,
}

/// What sending keeps of the hosted sessions.
#[derive(Debug, Default)]
pub(in crate::app) struct Sending {
    /// Each session's newest sender, and whether the person has not seen the session since.
    from: HashMap<SessionKey, (String, bool)>,
    /// The sessions the worker has looked at since they started, each with whether a hold
    /// rule matched the newest look, `None` while that look ran out of time: a send waits for
    /// a look judged in time, and a view that holds refuses it.
    judged: HashMap<SessionKey, Option<bool>>,
    /// The session a `session-yank-image` reads the clipboard's image for, while the read or
    /// the save is out: one at a time (ADR 0142).
    pub(super) reading_image_for: Option<SessionKey>,
    /// The thread that keeps and removes the sessions' images (ADR 0142).
    pub(super) keeper: crate::sessions::images::Keeper,
}

impl super::Watch {
    /// The names of other programs' sessions, as `*Spaces*` lists them, which a rename may not
    /// take (ADR 0155).
    pub(in crate::app) fn external_names(&self) -> impl Iterator<Item = &str> {
        self.externals.iter().map(|entry| entry.name.as_str())
    }

    /// Whether the worker was asked to look at Claude Code's registry: a look it made after it
    /// was told to stop is dropped.
    pub(in crate::app) fn polls(&self) -> bool {
        self.polling
    }
}

impl App {
    /// `session-send` or `session-mention` asked to paste `parts` into the session `to`.
    pub(in crate::app) fn send_request(&mut self, to: &SendTo, parts: &[SendPart]) {
        let table = self.hosting.table();
        let session = match to {
            SendTo::Named(name) => self.session_named(name).and_then(|key| table.get(key)),
            // A native session's composer is the session's too (ADR 0165, part 2f).
            SendTo::Buffer(buffer) => {
                (table.by_buffer(*buffer)).or_else(|| table.get(self.composer_session(*buffer)?))
            }
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
                let send = self.dispatcher.where_is(&self.editor, "session-send");
                return self.editor.message(too_large(bytes, &send));
            }
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
        let put = self.sent_echo(key, &name);
        self.editor.message(if withheld {
            format!("{put}. Only the place went, as the buffer may hold secrets")
        } else {
            put
        });
    }

    /// [`App::paste_into_session`] of `text`, composed by oso, the session shown and selected
    /// in its space (Q2), point after the text in a native session's composer.
    pub(in crate::app) fn send_to_session(
        &mut self,
        key: SessionKey,
        text: String,
    ) -> Result<(), String> {
        let sent = self.paste_into_session(key, Outgoing::Composed(&text), Shown::Session);
        sent.map(|_| ())
    }

    /// How what was sent to session `key` goes from there: a native session's composer sends
    /// with its own key (ADR 0165, part 2f).
    pub(in crate::app) fn send_hint(&self, key: SessionKey) -> String {
        match self.hosting.natives.has(key) {
            true => {
                let send = self
                    .dispatcher
                    .where_is(&self.editor, "native-composer-send");
                format!("{send} in its composer sends it")
            }
            false => "press Enter there".to_owned(),
        }
    }

    /// What the echo area says once session `key`, named `name`, was sent something.
    pub(in crate::app) fn sent_echo(&self, key: SessionKey, name: &str) -> String {
        match self.hosting.natives.has(key) {
            true => format!("Put into {name}'s composer: {}", self.send_hint(key)),
            false => format!("Pasted into {name}: {}", self.send_hint(key)),
        }
    }

    /// The one delivery path: what is sent to a session goes through here. `text` is escaped
    /// once, as [`Outgoing`] says, then pasted into hosted session `key`'s terminal as the
    /// person's bracketed paste, not submitted, or put into a native session's composer at
    /// point (ADR 0165, part 2f); with [`Shown::Session`] the session is shown and selected
    /// too. Returns the buffer it went into. Refused, with why, while the session cannot take
    /// it: a terminal not running, not judged yet since it started, asking for input, or held
    /// by a full-screen view; a composer that would hold too much or holds secret text.
    pub(in crate::app) fn paste_into_session(
        &mut self,
        key: SessionKey,
        text: Outgoing<'_>,
        shown: Shown,
    ) -> Result<oso_core::buffer::BufferId, String> {
        if self.hosting.natives.has(key) {
            return self.compose_into(key, text, shown == Shown::Session);
        }
        let buffer = self.paste_target(key)?;
        if !self.paste(buffer, text.escaped(None).into_owned()) {
            return Err("Nothing was pasted".to_owned());
        }
        if shown == Shown::Session {
            self.show_session(key);
        }
        Ok(buffer)
    }

    /// Submits what was pasted into `buffer`, session `key`'s terminal: one Enter, as a key of
    /// its own, while the session still takes a paste there; else why not, and nothing is
    /// sent. The only submit to a session: ADR 0163's exception to Q17, for a review's question
    /// the person confirmed in oso's prompt.
    pub(in crate::app) fn submit_paste(
        &mut self,
        key: SessionKey,
        buffer: oso_core::buffer::BufferId,
    ) -> Result<(), String> {
        match self.paste_target(key)? {
            shown if shown == buffer => {
                self.send_text(buffer, "\r");
                Ok(())
            }
            _ => Err("its terminal changed".to_owned()),
        }
    }

    /// The terminal of hosted session `key`, if a paste may go there now; else why not, as
    /// [`App::send_to_session`] refuses it.
    pub(in crate::app) fn paste_target(
        &self,
        key: SessionKey,
    ) -> Result<oso_core::buffer::BufferId, String> {
        let session = self
            .hosting
            .table()
            .get(key)
            .ok_or_else(|| "That session is gone".to_owned())?;
        let name = session.name.clone();
        // A native session takes messages in its composer, which has no terminal
        // (ADR 0165, step 2f): `send_to_session` puts them there.
        if self.hosting.natives.has(key) {
            return Err(format!("{name} is a native session, with no terminal"));
        }
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
        match send::refusal(&name, standing) {
            Some(why) => Err(why),
            None => Ok(buffer),
        }
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
            // A renamed one still running sends under the name Claude Code knows it by (ADR 0155).
            .find(|session| session.name == from || self.messaging_name(session) == from)
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
