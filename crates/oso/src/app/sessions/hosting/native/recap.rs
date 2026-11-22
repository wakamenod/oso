//! The recap of a native session's turn that ended out of the person's sight, in the app
//! (ADR 0165, part 3f): a turn whose `result` comes while the person is not in front of the
//! session is marked, and once they are, its transcript or its composer, the echo area says
//! the first sentence of its last reply, once. Its text is `sessions::native::recap`'s.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - The recap quotes the transcript, which is withheld from agents, so it is said with
//!   `Editor::echo_withheld_text`: no log line, not in `*Messages*`, and left out of what the
//!   control socket and the agent channel read.
//! - It is looked for after each event, only for the sessions marked, so the person's coming
//!   back however they do, a key, a click or oso's window focused again, says it.

use super::Native;
use crate::app::App;
use crate::sessions::native::recap;
use crate::sessions::native::turns;
use crate::sessions::native::wire::Event;
use crate::sessions::state::Target;
use crate::sessions::worker::SessionKey;

/// Notes whether `event` of a native session ends its turn out of sight, `seen` saying
/// whether the person is in front of it; a turn that starts takes the mark away.
pub(super) fn recap_event(native: &mut Native, event: &Event, seen: bool) {
    match turns::target(event) {
        Some(Target::Idle { .. }) if matches!(event, Event::Result(_)) => native.unseen = !seen,
        Some(Target::Idle { .. }) | None => {}
        Some(_) => native.unseen = false,
    }
}

impl App {
    /// Whether the person is in front of native session `key`: its transcript or its composer
    /// is the buffer in front of them.
    pub(super) fn native_in_front(&self, key: SessionKey) -> bool {
        let Some(shown) = self.buffer_in_front() else {
            return false;
        };
        (self.hosting.natives.map.get(&key))
            .is_some_and(|native| shown == native.buffer || native.composing.buffer == Some(shown))
    }

    /// After each event: each native session whose turn ended out of sight, now in front of
    /// the person, is recapped, once the echo area is empty: what oso said as the person came
    /// back stays until the next key.
    pub(super) fn sync_recaps(&mut self) {
        let marked: Vec<SessionKey> = (self.hosting.natives.map.iter())
            .filter(|(_, native)| native.unseen)
            .map(|(key, _)| *key)
            .collect();
        if marked.is_empty() || !self.echo_empty() {
            return;
        }
        for key in marked {
            if self.native_in_front(key) {
                self.native_recap(key);
            }
        }
    }

    /// Whether the echo area says nothing.
    fn echo_empty(&self) -> bool {
        let mut status = oso_core::editor::Status::default();
        self.editor
            .status_into(self.editor.tiled().0.id(), &mut status);
        status.echo.is_empty()
    }

    /// Says native session `key`'s recap in the echo area, and takes its mark away.
    fn native_recap(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        native.unseen = false;
        let Some(reply) = recap::last_reply(native.rows.rows()) else {
            return;
        };
        let Some(name) = self
            .hosting
            .table
            .get(key)
            .map(|session| session.name.as_str())
        else {
            return;
        };
        let cells = self.editor.frame().echo.columns;
        if let Some(recap) = recap::recap(name, reply, cells) {
            self.editor.echo_withheld_text(recap);
        }
    }
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Buffer;
    use oso_core::editor::Editor;

    use super::*;
    use crate::sessions::native::wire::parse;

    fn native() -> Native {
        let mut editor = Editor::with_buffer(Buffer::from_text("*scratch*", ""));
        super::super::markdown::tests::native(editor.add_native_session_buffer("*claude: a*"))
    }

    fn ended() -> Event {
        parse(br#"{"type":"result","subtype":"success"}"#)
    }

    #[test]
    fn a_turn_ended_out_of_sight_is_marked_until_recapped_or_a_turn_starts() {
        let mut native = native();
        recap_event(&mut native, &ended(), true);
        assert!(!native.unseen, "seen as it ended");
        recap_event(&mut native, &ended(), false);
        assert!(native.unseen);
        recap_event(&mut native, &Event::Other("system/x".into()), true);
        assert!(native.unseen, "nothing else takes it away");
        let replay = parse(br#"{"type":"user","isReplay":true,"message":{"content":"go"}}"#);
        recap_event(&mut native, &replay, false);
        assert!(!native.unseen, "a turn started");
    }
}
