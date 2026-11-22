//! A native session's composer and queue in the app (ADR 0165, step 2, the author's Q1 (a);
//! ADR 0170): `*compose: NAME*`, an editable buffer withheld from agents, docked under the
//! session's transcript; what it sends goes to the session's standard input while the session
//! is idle, and waits in the queue while a turn runs, its rows grey at the transcript's end.
//!
//! Must not depend on: key presses or how a frame is drawn; the commands ask, and this carries
//! out.
//!
//! - oso writes to the session only while it is idle: one message a turn, the queue's first,
//!   on each turn's end. A turn oso did not start, as a peer's message, counts as one.
//! - The queue pauses when the process ends: nothing queued goes until the person sends again,
//!   or sends with an empty composer, which sends the next.
//! - The queue's rows sit after the transcript's rows, which new rows go in above.

use std::collections::VecDeque;

use oso_core::buffer::{Buffer, BufferId, Mode, Replacement};
use oso_core::editor::NativeAction;
use oso_core::window::DockSlot;

use super::highlight;
use crate::app::App;
use crate::sessions::native::queue::{Move, Queue};
use crate::sessions::native::render::Face;
use crate::sessions::stream::NotSent;
use crate::sessions::worker::SessionKey;
use crate::sessions::worktree::brief::MAX_BRIEF;

/// The most messages the composer's history keeps.
const MAX_HISTORY: usize = 100;

/// The most of a queued message's first line its row shows, in characters.
const QUEUED_SHOWN: usize = 72;

