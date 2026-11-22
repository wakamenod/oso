//! A native session's composer and queue in the app (ADR 0165, step 2, the author's Q1 (a);
//! ADR 0170): `*compose: NAME*`, an editable buffer withheld from agents, docked under the
//! session's transcript. What it sends goes to the session's standard input, or waits in the
//! queue, whose rules are `sessions::native::queue`'s; this carries out what they come to, and
//! draws the queue's rows grey at the transcript's end, after its rows, which new rows go in
//! above.
//!
//! Must not depend on: key presses or how a frame is drawn; the commands ask, and this carries
//! out.
//!
//! - The session's modeline note is written here alone, from its run's state and the queue.
//! - What the composer holds is measured before it is copied: nothing longer than
//!   [`MAX_MESSAGE`] is copied out of it.

use std::collections::VecDeque;

use oso_core::buffer::{Buffer, BufferId, Mode, Replacement};
use oso_core::editor::NativeAction;
use oso_core::window::DockSlot;

use super::highlight;
use crate::app::App;
use crate::app::sessions::withheld_draft;
use crate::sessions::native::queue::{MAX_MESSAGE, Move, Queue, Refusal, Sent, Turn};
use crate::sessions::native::render::Face;
use crate::sessions::stream::NotSent;
use crate::sessions::worker::SessionKey;

/// The most messages the composer's history keeps.
const MAX_HISTORY: usize = 100;

/// What a native session keeps of its composer.
#[derive(Debug, Default)]
pub(super) struct Composing {
    /// `*compose: NAME*`, once docked.
    pub(super) buffer: Option<BufferId>,
    queue: Queue,
    /// The queue's rows as drawn at the transcript's end: their bytes are the queue's there.
    pub(super) rows: String,
    /// The messages sent, the newest last, and where `M-p` is among them.
    history: VecDeque<String>,
    at: Option<usize>,
    /// A turn is under way: what is sent now is queued.
    pub(super) busy: bool,
    /// What its run says in its modeline before the queue's count: `starting`, `stopped`,
    /// `failed`, or nothing while it runs.
    state: &'static str,
}

impl Composing {
    /// A composer of a session whose turn is under way if `busy`, as its first message's is.
    pub(super) fn starting(busy: bool) -> Self {
        Self {
            busy,
            ..Self::default()
        }
    }
}

