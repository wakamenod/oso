//! Whether a native session's request waits in front of the person (ADR 0165), or on
//! any card.
//!
//! Must not depend on: key presses, or who asks.
//!
//! - In front: the selected window, or the one beneath the prompts, shows the session's
//!   transcript, its composer or its plan's edit, or is docked under its transcript, as its card
//!   is, while its request waits on a card.
//! - On any card: one a window shows, and one of a session no window shows.

use oso_core::editor::Editor;

use crate::app::App;
use crate::app::sessions::hosting::native::NativeSessions;
use crate::sessions::worker::SessionKey;

/// The session whose request waits in front of the person, if one's does.
fn waiting_in_front(editor: &Editor, natives: &NativeSessions) -> Option<SessionKey> {
    let window = editor.tiled().0;
    let selected = window.buffer();
    let host = window.docked().map_or(selected, |(host, _)| host);
    let its = (natives.map.iter()).find_map(|(key, native)| {
        let here = native.buffer == host
            || native.composing.buffer == Some(selected)
            || native.card == Some(selected);
        here.then_some(*key)
    });
    let key = its.or_else(|| natives.permissions.plans.session(selected))?;
    (natives.permissions.asks.shown(key).is_some()).then_some(key)
}

impl App {
    /// Whether a native session's request waits in front of the person.
    pub(in crate::app) fn request_in_front(&self) -> bool {
        waiting_in_front(&self.editor, &self.hosting.natives).is_some()
    }

    /// Whether a native session's request waits on a card, in front of the person or not.
    pub(in crate::app) fn request_shown(&self) -> bool {
        self.hosting.natives.permissions.asks.any_shown()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Instant;

    use oso_core::buffer::{Buffer, Mode};
    use oso_core::editor::Editor;
    use oso_core::window::DockSlot;

    use super::super::plan::tests::editing;
    use super::{NativeSessions, waiting_in_front};
    use crate::app::sessions::hosting::native::markdown::tests::native;
    use crate::channel::Connection;
    use crate::channel::protocol::permission::Call;
    use crate::sessions::native::asks::Ask;
    use crate::sessions::worker::SessionKey;

    const KEY: SessionKey = 7;

    fn wait(natives: &mut NativeSessions, key: SessionKey, shown: bool) {
        natives.permissions.asks.push(Ask {
            from: Connection {
                run: 1,
                number: key,
            },
            key,
            call: Call {
                tool_name: "Bash".to_owned(),
                tool_use_id: "t1".to_owned(),
                input: None,
            },
            came: Instant::now(),
            question: None,
            shown: shown.then(Instant::now),
            reply: mpsc::channel().0,
        });
    }

    #[test]
    fn a_request_is_in_front_wherever_its_session_is_selected() {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.md", ""));
        let notes = editor.window().buffer();
        let transcript = editor.add_native_session_buffer("*claude: s*");
        let other = editor.add_native_session_buffer("*claude: t*");
        let mut add = |name: &str, mode| editor.add_internal_buffer(name.to_owned(), mode);
        let card = add("*card: s*", Mode::Card);
        let composer = add("*compose: s*", Mode::Composer);
        let strip = add("*todo: s*", Mode::Fundamental);
        let plan = add("*plan: s*", Mode::Plan);
        let mut natives = NativeSessions::default();
        let mut its = native(transcript);
        its.card = Some(card);
        its.composing.buffer = Some(composer);
        natives.map.insert(KEY, its);
        natives.map.insert(KEY + 1, native(other));
        natives.permissions.plans = editing(KEY, plan);
        editor.show(transcript);
        editor.set_dock(transcript, DockSlot::Card, Some(card));
        editor.set_dock(transcript, DockSlot::Strip, Some(strip));
        let docked = [DockSlot::Card, DockSlot::Strip]
            .map(|slot| editor.dock_window(transcript, slot).unwrap());
        let host = editor.window().id();

        let look = |natives: &NativeSessions, editor: &mut Editor| -> Vec<Option<SessionKey>> {
            let mut seen = Vec::new();
            for window in docked {
                editor.select_window(window);
                seen.push(waiting_in_front(editor, natives));
            }
            editor.select_window(host);
            for shown in [transcript, composer, plan] {
                editor.show(shown);
                seen.push(waiting_in_front(editor, natives));
            }
            seen
        };
        assert_eq!(look(&natives, &mut editor), [None; 5], "nothing waits");
        editor.show(transcript);
        wait(&mut natives, KEY, false);
        assert_eq!(look(&natives, &mut editor), [None; 5], "not shown yet");
        assert!(!natives.permissions.asks.any_shown());
        let _ = natives.permissions.asks.remove_where(|_| true);
        wait(&mut natives, KEY, true);
        assert!(natives.permissions.asks.any_shown());
        editor.show(transcript);
        assert_eq!(look(&natives, &mut editor), [Some(KEY); 5]);
        for elsewhere in [notes, other] {
            editor.show(elsewhere);
            assert_eq!(waiting_in_front(&editor, &natives), None);
        }
    }
}