/// What a native session keeps of its composer.
#[derive(Debug, Default)]
pub(super) struct Composing {
    /// `*compose: NAME*`, once docked.
    pub(super) buffer: Option<BufferId>,
    queue: Queue,
    /// The bytes the queue's rows take at the transcript's end.
    pub(super) queue_len: usize,
    /// The messages sent, the newest last, and where `M-p` is among them.
    history: VecDeque<String>,
    at: Option<usize>,
    /// A turn is under way: what is sent now is queued.
    pub(super) busy: bool,
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
        let mut buffer = Buffer::from_text(format!("*compose: {name}*"), "");
        buffer.set_mode(Mode::Composer);
        // The person's message: no agent reads it before it is sent (ADR 0165).
        buffer.withhold_from_agents();
        let id = self.editor.add_buffer(buffer);
        self.editor
            .set_dock(transcript, DockSlot::Composer, Some(id));
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.composing.buffer = Some(id);
        }
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
            NativeAction::GiveUp => self.composer_give_up(key),
            NativeAction::History(count) => self.composer_history(key, count),
            NativeAction::Transcript => self.composer_go(key, false),
            NativeAction::Compose => self.composer_go(key, true),
            NativeAction::Queue => self.queue_go(key),
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

    /// `C-c C-c`: sends the composer's text, queues it while a turn runs, or puts an edited
    /// queued message back in its place. An empty composer sends the next of a paused queue.
    fn composer_send(&mut self, key: SessionKey) {
        let Some(text) = self.composer_text(key) else {
            return;
        };
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        if native.composing.queue.editing().is_some() {
            native.composing.queue.put_back(text);
            self.set_composer_text(key, "");
            return self.draw_queue(key);
        }
        if text.trim().is_empty() {
            if native.composing.queue.paused() {
                native.composing.queue.resume();
                return self.send_queued(key);
            }
            return self.editor.message("The message is empty");
        }
        if text.len() > MAX_BRIEF {
            return self.editor.message("The message is over 64 KiB");
        }
        native.composing.queue.resume();
        if !native.composing.busy && self.deliver(key, &text) {
            self.set_composer_text(key, "");
            return;
        }
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        match native.composing.queue.push(text) {
            Ok(_) => {
                let count = native.composing.queue.waiting().len();
                self.set_composer_text(key, "");
                self.draw_queue(key);
                self.editor
                    .message(format!("Queued as {count}: C-c C-q goes to the queue"));
            }
            Err(_) => self
                .editor
                .message("The queue is full: wait for a turn to end"),
        }
    }

    /// Writes `text` to native session `key` as the person's message, now that it is idle.
    /// Whether it went.
    fn deliver(&mut self, key: SessionKey, text: &str) -> bool {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return false;
        };
        let Some(stream) = &native.stream else {
            self.editor
                .message("The session is not running: it takes the message once resumed");
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

    /// Native session `key` is idle: the queue's first goes, unless it is paused.
    pub(super) fn send_queued(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        native.composing.busy = false;
        let Some(text) = native.composing.queue.next(true) else {
            return self.draw_queue(key);
        };
        if !self.deliver(key, &text)
            && let Some(native) = self.hosting.natives.map.get_mut(&key)
        {
            // It keeps its place, and the queue waits for the person.
            let _ = native.composing.queue.push(text);
            native.composing.queue.pause();
        }
        self.draw_queue(key);
    }

    /// Native session `key`'s process ended: no turn runs, and the queue waits for the person.
    pub(super) fn composer_ended(&mut self, key: SessionKey) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.composing.busy = false;
            native.composing.queue.pause();
        }
        self.draw_queue(key);
    }

    /// `C-c C-k`: gives up the edit of a queued message.
    fn composer_give_up(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        if native.composing.queue.editing().is_none() {
            return self.editor.message("Interrupting comes with a later step");
        }
        native.composing.queue.give_up_edit();
        self.set_composer_text(key, "");
        self.draw_queue(key);
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
        let from = native.composing.at.map_or(newest + 1, |at| at);
        let to = from.saturating_add_signed(-count).min(newest + 1);
        native.composing.at = (to <= newest).then_some(to);
        let text = native
            .composing
            .history
            .get(to)
            .cloned()
            .unwrap_or_default();
        self.set_composer_text(key, &text);
    }

    /// `i` and `C-c C-o`: to the composer, or back to the transcript.
    fn composer_go(&mut self, key: SessionKey, to_composer: bool) {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        let transcript = native.buffer;
        let target = if to_composer {
            self.editor.dock_window(transcript, DockSlot::Composer)
        } else {
            (self.editor.frame().windows().iter())
                .map(|(id, _)| *id)
                .find(|id| {
                    (self.editor.windows())
                        .any(|window| window.id() == *id && window.buffer() == transcript)
                })
        };
        let shown = target.filter(|id| self.editor.regions(*id).is_some());
        match shown {
            Some(id) => self.editor.select_window(id),
            None if to_composer => self
                .editor
                .message("The composer is hidden: the window is too short for it"),
            None => self.editor.show(transcript),
        }
    }

    /// The composer's text, as it is.
    fn composer_text(&self, key: SessionKey) -> Option<String> {
        let id = self.hosting.natives.map.get(&key)?.composing.buffer?;
        self.editor
            .buffer(id)
            .map(|buffer| buffer.text().to_string())
    }

    /// Puts `text` in native session `key`'s composer, in place of what is there.
    fn set_composer_text(&mut self, key: SessionKey, text: &str) {
        let Some(id) = (self.hosting.natives.map.get(&key)).and_then(|n| n.composing.buffer) else {
            return;
        };
        let Some(len) = self.editor.buffer(id).map(|buffer| buffer.text().len()) else {
            return;
        };
        let edit = Replacement {
            range: 0..len,
            text,
        };
        let _ = self.editor.apply_edits(id, &[edit]);
    }

    /// Draws native session `key`'s queue as grey rows at its transcript's end, and says how
    /// many wait in its modeline.
    pub(super) fn draw_queue(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let text = queue_text(&native.composing.queue);
        let start = native.placed.end();
        let range = start..start + native.composing.queue_len;
        let runs = vec![(text.len(), Some(highlight(Face::Dim)))];
        let buffer = native.buffer;
        self.editor.replace_native_rows(buffer, range, &text, &runs);
        native.composing.queue_len = text.len();
        let count = native.composing.queue.waiting().len();
        let note = match (&native.stream, count) {
            (Some(_), 0) => String::new(),
            (Some(_), count) => format!("{count} queued"),
            (None, 0) => super::STOPPED.to_owned(),
            (None, count) => format!("{} · {count} queued", super::STOPPED),
        };
        self.editor.set_mode_note(buffer, note);
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
        native
            .composing
            .queue
            .waiting()
            .nth(at)
            .map(|queued| queued.id)
    }

    /// `C-c C-q`: point to the queue's first row, in the transcript.
    fn queue_go(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        if native.composing.queue.is_empty() {
            return self.editor.message("Nothing is queued");
        }
        let (transcript, start) = (native.buffer, native.placed.end());
        self.composer_go(key, false);
        if self.editor.current().0.buffer() != transcript {
            return;
        }
        let Some(first) = self.editor.buffer(transcript).map(|buffer| {
            let text = buffer.text();
            text.line_start(text.offset_to_point(start).row + 1)
        }) else {
            return;
        };
        self.editor.current_mut().0.selections.move_primary(first);
    }

    /// `RET` on a queued row: its message goes back into the composer, above what is there.
    fn queued_take_back(&mut self, key: SessionKey, id: u64) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let Some(taken) = native.composing.queue.take_back(id) else {
            return;
        };
        let draft = self.composer_text(key).unwrap_or_default();
        let text = if draft.is_empty() {
            taken
        } else {
            format!("{taken}\n\n{draft}")
        };
        self.set_composer_text(key, &text);
        self.draw_queue(key);
        self.composer_go(key, true);
    }

    /// `e`, `d`, `M-<up>` and `M-<down>` on queued message `id`.
    fn queued_act(&mut self, key: SessionKey, id: u64, action: NativeAction) {
        let empty = self.composer_text(key).is_none_or(|text| text.is_empty());
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let queue = &mut native.composing.queue;
        match action {
            NativeAction::EditQueued if !empty => {
                return self
                    .editor
                    .message("The composer holds a message: send or clear it first");
            }
            NativeAction::EditQueued => {
                let Some(text) = queue.edit(id) else {
                    return;
                };
                self.set_composer_text(key, &text);
                self.draw_queue(key);
                return self.composer_go(key, true);
            }
            NativeAction::DropQueued => {
                queue.drop_one(id);
            }
            NativeAction::MoveQueued(count) => {
                let way = if count < 0 { Move::Up } else { Move::Down };
                for _ in 0..count.unsigned_abs() {
                    if !queue.shift(id, way) {
                        break;
                    }
                }
            }
            _ => return,
        }
        self.draw_queue(key);
    }
}

