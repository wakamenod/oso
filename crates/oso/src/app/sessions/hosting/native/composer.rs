//! A native session's composer in the app (ADR 0165, step 2, the author's Q1 (a); ADR 0170):
//! `*compose: NAME*`, an editable buffer withheld from agents, docked under the session's
//! transcript. What it sends goes to the session's standard input, or waits in the queue,
//! whose rules are `sessions::native::queue`'s: this carries out what they come to, and
//! `queued` draws the queue and acts on its rows.
//!
//! Must not depend on: key presses or how a frame is drawn; the commands ask, and this carries
//! out.
//!
//! - What the composer holds is measured before it is copied: nothing longer than
//!   [`MAX_MESSAGE`] is copied out of it, and nothing is sent from a buffer that is no longer
//!   in `Composer` mode or that a yank turned secret.
//! - The queue goes from one place, [`App::send_waiting_queues`], after each event.
//! - What oso sends a native session is put into the composer, or a review's confirmed
//!   question sent, by `composed` (part 2f).
//! - A send while a dictation's fix runs waits for it, in `held` (ADR 0175, step 2).
//! - What an empty composer's placeholder says is `placeholder`'s (ADR 0170).

mod composed;
mod held;
mod placeholder;
mod queued;

pub(in crate::app) use placeholder::Placeholder;

use std::collections::VecDeque;

use oso_core::buffer::{BufferId, Mode, Replacement};
use oso_core::editor::{Editor, NativeAction};
use oso_core::window::{DockSlot, WindowId};

use super::NativeSessions;
use super::carrier::carrier;
use super::interrupting::Interrupted;
use super::note::NativeState;
use crate::app::App;
use crate::app::sessions::withheld_draft;
use crate::sessions::native::queue::{MAX_MESSAGE, Queue, Refusal, Run, Sent, Turn};
use crate::sessions::state::Target;
use crate::sessions::stream::NotSent;
use crate::sessions::worker::{Job, SessionKey};

/// The most messages the composer's history keeps.
const MAX_HISTORY: usize = 100;

/// What a key to a composer that is not shown says.
const HIDDEN: &str = "The composer is hidden: the window is too short for it";

/// What a native session keeps of its composer.
#[derive(Debug, Default)]
pub(super) struct Composing {
    /// `*compose: NAME*`, once docked.
    pub(super) buffer: Option<BufferId>,
    queue: Queue,
    /// The messages of the queue's rows as drawn, so that point stays on its own.
    drawn: Vec<u64>,
    /// The CLI's prompt suggestion since its turn ended, until a turn starts, the person
    /// sends or the process ends (part 3f), which its empty composer's placeholder shows.
    pub(super) suggestion: Option<String>,
    /// The messages sent, the newest last, and where `M-p` is among them.
    history: VecDeque<String>,
    at: Option<usize>,
    /// A turn is under way: what is sent now is queued.
    pub(super) busy: bool,
    /// A send that waits for a dictation's fix (ADR 0175, step 2).
    held: Option<held::Held>,
}

impl Composing {
    /// A composer of a session whose turn is under way if `busy`, as its first message's is.
    pub(super) fn starting(busy: bool) -> Self {
        Self {
            busy,
            ..Self::default()
        }
    }

    /// How many messages wait.
    pub(super) fn queued(&self) -> usize {
        self.queue.waiting().len()
    }

    /// The process runs again after it ended: the queue waits for an empty `C-c C-c`.
    pub(super) fn resumed(&mut self) {
        self.queue.resumed();
    }

    /// The person interrupted the turn: the queue waits for them (step 2c).
    pub(super) fn interrupted(&mut self) {
        self.queue.interrupted();
    }
}

/// A new composer of the session named `name`, docked under its transcript `transcript`. Its
/// long lines wrap, as its window's height counts them (ADR 0170).
pub(super) fn new_docked_composer(
    editor: &mut Editor,
    transcript: BufferId,
    name: &str,
) -> BufferId {
    let name = format!("*compose: {name}*");
    let id = withheld_draft(editor, &name, Mode::Composer, "");
    editor.set_wraps(id, true);
    editor.set_dock(transcript, DockSlot::Composer, Some(id));
    id
}

