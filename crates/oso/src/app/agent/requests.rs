//! What the agent channel does not do while a native session's request waits for the person
//! (ADR 0182, "Point" 8): with one in front of them, or in sight with the side window selected, its
//! `open` shows no file and none of its commands runs, a look among them; with one in sight, nothing
//! it does changes the window it is in; and with one anywhere, highlighting, wrapping and the session shown
//! stay as they are. The dispatcher refuses its commands in a selected card or plan too.
//!
//! Must not depend on: how a request is shown or answered; the sessions say where one waits.

use crate::app::App;
use crate::app::sessions::RequestWaits;
use crate::commands::{FONT_LOCK, TRUNCATE_LINES, WINDOW_COMMANDS};

/// What an agent is told while a request waits.
const WAITS: &str =
    "a native session's request waits for the person; try again once it is answered";

/// The commands besides [`WINDOW_COMMANDS`] after which a window other than the one the agent's
/// command runs in may be another size, or show or draw something else.
const CHANGES_WINDOWS: [&str; 11] = [
    "global-display-line-numbers-mode",
    "global-hl-line-mode",
    "next-buffer",
    "previous-buffer",
    "review-files",
    "review-quit",
    "review-toggle-panes",
    "session-explain",
    "sessions-narrower",
    "sessions-wider",
    "view-echo-area-messages",
];

/// The commands that show another session, or change how buffers are drawn.
const HELD_ANYWHERE: [&str; 3] = [FONT_LOCK, TRUNCATE_LINES, "session-switch"];

/// Why `open` waits, if a request waits where it would show the file.
fn opens(waits: RequestWaits) -> Option<&'static str> {
    waits.holds_shown().then_some(WAITS)
}

/// Why command `name` waits, if a request waits where it may change what the person sees of it.
fn runs(name: &str, waits: RequestWaits) -> Option<&'static str> {
    let windows = || {
        WINDOW_COMMANDS.iter().any(|command| command.name == name)
            || CHANGES_WINDOWS.contains(&name)
    };
    let held = waits.holds_shown()
        || (waits.in_sight && windows())
        || (waits.any && HELD_ANYWHERE.contains(&name));
    held.then_some(WAITS)
}