/// The queue's rows: a heading, then each message's first line, cut, the one edited marked.
fn queue_text(queue: &Queue) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    let count = queue.waiting().len();
    if count == 0 {
        return text;
    }
    let paused = if queue.paused() {
        " · paused: C-c C-c on an empty composer sends the next"
    } else {
        ""
    };
    let _ = writeln!(text, "Queued ({count}){paused}");
    for (at, queued) in queue.waiting().enumerate() {
        let first = queued.text.lines().next().unwrap_or("");
        let mut shown: String = first
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(QUEUED_SHOWN)
            .collect();
        if first.chars().count() > QUEUED_SHOWN || queued.text.lines().nth(1).is_some() {
            shown.push('…');
        }
        let editing = if queue.editing() == Some(queued.id) {
            " (editing)"
        } else {
            ""
        };
        let _ = writeln!(text, "  {}. {shown}{editing}", at + 1);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queues_rows_name_each_message_by_its_first_line() {
        let mut queue = Queue::default();
        assert_eq!(queue_text(&queue), "");
        let a = queue.push("fix the tests\nthen push".into()).unwrap();
        queue.push("x".repeat(100)).unwrap();
        queue.edit(a).unwrap();
        let text = queue_text(&queue);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Queued (2)");
        assert_eq!(lines[1], "  1. fix the tests… (editing)");
        assert_eq!(lines[2].chars().count(), "  2. ".len() + QUEUED_SHOWN + 1);
        queue.give_up_edit();
        queue.pause();
        assert!(queue_text(&queue).starts_with("Queued (2) · paused"));
    }
}
