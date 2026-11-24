//! Where a native session's request waits (ADR 0182, "Point" 8): in front of the person, in
//! sight, or anywhere.
//!
//! Must not depend on: key presses, or who asks.
//!
//! - In front: the selected window, or the one beneath the prompts, shows the session's
//!   transcript, its composer or its plan's edit, or is docked under its transcript, as its card
//!   or its line is, or a call's whole input or result, while its request waits.
//! - In sight: a window of the frame shows one of those.
//! - Anywhere: one a window shows, and one of a session no window shows.
//! - With the side window selected, a buffer shown goes to a window of the tree, which may be a
//!   request's in sight.

use oso_core::editor::Editor;
use oso_core::window::Window;

use crate::app::App;
use crate::app::sessions::hosting::native::NativeSessions;
use crate::sessions::worker::SessionKey;

/// Where a request waits, and what is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct RequestWaits {
    /// In front of the person.
    pub(in crate::app) in_front: bool,
    /// In a window of the frame.
    pub(in crate::app) in_sight: bool,
    /// Somewhere.
    pub(in crate::app) any: bool,
    /// The side window is selected: what is shown from it goes to a window of the tree.
    pub(in crate::app) side_selected: bool,
}

impl RequestWaits {
    /// Whether a buffer shown now might come in the window of a request, which holds it back.
    pub(in crate::app) fn holds_shown(self) -> bool {
        self.in_front || (self.in_sight && self.side_selected)
    }
}

/// Where a request of `natives` waits.
fn request_waits(editor: &Editor, natives: &NativeSessions) -> RequestWaits {
    let any = natives.permissions.asks.any_shown();
    let in_front = any && waiting_at(natives, editor.tiled().0).is_some();
    let in_a_window = || {
        (editor.frame().windows().iter())
            .filter(|(_, regions)| regions.text.columns > 0)
            .filter_map(|(id, _)| editor.window_by_id(*id))
            .any(|window| waiting_at(natives, window).is_some())
    };
    RequestWaits {
        in_front,
        in_sight: in_front || (any && in_a_window()),
        any,
        side_selected: editor.side_window() == Some(editor.tiled().0.id()),
    }
}

/// The session whose request waits and that `window` shows, if one's does.
fn waiting_at(natives: &NativeSessions, window: &Window) -> Option<SessionKey> {
    let shown = window.buffer();
    let host = window.docked().map_or(shown, |(host, _)| host);
    let its = (natives.map.iter()).find_map(|(key, native)| {
        let here = native.buffer == host
            || native.composing.buffer == Some(shown)
            || native.card == Some(shown)
            || native.line == Some(shown)
            // A call's whole input, as its line shows it, in the transcript's place.
            || native.result == Some(shown);
        here.then_some(*key)
    });
    let key = its.or_else(|| natives.permissions.plans.session(shown))?;
    (natives.permissions.asks.shown(key).is_some()).then_some(key)
}

impl App {
    /// Where a native session's request waits.
    pub(in crate::app) fn request_waits(&self) -> RequestWaits {
        request_waits(&self.editor, &self.hosting.natives)
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
    use super::{NativeSessions, request_waits, waiting_at};
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
            made: None,
            asked: None,
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
        let line = add("*request: s*", Mode::Request);
        let result = add("*result: s*", Mode::Fundamental);
        let mut natives = NativeSessions::default();
        let mut its = native(transcript);
        its.card = Some(card);
        its.line = Some(line);
        its.result = Some(result);
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
                seen.push(waiting_at(natives, editor.tiled().0));
            }
            editor.select_window(host);
            for shown in [transcript, composer, plan, line, result] {
                editor.show(shown);
                seen.push(waiting_at(natives, editor.tiled().0));
            }
            seen
        };
        assert_eq!(look(&natives, &mut editor), [None; 7], "nothing waits");
        editor.show(transcript);
        wait(&mut natives, KEY, false);
        assert_eq!(look(&natives, &mut editor), [None; 7], "not shown yet");
        assert!(!natives.permissions.asks.any_shown());
        let _ = natives.permissions.asks.remove_where(|_| true);
        wait(&mut natives, KEY, true);
        assert!(natives.permissions.asks.any_shown());
        editor.show(transcript);
        assert_eq!(look(&natives, &mut editor), [Some(KEY); 7]);
        // Its line, docked in the card's place.
        editor.show(transcript);
        editor.set_dock(transcript, DockSlot::Card, Some(line));
        assert_eq!(look(&natives, &mut editor)[0], Some(KEY));
        for elsewhere in [notes, other] {
            editor.show(elsewhere);
            assert_eq!(waiting_at(&natives, editor.tiled().0), None);
        }
    }

    #[test]
    fn a_request_is_in_sight_in_any_window_of_the_frame() {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.md", ""));
        editor.set_frame_size(oso_core::frame::FrameSize {
            rows: 40,
            columns: 120,
        });
        let transcript = editor.add_native_session_buffer("*claude: s*");
        let plan = editor.add_internal_buffer("*plan: s*".to_owned(), Mode::Plan);
        let mut natives = NativeSessions::default();
        natives.map.insert(KEY, native(transcript));
        natives.permissions.plans = editing(KEY, plan);
        let at = |natives: &NativeSessions, editor: &Editor| {
            let waits = request_waits(editor, natives);
            (
                waits.in_front,
                waits.in_sight,
                waits.any,
                waits.side_selected,
            )
        };
        assert_eq!(at(&natives, &editor), (false, false, false, false));
        wait(&mut natives, KEY, true);
        assert_eq!(at(&natives, &editor), (false, false, true, false));
        let notes = editor.window().id();
        let beside = editor
            .split_window(oso_core::frame::Way::Across, true)
            .unwrap();
        for shown in [transcript, plan] {
            editor.select_window(beside);
            editor.show(shown);
            assert_eq!(at(&natives, &editor), (true, true, true, false));
            editor.select_window(notes);
            assert_eq!(at(&natives, &editor), (false, true, true, false));
        }
        let _ = natives.permissions.asks.remove_where(|_| true);
        assert_eq!(at(&natives, &editor), (false, false, false, false));
    }
}