impl NativeSessions {
    /// The session whose composer `buffer` is.
    fn composer_session(&self, buffer: BufferId) -> Option<SessionKey> {
        (self.map.iter())
            .find_map(|(key, native)| (native.composing.buffer == Some(buffer)).then_some(*key))
    }
}

impl App {
    /// Makes native session `key`'s composer, `*compose: NAME*`, and docks it under its
    /// transcript `transcript`.
    pub(super) fn dock_composer(&mut self, key: SessionKey, transcript: BufferId, name: &str) {
        let id = new_docked_composer(&mut self.editor, transcript, name);
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.composing.buffer = Some(id);
        }
    }

    /// Native session `key`'s composer, docked again if the person killed it; the edit of a
    /// queued message it held is given up.
    pub(super) fn composer(&mut self, key: SessionKey) -> Option<BufferId> {
        let native = self.hosting.natives.map.get_mut(&key)?;
        let kept = native.composing.buffer;
        if let Some(id) = kept.filter(|id| self.editor.buffer(*id).is_some()) {
            return Some(id);
        }
        native.composing.queue.give_up_edit();
        let transcript = native.buffer;
        let name = self.hosting.table.get(key)?.name.clone();
        self.dock_composer(key, transcript, &name);
        self.draw_queue(key);
        self.hosting.natives.map.get(&key)?.composing.buffer
    }

    /// The native session whose composer `buffer` is.
    pub(in crate::app) fn composer_session(&self, buffer: BufferId) -> Option<SessionKey> {
        self.hosting.natives.composer_session(buffer)
    }

    /// Carries out a composer's or a queue's `action` for native session `key`, at `offset`
    /// of its transcript when the key was there. Whether `action` is one of them.
    pub(super) fn composer_key(
        &mut self,
        key: SessionKey,
        offset: Option<usize>,
        action: NativeAction,
    ) -> bool {
        match action {
            // While a dictation's fix runs, the send waits for it to end, and then sends the
            // fixed text (ADR 0175, step 2).
            NativeAction::Send | NativeAction::SendNow if self.hold_send_for_fix(key) => {}
            NativeAction::Send => self.composer_send(key, false),
            NativeAction::SendNow => self.composer_send(key, true),
            // From the transcript, an interrupt alone (step 2c).
            NativeAction::Cancel if offset.is_some() => self.person_interrupts(key),
            // A send that waits for a fix is dropped, and nothing more.
            NativeAction::Cancel if self.drop_held_send(key) => {}
            NativeAction::Cancel => self.composer_cancel(key),
            NativeAction::History(count) => self.composer_history(key, count),
            NativeAction::Transcript => self.composer_go(key, false),
            NativeAction::Compose => self.composer_go(key, true),
            NativeAction::Request => self.go_to_request(key),
            NativeAction::Queue => self.queue_go(key, 0),
            NativeAction::Suggestion => self.suggestion_to_composer(key),
            // The transcript's and the composer's `S-TAB` (part 3b).
            NativeAction::CycleMode => self.native_cycle_mode(key),
            NativeAction::Open => match offset.and_then(|at| self.queued_at(key, at)) {
                Some(id) => self.queued_take_back(key, id),
                None => return false,
            },
            NativeAction::EditQueued | NativeAction::DropQueued | NativeAction::MoveQueued(_) => {
                match offset.and_then(|at| self.queued_at(key, at)) {
                    Some(id) => self.queued_act(key, id, action),
                    None => self.editor.message("No queued message here"),
                }
            }
            NativeAction::Toggle
            | NativeAction::CopyMarkdown
            | NativeAction::Prompt(_)
            | NativeAction::Tool(_)
            | NativeAction::Allow
            | NativeAction::Deny
            | NativeAction::Note
            | NativeAction::Choose(_)
            | NativeAction::Other
            | NativeAction::Page(_)
            | NativeAction::ReadOn(_)
            | NativeAction::Unread
            | NativeAction::Todo
            | NativeAction::EditPlan
            | NativeAction::Approve(_)
            | NativeAction::Confirmed
            | NativeAction::RemoteControl
            | NativeAction::RemoteConfirmed
            | NativeAction::RemoteAddress => return false,
        }
        self.request_redraw();
        true
    }

    /// Where native session `key` is, for the queue's rules.
    fn turn(&self, key: SessionKey) -> Option<Turn> {
        let native = self.hosting.natives.map.get(&key)?;
        let run = match (&native.stream, native.state) {
            (Some(_), _) => Run::Running,
            (None, NativeState::Starting) => Run::Starting,
            (None, _) => Run::Ended,
        };
        Some(Turn {
            busy: native.composing.busy,
            run,
            // An exit or a restart that waits for the sessions, or a restart under way, sends
            // nothing queued (part 4b).
            leaving: self.editor.quit_when_idle()
                || self.editor.restart_when_idle()
                || self.restart_holds(),
        })
    }

    /// `C-c C-c`: sends the composer's text now, queues it, or puts an edited queued message
    /// back in its place, as the queue's rules say. `now`, `C-u C-c C-c`: puts it ahead of
    /// the queue and interrupts the turn under way (step 2c).
    pub(in crate::app) fn composer_send(&mut self, key: SessionKey, now: bool) {
        let Some(id) = self.composer(key) else {
            return;
        };
        let Some(buffer) = self.editor.buffer(id) else {
            return;
        };
        if buffer.mode() != Mode::Composer {
            return self.editor.message("This buffer is no longer the composer");
        }
        // A yank of a secret entry turns the message secret: it stays here, as a review
        // message's does (ADR 0122).
        if buffer.is_secret() {
            return self
                .editor
                .message("The message holds secret text, which is not sent: take it out");
        }
        // Measured before it is copied.
        if buffer.text().len() > MAX_MESSAGE {
            return self.editor.message("The message is over 64 KiB");
        }
        let text = buffer.text().to_string();
        // `/clear` would leave the conversation oso follows (step 2d, Q2 (a)).
        let refused = (self.hosting.natives.map.get(&key)).and_then(|n| n.commands.refused(&text));
        if let Some(why) = refused {
            return self.editor.message(why);
        }
        let Some(turn) = self.turn(key) else {
            return;
        };
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let sent = match now {
            true => native.composing.queue.send_now(text, turn),
            false => native.composing.queue.send(text, turn),
        };
        // Sent or queued: the window it was written in follows again.
        let mut went = true;
        match sent {
            Sent::Now(text) if self.deliver(key, &text) => self.set_composer_text(key, ""),
            // The person's text stays in the composer, and the queue waits for the person.
            Sent::Now(_) => {
                went = false;
                self.not_sent(key, None);
            }
            Sent::Next(text) if !self.deliver(key, &text) => {
                went = false;
                self.not_sent(key, Some(text));
            }
            Sent::Next(_) => {}
            Sent::Resumed => {
                went = false;
                self.editor.message("The queue goes on once the turn ends");
            }
            Sent::Queued(count) => {
                self.set_composer_text(key, "");
                let when = match turn.run {
                    Run::Running => "C-c C-q goes to the queue",
                    Run::Starting if turn.busy => "it goes once the first turn ends",
                    Run::Starting => "it goes once the session runs",
                    Run::Ended => "the session stopped, so it waits",
                };
                self.editor.message(format!("Queued as {count}: {when}"));
            }
            // It goes first, the queue not paused: a send now is a send (Q1 (a)).
            Sent::Ahead(_) => {
                self.set_composer_text(key, "");
                if self.native_interrupt(key, false) != Interrupted::Yes {
                    self.editor.message("It goes first, once the turn ends");
                }
            }
            Sent::PutBack => {
                went = false;
                self.set_composer_text(key, "");
            }
            Sent::Refused(why) => {
                went = false;
                self.editor.message(not_queued(why));
            }
        }
        self.draw_queue(key);
        if went {
            self.transcript_to_end(key);
        }
    }

    /// Puts the window that carries native session `key`'s composer at the end of its
    /// transcript, with its mark, so that it follows the conversation.
    fn transcript_to_end(&mut self, key: SessionKey) {
        let Some(transcript) = self.hosting.natives.map.get(&key).map(|n| n.buffer) else {
            return;
        };
        let end = (self.editor.buffer(transcript)).map(|buffer| buffer.text().len());
        let carrier = self.transcript_carrier(key, transcript);
        let window = carrier.and_then(|id| self.editor.window_by_id(id));
        if let (Some(end), Some(window)) = (end, window) {
            let mut selections = window.selections.clone();
            selections.keep_primary();
            selections.move_primary(end);
            let (id, scroll) = (window.id(), window.scroll());
            self.editor.restore_positions_in(id, selections, scroll);
        }
    }

    /// The window that carries native session `key`'s composer, which shows `transcript`. With the
    /// composer hidden, the selected window, if it shows it.
    fn transcript_carrier(&mut self, key: SessionKey, transcript: BufferId) -> Option<WindowId> {
        self.composer(key);
        // As its request's line has its place, that window is the one (ADR 0182).
        let carried = [DockSlot::Composer, DockSlot::Card]
            .into_iter()
            .find_map(|slot| carrier(&self.editor, transcript, slot))
            .map(|(id, _)| id);
        carried.or_else(|| {
            let (selected, _) = self.editor.tiled();
            (selected.buffer() == transcript).then(|| selected.id())
        })
    }

    /// Writes `text` to native session `key` as the person's message. Whether it went.
    fn deliver(&mut self, key: SessionKey, text: &str) -> bool {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return false;
        };
        let Some(stream) = &native.stream else {
            return false;
        };
        // Every message passes here: one queued before the list named an alias too.
        if let Some(why) = native.commands.refused(text) {
            self.editor.message(why);
            return false;
        }
        match stream.send(text) {
            Ok(()) => {
                native.composing.busy = true;
                // The CLI replays no command it runs: the person's row is shown now (step 2d).
                // Any other message shows as being sent, until its replay.
                let change = match native.commands.runs(text) {
                    true => Some(native.rows.sent_command(text)),
                    false => native.rows.sent(text),
                };
                super::draw(&mut self.editor, native, change);
                // A `/model` or `/effort` is noted once its turn's result says it was taken.
                native.choice.sent(&native.commands, text);
                // So that an exit asks about it at once, before the stream replays it (ADR
                // 0133); a local command's result ends it again.
                if let Some(worker) = &self.sessions {
                    worker.send(Job::Stream(key, Target::Working));
                }
                native.spoke = true;
                native.composing.suggestion = None;
                let history = &mut native.composing.history;
                if history.len() == MAX_HISTORY {
                    history.pop_front();
                }
                history.push_back(text.to_owned());
                native.composing.at = None;
                // A turn starts: an exit or a restart that waits for the sessions waits for
                // it too.
                self.idle_quit.restart();
                self.restart.wait.restart();
                true
            }
            Err(NotSent::Full) => {
                self.editor
                    .message("The session is not reading what it is sent; nothing is sent");
                false
            }
            Err(NotSent::Closed) => {
                self.editor.message("The session's input is closed");
                false
            }
        }
    }

    /// What the queue gave was not written: the queue's own (`Some`) goes back first, and the
    /// queue waits for the person.
    fn not_sent(&mut self, key: SessionKey, queued: Option<String>) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.composing.queue.failed(queued);
        }
    }

    /// After each event: each native session that is idle, with messages waiting the rules let
    /// go, is written its queue's first.
    pub(super) fn send_waiting_queues(&mut self) {
        let waiting: Vec<SessionKey> = (self.hosting.natives.map.iter())
            .filter(|(_, native)| !native.composing.queue.is_empty())
            .map(|(key, _)| *key)
            .collect();
        for key in waiting {
            let Some(turn) = self.turn(key) else {
                continue;
            };
            let Some(native) = self.hosting.natives.map.get_mut(&key) else {
                continue;
            };
            let Some(text) = native.composing.queue.idle(turn) else {
                continue;
            };
            // Refused, it goes back first and the queue waits: the person drops it.
            if native.commands.refused(&text).is_some() {
                native.composing.queue.failed(Some(text));
                self.editor.message(
                    "The first queued message would /clear, which oso does not follow: d on its row drops it",
                );
                self.draw_queue(key);
                continue;
            }
            let buffer = native.buffer;
            self.editor.refill_wraps();
            if !self.deliver(key, &text) {
                self.not_sent(key, Some(text));
            }
            self.draw_queue(key);
            if self
                .editor
                .windows()
                .any(|window| window.buffer() == buffer)
            {
                self.request_redraw();
            }
        }
    }

    /// Native session `key`'s process ended: no turn runs, and the queue waits for the person.
    pub(super) fn composer_ended(&mut self, key: SessionKey) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.composing.busy = false;
            native.composing.queue.ended();
            native.composing.suggestion = None;
        }
        self.draw_queue(key);
    }

    /// Native session `key`'s process ended to resume: no turn runs, and the queue stays as
    /// the interrupt left it (step 2c).
    pub(super) fn composer_idle(&mut self, key: SessionKey) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.composing.busy = false;
        }
    }

    /// `C-c C-k`: gives up the edit of a queued message, or else interrupts the turn under
    /// way (step 2c).
    fn composer_cancel(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        if native.composing.queue.editing().is_none() {
            return self.person_interrupts(key);
        }
        native.composing.queue.give_up_edit();
        self.set_composer_text(key, "");
        self.draw_queue(key);
    }

    /// The person's `C-c C-k` with no edit to give up, from the composer or the transcript:
    /// the turn under way is interrupted, and the queue waits for them.
    fn person_interrupts(&mut self, key: SessionKey) {
        if self.native_interrupt(key, true) == Interrupted::NoTurn {
            self.editor.message("No turn runs to interrupt");
        }
    }

    /// `M-p` and `M-n`: the message sent `count` before the one shown, after if negative;
    /// `M-n` past the newest, in an empty composer, puts the suggestion in (part 3f).
    fn composer_history(&mut self, key: SessionKey, count: isize) {
        let past_newest =
            (self.hosting.natives.map.get(&key)).is_some_and(|n| n.composing.at.is_none());
        if count < 0 && past_newest && self.take_suggestion(key) {
            return;
        }
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let history = &native.composing.history;
        if history.is_empty() {
            return self.editor.message("Nothing sent yet");
        }
        let newest = history.len() - 1;
        let from = native.composing.at.unwrap_or(newest + 1);
        let to = from.saturating_add_signed(-count).min(newest + 1);
        native.composing.at = (to <= newest).then_some(to);
        let text = history.get(to).cloned().unwrap_or_default();
        self.set_composer_text(key, &text);
    }

    /// `i` and `C-c C-o`: to the composer, or back to the window that carries it.
    pub(super) fn composer_go(&mut self, key: SessionKey, to_composer: bool) {
        let Some(transcript) = self
            .hosting
            .natives
            .map
            .get(&key)
            .map(|native| native.buffer)
        else {
            return;
        };
        // It takes its place back from a request's line (ADR 0182).
        if to_composer && !self.composer_back_for_point(key) {
            return self.editor.message(HIDDEN);
        }
        let composer = self.composer(key);
        let docked = composer.and(self.editor.dock_window(transcript, DockSlot::Composer));
        let shown = docked.and_then(|id| Some((id, self.editor.regions(id)?.text)));
        if to_composer {
            return match shown {
                Some((id, _)) => self.editor.select_window(id),
                None => self.editor.message(HIDDEN),
            };
        }
        match self.transcript_carrier(key, transcript) {
            Some(id) => self.editor.select_window(id),
            None => self.editor.show(transcript),
        }
    }

    /// Puts `text` in native session `key`'s composer, in place of what is there.
    pub(super) fn set_composer_text(&mut self, key: SessionKey, text: &str) {
        let Some(id) = self.composer(key) else {
            return;
        };
        let Some(len) = self.editor.buffer(id).map(|buffer| buffer.text().len()) else {
            return;
        };
        if len == 0 && text.is_empty() {
            return;
        }
        self.composer_back(key);
        let edit = Replacement {
            range: 0..len,
            text,
        };
        let _ = self.editor.apply_edits(id, &[edit]);
    }

    /// Whether native session `key`'s composer holds nothing, read without a copy.
    pub(super) fn composer_empty(&self, key: SessionKey) -> bool {
        let id = (self.hosting.natives.map.get(&key)).and_then(|native| native.composing.buffer);
        id.and_then(|id| self.editor.buffer(id))
            .is_none_or(|buffer| buffer.text().is_empty())
    }
}

/// What `C-c C-c` says as the queue refuses a message.
fn not_queued(why: Refusal) -> &'static str {
    match why {
        Refusal::Empty => "The message is empty",
        Refusal::TooLong => "The message is over 64 KiB",
        Refusal::Full => "The queue is full: wait for a turn to end",
        Refusal::Editing => "A queued message is edited here: C-c C-c puts it back first",
    }
}

#[cfg(test)]
mod tests;