impl App {
    /// Why the channel shows no file now, if it may not: the person is in the middle of
    /// something, or a request waits where it would be shown.
    pub(super) fn open_waits(&self) -> Option<&'static str> {
        self.agent_waits().or_else(|| opens(self.request_waits()))
    }

    /// Why the channel does not run command `name` now, if it may not.
    pub(super) fn run_waits(&self, name: &str) -> Option<&'static str> {
        self.agent_waits()
            .or_else(|| runs(name, self.request_waits()))
    }
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::{Buffer, Mode};
    use oso_core::command::{ArgSpec, Reach};
    use oso_core::editor::Editor;
    use oso_core::frame::{CellRect, FrameSize, Way};
    use oso_core::window::{DockSlot, Scroll, WindowId};

    use super::{CHANGES_WINDOWS, RequestWaits, WAITS, opens, runs};
    use crate::commands::{FONT_LOCK, TRUNCATE_LINES, WINDOW_COMMANDS, registry};
    use crate::dispatch::{Dispatcher, Given};

    const NOWHERE: RequestWaits = RequestWaits {
        in_front: false,
        in_sight: false,
        side_selected: false,
        any: false,
    };
    const ANYWHERE: RequestWaits = RequestWaits {
        any: true,
        ..NOWHERE
    };
    const IN_SIGHT: RequestWaits = RequestWaits {
        in_sight: true,
        ..ANYWHERE
    };
    const BESIDE_THE_LIST: RequestWaits = RequestWaits {
        side_selected: true,
        ..IN_SIGHT
    };
    const IN_FRONT: RequestWaits = RequestWaits {
        in_front: true,
        ..IN_SIGHT
    };

    #[test]
    fn a_request_holds_an_agents_open_where_it_would_be_shown() {
        for elsewhere in [NOWHERE, ANYWHERE, IN_SIGHT] {
            assert_eq!(opens(elsewhere), None, "{elsewhere:?}");
        }
        for here in [IN_FRONT, BESIDE_THE_LIST] {
            assert_eq!(opens(here), Some(WAITS), "{here:?}");
        }
    }

    #[test]
    fn no_command_of_an_agents_runs_with_a_request_in_front() {
        for name in ["forward-char", "switch-to-buffer", "insert", FONT_LOCK] {
            assert_eq!(runs(name, NOWHERE), None, "{name}");
            assert_eq!(runs(name, IN_FRONT), Some(WAITS), "{name}");
            assert_eq!(runs(name, BESIDE_THE_LIST), Some(WAITS), "{name}");
        }
        for name in ["forward-char", "switch-to-buffer", "insert"] {
            assert_eq!(runs(name, IN_SIGHT), None, "{name}");
        }
    }

    #[test]
    fn highlighting_wrapping_and_the_session_shown_stay_while_a_request_waits() {
        for name in [FONT_LOCK, TRUNCATE_LINES, "session-switch"] {
            assert!(registry().lookup(name).is_some(), "{name} is a command");
            assert_eq!(runs(name, NOWHERE), None, "{name}");
            assert_eq!(runs(name, ANYWHERE), Some(WAITS), "{name}");
        }
        assert_eq!(runs("other-window", ANYWHERE), None);
        for name in CHANGES_WINDOWS {
            assert!(registry().lookup(name).is_some(), "{name} is a command");
        }
    }

    /// What a window shows, and where.
    type Seen = (WindowId, CellRect, Scroll, Vec<usize>);

    /// What the windows of the transcript and of what is docked under it show.
    fn seen(editor: &Editor, of: &[oso_core::buffer::BufferId]) -> Vec<Seen> {
        let mut seen: Vec<Seen> = (editor.windows())
            .filter(|window| of.contains(&window.buffer()))
            .filter_map(|window| {
                let text = editor.regions(window.id())?.text;
                let cursors = window.selections.iter().map(|s| s.cursor).collect();
                Some((window.id(), text, window.scroll(), cursors))
            })
            .collect();
        seen.sort_by_key(|(id, ..)| *id);
        seen
    }

    #[test]
    fn what_changes_the_windows_is_held_while_a_request_is_in_sight() {
        let windows = WINDOW_COMMANDS.iter().map(|command| command.name);
        for name in windows.chain(CHANGES_WINDOWS) {
            assert_eq!(runs(name, ANYWHERE), None, "{name}");
            assert_eq!(runs(name, IN_SIGHT), Some(WAITS), "{name}");
        }
    }

    /// The commands that ask the host for what changes no other window: an agent's `M-.`, whose
    /// answer opens as its `open` does.
    const HOST_LEAVES_WINDOWS: [&str; 1] = ["lsp-find-definition"];

    /// Whether `editor` asks its host for something after a command.
    fn asks_the_host(editor: &mut Editor) -> bool {
        !editor.take_session_requests().is_empty()
            || !editor.take_review_requests().is_empty()
            || !editor.take_git_requests().is_empty()
            || !editor.take_listing_requests().is_empty()
            || !editor.take_terminal_requests().is_empty()
            || !editor.take_server_requests().is_empty()
            || !editor.take_file_requests().is_empty()
            || !editor.take_asked_visits().is_empty()
            || !editor.take_revert_requests().is_empty()
            || editor.take_search_request().is_some()
            || editor.take_walk_request().is_some()
    }

    /// Every command an agent may run, with each argument it takes, run in an ordinary file, leaves
    /// the window that carries a request as it is, or waits. Walked over the registry, so that a
    /// new command is looked at too; one that asks the host for something, which this does not see,
    /// is named here or held. What needs another kind of buffer, as a review's, fails here.
    #[test]
    fn no_command_of_an_agents_from_elsewhere_changes_the_window_of_a_request() {
        let registry = registry();
        let mut ran = 0;
        for (_, command) in registry.iter().filter(|(_, c)| c.reach != Reach::Person) {
            if runs(command.name, IN_SIGHT).is_some() {
                continue;
            }
            let given: &[Given<'_>] = match command.args {
                ArgSpec::None => &[Given::Nothing],
                ArgSpec::Count => &[Given::Nothing, Given::Count(3), Given::Count(-3)],
                ArgSpec::Buffer { .. } => &[Given::Text("other.md"), Given::Text("*claude: s*")],
                _ => &[Given::Nothing, Given::Count(3), Given::Text("x")],
            };
            for &given in given {
                let (mut editor, its) = beside_a_request();
                let mut dispatcher = Dispatcher::new();
                let before = seen(&editor, &its);
                assert_eq!(before.len(), 3, "the transcript, its card and its composer");
                let outcome =
                    dispatcher.run_for_agent(&mut editor, command.name, given, command.reach);
                assert_eq!(seen(&editor, &its), before, "{} {given:?}", command.name);
                let selected = editor.tiled().0.buffer();
                assert!(!its.contains(&selected), "{} {given:?}", command.name);
                let asks = asks_the_host(&mut editor);
                let named = HOST_LEAVES_WINDOWS.contains(&command.name);
                assert!(!asks || named, "{} asks the host", command.name);
                ran += usize::from(matches!(outcome, Ok((_, Ok(())))));
            }
        }
        assert!(ran > 100, "{ran} ran");
    }

    /// A file in the selected window, and beside it a transcript with a card and a composer.
    fn beside_a_request() -> (Editor, Vec<oso_core::buffer::BufferId>) {
        let lines: String = (0..60).map(|n| format!("line {n}\n")).collect();
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.md", &lines));
        editor.set_frame_size(FrameSize {
            rows: 40,
            columns: 120,
        });
        let _ = editor.add_buffer(Buffer::from_text("other.md", &lines));
        let host = editor.add_native_session_buffer("*claude: s*");
        editor.replace_native_rows(host, 0..0, &lines, &[]);
        let card = editor.add_internal_buffer("*card: s*", Mode::Card);
        editor.set_internal_text(card, "Bash wants to run\n│ $ ls\n[y] allow once");
        let composer = editor.add_internal_buffer("*compose: s*", Mode::Composer);
        let notes = editor.window().buffer();
        let first = editor.window().id();
        editor.split_window(Way::Across, true).unwrap();
        assert_ne!(editor.window().id(), first);
        editor.show(host);
        editor.set_dock(host, DockSlot::Card, Some(card));
        editor.set_dock(host, DockSlot::Composer, Some(composer));
        // Point is above the end.
        let end = editor.buffer(host).unwrap().text().len();
        let reads = editor.buffer(host).unwrap().text().to_string()[..end - 1].rfind('\n');
        editor
            .current_mut()
            .0
            .selections
            .move_primary(reads.unwrap() - 12);
        editor.select_window(first);
        assert_eq!(editor.window().buffer(), notes);
        (editor, vec![host, card, composer])
    }
}