impl App {
    /// Makes native session `key`'s composer, `*compose: NAME*`, and docks it under its
    /// transcript `transcript`.
    pub(super) fn dock_composer(&mut self, key: SessionKey, transcript: BufferId, name: &str) {
        let name = format!("*compose: {name}*");
        let id = withheld_draft(&mut self.editor, &name, Mode::Composer, "");
        self.editor
            .set_dock(transcript, DockSlot::Composer, Some(id));
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.composing.buffer = Some(id);
        }
    }

    /// Native session `key`'s composer, docked again if the person killed it.
    fn composer(&mut self, key: SessionKey) -> Option<BufferId> {
        let native = self.hosting.natives.map.get(&key)?;
        let kept = native.composing.buffer;
        if let Some(id) = kept.filter(|id| self.editor.buffer(*id).is_some()) {
            return Some(id);
        }
        let transcript = native.buffer;
        let name = self.hosting.table.get(key)?.name.clone();
        self.dock_composer(key, transcript, &name);
        self.hosting.natives.map.get(&key)?.composing.buffer
    }

    /// The native session whose composer `buffer` is.
    pub(in crate::app) fn composer_session(&self, buffer: BufferId) -> Option<SessionKey> {
        (self.hosting.natives.map.iter())
            .find_map(|(key, native)| (native.composing.buffer == Some(buffer)).then_some(*key))
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
            NativeAction::Send => self.composer_send(key),
            NativeAction::SendNow => self
                .editor
                .message("Interrupting comes with a later step: C-c C-c queues it meanwhile"),
            NativeAction::GiveUp => self.composer_cancel(key),
            NativeAction::History(count) => self.composer_history(key, count),
            NativeAction::Transcript => self.composer_go(key, false),
            NativeAction::Compose => self.composer_go(key, true),
            NativeAction::Queue => self.queue_go(key, 0),
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
            NativeAction::Toggle | NativeAction::Prompt(_) | NativeAction::Tool(_) => {
                return false;
            }
        }
        self.request_redraw();
        true
    }

    /// Where native session `key` is, for the queue's rules.
    fn turn(&self, key: SessionKey) -> Turn {
        let native = self.hosting.natives.map.get(&key);
        Turn {
            busy: native.is_some_and(|native| native.composing.busy),
            running: native.is_some_and(|native| native.stream.is_some()),
            leaving: self.editor.quit_when_idle(),
        }
    }

    /// `C-c C-c`: sends the composer's text now, queues it, or puts an edited queued message
    /// back in its place, as the queue's rules say.
    fn composer_send(&mut self, key: SessionKey) {
        let Some(id) = self.composer(key) else {
            return;
        };
        let Some(text) = self.editor.buffer(id).map(Buffer::text) else {
            return;
        };
        // Measured before it is copied.
        if text.len() > MAX_MESSAGE {
            return self.editor.message("The message is over 64 KiB");
        }
        let text = text.to_string();
        // A blank send writes the next of a paused queue, if anything.
        let blank = text.trim().is_empty();
        let turn = self.turn(key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        match native.composing.queue.send(text, turn) {
            // The person's text stays in the composer if it was not written; the queue's goes
            // back first.
            Sent::Now(text) => match (self.deliver(key, &text), blank) {
                (true, false) => self.set_composer_text(key, ""),
                (false, true) => self.requeue(key, text),
                _ => {}
            },
            Sent::Queued(count) => {
                self.set_composer_text(key, "");
                let note = if turn.running {
                    "C-c C-q goes to the queue"
                } else {
                    "it goes once the session runs"
                };
                self.editor.message(format!("Queued as {count}: {note}"));
            }
            Sent::PutBack => {
                self.set_composer_text(key, "");
                // The turn may have ended during the edit, with the first held back.
                return self.send_queued(key);
            }
            Sent::Resumed => self.editor.message("The queue goes on once the turn ends"),
            Sent::Refused(Refusal::Empty) => self.editor.message("The message is empty"),
            Sent::Refused(Refusal::TooLong) => self.editor.message("The message is over 64 KiB"),
            Sent::Refused(Refusal::Full) => self
                .editor
                .message("The queue is full: wait for a turn to end"),
        }
        self.draw_queue(key);
    }

    /// Writes `text` to native session `key` as the person's message. Whether it went.
    fn deliver(&mut self, key: SessionKey, text: &str) -> bool {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return false;
        };
        let Some(stream) = &native.stream else {
            return false;
        };
        match stream.send(text) {
            Ok(()) => {
                native.rows.sent(text);
                native.composing.busy = true;
                let history = &mut native.composing.history;
                if history.len() == MAX_HISTORY {
                    history.pop_front();
                }
                history.push_back(text.to_owned());
                native.composing.at = None;
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

    /// `text` was not written: it goes back first in the queue, which waits for the person.
    fn requeue(&mut self, key: SessionKey, text: String) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.composing.queue.failed(text);
        }
    }

    /// Native session `key` may be idle: the queue's first goes, if the rules say so.
    pub(super) fn send_queued(&mut self, key: SessionKey) {
        let turn = self.turn(key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        if let Some(text) = native.composing.queue.idle(turn)
            && !self.deliver(key, &text)
        {
            self.requeue(key, text);
        }
        self.draw_queue(key);
    }

    /// After each event: a queue that may go and has not, as when oso no longer waits to exit
    /// or the turn ended while its first was edited, sends its first.
    pub(super) fn send_waiting_queues(&mut self) {
        let leaving = self.editor.quit_when_idle();
        let ready: Vec<SessionKey> = (self.hosting.natives.map.iter())
            .filter(|(_, native)| {
                !leaving
                    && !native.composing.busy
                    && native.stream.is_some()
                    && !native.composing.queue.is_empty()
            })
            .map(|(key, _)| *key)
            .collect();
        for key in ready {
            self.send_queued(key);
        }
    }

    /// Why a restart cannot hand over now: a native session's queue holds messages not sent
    /// (ADR 0165, step 2, the author's choice), which the restart would lose. The composer's
    /// own text is the carry's to refuse.
    pub(in crate::app) fn unsent_queue(&self) -> Result<(), String> {
        let waiting =
            (self.hosting.natives.map.values()).find(|native| !native.composing.queue.is_empty());
        let Some(native) = waiting else {
            return Ok(());
        };
        let name = self.editor.buffer(native.buffer).map_or("", Buffer::name);
        let count = native.composing.queue.waiting().len();
        Err(format!(
            "cannot hand over {name}: {count} queued message(s) not sent; send or drop them"
        ))
    }

    /// Native session `key`'s process ended: no turn runs, and the queue waits for the person.
    pub(super) fn composer_ended(&mut self, key: SessionKey) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.composing.busy = false;
            native.composing.queue.ended();
        }
        self.draw_queue(key);
    }

    /// `C-c C-k`: gives up the edit of a queued message.
    fn composer_cancel(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        if native.composing.queue.editing().is_none() {
            return self.editor.message("Interrupting comes with a later step");
        }
        native.composing.queue.give_up_edit();
        self.set_composer_text(key, "");
        // The turn may have ended during the edit, with the first held back.
        self.send_queued(key);
    }

    /// `M-p` and `M-n`: the message sent `count` before the one shown, after if negative.
    fn composer_history(&mut self, key: SessionKey, count: isize) {
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
    fn composer_go(&mut self, key: SessionKey, to_composer: bool) {
        let Some(transcript) = self
            .hosting
            .natives
            .map
            .get(&key)
            .map(|native| native.buffer)
        else {
            return;
        };
        let composer = self.composer(key);
        let docked = composer.and(self.editor.dock_window(transcript, DockSlot::Composer));
        let shown = docked.and_then(|id| Some((id, self.editor.regions(id)?.text)));
        if to_composer {
            return match shown {
                Some((id, _)) => self.editor.select_window(id),
                None => self
                    .editor
                    .message("The composer is hidden: the window is too short for it"),
            };
        }
        // The window that carries the dock holds the composer's rows inside its own.
        let carrier = (self.editor.frame().windows().iter()).find_map(|(id, regions)| {
            let window = self.editor.window_by_id(*id)?;
            let holds = shown.is_none_or(|(_, inside)| {
                regions.text.row <= inside.row && inside.row < regions.modeline.row
            });
            (window.docked().is_none() && window.buffer() == transcript && holds).then_some(*id)
        });
        match carrier {
            Some(id) => self.editor.select_window(id),
            None => self.editor.show(transcript),
        }
    }

    /// Puts `text` in native session `key`'s composer, in place of what is there.
    fn set_composer_text(&mut self, key: SessionKey, text: &str) {
        let Some(id) = self.composer(key) else {
            return;
        };
        let Some(len) = self.editor.buffer(id).map(|buffer| buffer.text().len()) else {
            return;
        };
        if len == 0 && text.is_empty() {
            return;
        }
        let edit = Replacement {
            range: 0..len,
            text,
        };
        let _ = self.editor.apply_edits(id, &[edit]);
    }

    /// Whether native session `key`'s composer holds nothing, read without a copy.
    fn composer_empty(&self, key: SessionKey) -> bool {
        let id = (self.hosting.natives.map.get(&key)).and_then(|native| native.composing.buffer);
        id.and_then(|id| self.editor.buffer(id))
            .is_none_or(|buffer| buffer.text().is_empty())
    }

    /// Native session `key`'s run state for its modeline: `starting`, `stopped`, `failed`, or
    /// nothing while it runs.
    pub(super) fn set_native_state(&mut self, key: SessionKey, state: &'static str) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.composing.state = state;
        }
        self.write_native_note(key);
    }

    /// Native session `key`'s modeline note: its run's state, and how many wait.
    fn write_native_note(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        let count = native.composing.queue.waiting().len();
        let state = native.composing.state;
        let note = match (state, count) {
            (state, 0) => state.to_owned(),
            ("", count) => format!("{count} queued"),
            (state, count) => format!("{state} · {count} queued"),
        };
        self.editor.set_mode_note(native.buffer, note);
    }

    /// Draws native session `key`'s queue as grey rows at its transcript's end, when they
    /// changed, and says how many wait in its modeline.
    pub(super) fn draw_queue(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let rows = native.composing.queue.rows();
        if rows != native.composing.rows {
            let start = native.placed.end();
            let range = start..start + native.composing.rows.len();
            let runs = [(rows.len(), Some(highlight(Face::Dim)))];
            self.editor
                .replace_native_rows(native.buffer, range, &rows, &runs);
            native.composing.rows = rows;
        }
        self.write_native_note(key);
    }

    /// The queued message whose row is at `offset` of native session `key`'s transcript; the
    /// queue's heading names the first.
    fn queued_at(&self, key: SessionKey, offset: usize) -> Option<u64> {
        let native = self.hosting.natives.map.get(&key)?;
        let start = native.placed.end();
        if offset < start || native.composing.queue.is_empty() {
            return None;
        }
        let text = self.editor.buffer(native.buffer)?.text();
        let line = text.offset_to_point(offset).row - text.offset_to_point(start).row;
        let at = line.saturating_sub(1);
        (native.composing.queue.waiting())
            .nth(at)
            .map(|queued| queued.id)
    }

    /// `C-c C-q`: point to the queue's row `at`, from 0, in the transcript.
    fn queue_go(&mut self, key: SessionKey, at: usize) {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        if native.composing.queue.is_empty() {
            return self.editor.message("Nothing is queued");
        }
        let (transcript, start) = (native.buffer, native.placed.end());
        if self.editor.current().0.buffer() != transcript {
            self.composer_go(key, false);
        }
        if self.editor.current().0.buffer() != transcript {
            return;
        }
        let Some(row) = self.editor.buffer(transcript).map(|buffer| {
            let text = buffer.text();
            text.line_start(text.offset_to_point(start).row + 1 + at)
        }) else {
            return;
        };
        self.editor.current_mut().0.selections.move_primary(row);
    }

    /// `RET` on a queued row: its message goes back into the composer, above what is there.
    fn queued_take_back(&mut self, key: SessionKey, id: u64) {
        let empty = self.composer_empty(key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let Some(mut taken) = native.composing.queue.take_back(id) else {
            return;
        };
        if !empty {
            taken.push_str("\n\n");
        }
        if let Some(composer) = self.composer(key) {
            let edit = Replacement {
                range: 0..0,
                text: &taken,
            };
            let _ = self.editor.apply_edits(composer, &[edit]);
        }
        self.draw_queue(key);
        self.composer_go(key, true);
    }

    /// `e`, `d`, `M-<up>` and `M-<down>` on queued message `id`.
    fn queued_act(&mut self, key: SessionKey, id: u64, action: NativeAction) {
        let empty = self.composer_empty(key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let queue = &mut native.composing.queue;
        match action {
            NativeAction::EditQueued if !empty => self
                .editor
                .message("The composer holds a message: send or clear it first"),
            NativeAction::EditQueued => {
                let Some(text) = queue.edit(id) else {
                    return;
                };
                self.set_composer_text(key, &text);
                self.draw_queue(key);
                self.composer_go(key, true);
            }
            NativeAction::DropQueued => {
                queue.drop_one(id);
                self.draw_queue(key);
            }
            NativeAction::MoveQueued(count) => {
                let way = if count < 0 { Move::Up } else { Move::Down };
                let mut place = None;
                for _ in 0..count.unsigned_abs() {
                    match queue.shift(id, way) {
                        Some(at) => place = Some(at),
                        None => break,
                    }
                }
                self.draw_queue(key);
                // Point stays on the message moved.
                if let Some(at) = place {
                    self.queue_go(key, at);
                }
            }
            _ => {}
        }
    }
}
