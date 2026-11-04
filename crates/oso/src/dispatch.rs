//! From key presses to commands: the keymaps, the key resolver and the
//! command registry, wired together without a window so that key sequences
//! can be tested as plain functions of the editor state.
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a
//! window.

#[cfg(any(test, feature = "dev"))]
use oso_core::command::ArgSpec;
use oso_core::command::{Args, CommandId, Ctx, Effects, ExecuteError, Registry};
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::keymap::{KeyResolver, Keymap, Layers, Resolution, display_keys};

use crate::{bindings, commands};

/// Runs commands for key presses. Keys from the window and from the dev
/// control socket both come through here.
pub struct Dispatcher {
    registry: Registry,
    global: Keymap,
    resolver: KeyResolver,
}

impl Dispatcher {
    /// A dispatcher with the default commands and global keymap.
    pub fn new() -> Self {
        let registry = commands::registry();
        let global = bindings::global_keymap(&registry);
        Self::with(registry, global)
    }

    /// A dispatcher over `registry`, with `global` as the global keymap.
    pub fn with(registry: Registry, global: Keymap) -> Self {
        Self {
            registry,
            global,
            resolver: KeyResolver::new(),
        }
    }

    /// The keys of the pending prefix, empty if none is pending. Only
    /// `get-state` shows them until the echo area (M2) does.
    #[cfg(any(test, feature = "dev"))]
    pub fn pending(&self) -> &[KeyPress] {
        self.resolver.pending()
    }

    /// Picks up a prefix in progress, for a state handoff. Commands never run
    /// here: keys that are no longer a prefix in this build are dropped.
    #[cfg(any(test, feature = "dev"))]
    pub fn restore_pending(&mut self, keys: &[KeyPress]) {
        let layers = Layers::global(&self.global);
        for key in keys {
            if !matches!(self.resolver.feed(&layers, *key), Resolution::Pending) {
                self.resolver.reset();
                return;
            }
        }
    }

    /// Handles key presses in order, as from the control socket's
    /// `send-keys`. Keys after one that exits are dropped.
    #[cfg(any(test, feature = "dev"))]
    pub fn handle_keys(
        &mut self,
        editor: &mut Editor,
        keys: impl IntoIterator<Item = KeyPress>,
    ) -> Effects {
        let mut effects = Effects::default();
        for key in keys {
            effects = effects.union(self.handle_key(editor, key));
            if effects.exit {
                break;
            }
        }
        effects
    }

    /// Runs the command named `name`, as the control socket's `run-command`
    /// does, with `arg` as its numeric argument. Returns what the host
    /// should do, which matters even if the command failed: a motion that
    /// hits the buffer end still moves as far as it can.
    #[cfg(any(test, feature = "dev"))]
    pub fn run_command(
        &self,
        editor: &mut Editor,
        name: &str,
        arg: Option<isize>,
    ) -> (Effects, Result<(), String>) {
        let found = self
            .registry
            .lookup(name)
            .and_then(|id| Some((id, self.registry.get(id)?)));
        let Some((id, command)) = found else {
            return (
                Effects::default(),
                Err(format!("no command named {name:?}")),
            );
        };
        let args = match (command.args, arg) {
            (_, None) => Args::NONE,
            (ArgSpec::Count, Some(n)) => Args::numeric(n),
            (ArgSpec::None, Some(_)) => {
                return (Effects::default(), Err(format!("{name} takes no argument")));
            }
        };
        let (effects, result) = self.execute(editor, id, args);
        (effects, result.map_err(|e| e.to_string()))
    }

    /// Runs one command, then scrolls the selected window to keep its cursor
    /// in view, as Emacs's redisplay does.
    fn execute(
        &self,
        editor: &mut Editor,
        id: CommandId,
        args: Args,
    ) -> (Effects, Result<(), ExecuteError>) {
        let mut ctx = Ctx::new(editor);
        let result = self.registry.execute(id, &mut ctx, args);
        let mut effects = ctx.effects();
        // What a panicking command changed may be visible.
        effects.redraw |= matches!(result, Err(ExecuteError::Panicked { .. }));
        effects.redraw |= editor.follow_cursor();
        (effects, result)
    }

    /// Handles one key press: waits on a prefix, or runs the command the
    /// keys resolve to. Returns what the host should do: redraw only when a
    /// command changed something visible, and exit when one asked to.
    pub fn handle_key(&mut self, editor: &mut Editor, key: KeyPress) -> Effects {
        let layers = Layers::global(&self.global);
        match self.resolver.feed(&layers, key) {
            Resolution::Command(id) => {
                // Prefix arguments (`C-u`) are not read yet.
                let (effects, result) = self.execute(editor, id, Args::NONE);
                // Not fatal (ADR 0014). The echo area shows these from M2.
                match result {
                    Ok(()) => {}
                    Err(e @ ExecuteError::Panicked { .. }) => log::error!("{e}"),
                    Err(e) => log::warn!("{e}"),
                }
                effects
            }
            Resolution::Pending => {
                log::debug!("{}-", display_keys(self.resolver.pending()));
                Effects::default()
            }
            Resolution::Unbound(keys) => {
                log::debug!("{} is undefined", display_keys(keys));
                Effects::default()
            }
            Resolution::Cancelled => {
                log::debug!("Quit");
                Effects::default()
            }
        }
    }
}

#[cfg(test)]
mod grid_tests;

#[cfg(test)]
mod tests {
    use super::Dispatcher;
    use crate::commands::COMMANDS;
    use oso_core::buffer::Buffer;
    use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Effects, Registry};
    use oso_core::display_map::DisplayPoint;
    use oso_core::editor::Editor;
    use oso_core::key::KeySequence;
    use oso_core::keymap::Keymap;
    use oso_core::window::{Scroll, ViewSize};

    /// Feeds the keys of `s` and returns the combined effects.
    fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, s: &str) -> Effects {
        let keys: KeySequence = s.parse().unwrap();
        dispatcher.handle_keys(editor, keys.0)
    }

    const REDRAW: Effects = Effects {
        redraw: true,
        exit: false,
    };

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("test", text));
        editor.set_view_size(ViewSize::new(10, 20));
        editor
    }

    fn cursor(editor: &Editor) -> usize {
        editor.window().selections.primary().cursor
    }

    /// The cursor as a display point.
    fn point(editor: &Editor) -> DisplayPoint {
        let (window, buffer) = editor.current();
        buffer
            .display()
            .offset_to_display_point(window.selections.primary().cursor)
    }

    #[test]
    fn char_motion_through_the_keymap() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abcdef");
        assert_eq!(send(&mut dispatcher, &mut editor, "C-f C-f C-b"), REDRAW);
        assert_eq!(cursor(&editor), 1);
        assert_eq!(
            send(&mut dispatcher, &mut editor, "<right> <right> <left>"),
            REDRAW
        );
        assert_eq!(cursor(&editor), 2);
    }

    #[test]
    fn one_key_passes_one_character_on_screen() {
        let mut dispatcher = Dispatcher::new();
        // Full width, a combining mark, a ZWJ family, a flag, a tab, CRLF.
        let text = "日か\u{3099}👨‍👩‍👧🇯🇵\tx\r\n";
        let mut editor = editor(text);
        let mut stops = vec![0];
        for _ in 0..7 {
            send(&mut dispatcher, &mut editor, "C-f");
            stops.push(cursor(&editor));
        }
        let pieces: Vec<&str> = stops.windows(2).map(|w| &text[w[0]..w[1]]).collect();
        assert_eq!(pieces, ["日", "か\u{3099}", "👨‍👩‍👧", "🇯🇵", "\t", "x", "\r\n"]);
        assert_eq!(point(&editor), DisplayPoint::new(1, 0));
        send(&mut dispatcher, &mut editor, "C-b C-b");
        assert_eq!(point(&editor), DisplayPoint::new(0, 16), "x after the tab");
    }

    #[test]
    fn line_motion_keeps_the_goal_column_across_widths() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abcdefgh\n日本語\n\nab\tc\r\nabcdefgh");
        send(&mut dispatcher, &mut editor, "C-f C-f C-f C-f C-f");
        assert_eq!(point(&editor), DisplayPoint::new(0, 5));
        send(&mut dispatcher, &mut editor, "C-n");
        assert_eq!(
            point(&editor),
            DisplayPoint::new(1, 4),
            "inside 語: its start"
        );
        send(&mut dispatcher, &mut editor, "C-n");
        assert_eq!(point(&editor), DisplayPoint::new(2, 0), "an empty line");
        send(&mut dispatcher, &mut editor, "C-n");
        assert_eq!(
            point(&editor),
            DisplayPoint::new(3, 2),
            "inside the tab: its start"
        );
        send(&mut dispatcher, &mut editor, "C-n");
        assert_eq!(point(&editor), DisplayPoint::new(4, 5), "back at the goal");
        send(&mut dispatcher, &mut editor, "C-p C-p C-p C-p");
        assert_eq!(point(&editor), DisplayPoint::new(0, 5));
        // Another motion resets the goal.
        send(&mut dispatcher, &mut editor, "C-b C-n");
        assert_eq!(point(&editor), DisplayPoint::new(1, 4));
    }

    #[test]
    fn line_motion_from_a_line_end_stays_at_line_ends() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abc\r\n日本語です\nx\nabcdefgh");
        send(&mut dispatcher, &mut editor, "C-e C-n");
        assert_eq!(point(&editor), DisplayPoint::new(1, 10));
        send(&mut dispatcher, &mut editor, "C-n C-n");
        assert_eq!(point(&editor), DisplayPoint::new(3, 8));
        send(&mut dispatcher, &mut editor, "C-a C-p");
        assert_eq!(point(&editor), DisplayPoint::new(2, 0));
        send(&mut dispatcher, &mut editor, "M-> M-<");
        assert_eq!(cursor(&editor), 0);
        send(&mut dispatcher, &mut editor, "M->");
        assert_eq!(point(&editor), DisplayPoint::new(3, 8));
    }

    #[test]
    fn the_window_follows_the_cursor_by_the_fewest_rows_and_columns() {
        let mut dispatcher = Dispatcher::new();
        let long = "x".repeat(30);
        let text: String = (0..30).map(|_| format!("{long}\n")).collect();
        let mut editor = editor(&text);
        for _ in 0..10 {
            send(&mut dispatcher, &mut editor, "C-n");
        }
        assert_eq!(editor.window().scroll(), Scroll { top: 1, left: 0 });
        assert_eq!(send(&mut dispatcher, &mut editor, "C-e"), REDRAW);
        assert_eq!(editor.window().scroll(), Scroll { top: 1, left: 11 });
        send(&mut dispatcher, &mut editor, "C-b");
        assert_eq!(editor.window().scroll().left, 11, "still in view");
        send(&mut dispatcher, &mut editor, "C-a M->");
        assert_eq!(editor.window().scroll(), Scroll { top: 21, left: 0 });
        send(&mut dispatcher, &mut editor, "M-<");
        assert_eq!(editor.window().scroll(), Scroll::default());
    }

    #[test]
    fn page_keys_scroll_by_a_screen() {
        let mut dispatcher = Dispatcher::new();
        let text: String = (0..30).map(|i| format!("{i:02}\n")).collect();
        let mut editor = editor(&text);
        send(&mut dispatcher, &mut editor, "C-v <next>");
        assert_eq!(editor.window().scroll().top, 16);
        assert_eq!(point(&editor), DisplayPoint::new(16, 0));
        send(&mut dispatcher, &mut editor, "M-v <prior>");
        assert_eq!(editor.window().scroll().top, 0);
        assert_eq!(point(&editor), DisplayPoint::new(9, 0));
        // At the start M-v signals and changes nothing.
        assert_eq!(
            send(&mut dispatcher, &mut editor, "M-v"),
            Effects::default()
        );
    }

    #[test]
    fn a_prefix_then_an_unbound_key_changes_nothing() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abcdef");
        send(&mut dispatcher, &mut editor, "C-f C-f C-f");
        assert_eq!(
            send(&mut dispatcher, &mut editor, "C-x"),
            Effects::default()
        );
        assert_eq!(
            dispatcher.pending(),
            &"C-x".parse::<KeySequence>().unwrap().0[..]
        );
        assert_eq!(
            send(&mut dispatcher, &mut editor, "C-f"),
            Effects::default()
        );
        assert_eq!(cursor(&editor), 3);
        assert!(dispatcher.pending().is_empty());
        // The next key starts afresh.
        assert_eq!(send(&mut dispatcher, &mut editor, "C-f"), REDRAW);
        assert_eq!(cursor(&editor), 4);
    }

    #[test]
    fn c_g_cancels_a_prefix_and_quits_without_one() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abc");
        assert_eq!(
            send(&mut dispatcher, &mut editor, "C-x C-g"),
            Effects::default()
        );
        assert!(dispatcher.pending().is_empty());
        assert_eq!(
            send(&mut dispatcher, &mut editor, "C-g"),
            Effects::default()
        );
        assert_eq!(
            send(&mut dispatcher, &mut editor, "C-c"),
            Effects::default()
        );
        assert_eq!(cursor(&editor), 0);
    }

    #[test]
    fn c_x_c_c_exits() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("");
        assert_eq!(
            send(&mut dispatcher, &mut editor, "C-x C-c"),
            Effects {
                redraw: false,
                exit: true
            }
        );
    }

    #[test]
    fn keys_after_an_exit_are_dropped() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abc");
        let effects = send(&mut dispatcher, &mut editor, "C-f C-x C-c <right> C-x");
        assert!(effects.exit);
        assert_eq!(cursor(&editor), 1);
        assert!(dispatcher.pending().is_empty());
    }

    fn explode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
        ctx.editor.current_mut().0.selections = oso_core::selection::SelectionSet::new(2);
        panic!("deliberate panic in a test command");
    }

    #[test]
    fn a_panicking_command_does_not_stop_the_editor() {
        let mut registry = Registry::new();
        registry.register_all(COMMANDS).unwrap();
        registry
            .register(Command {
                name: "explode",
                doc: "Panic, to test the command boundary.",
                args: ArgSpec::None,
                run: explode,
            })
            .unwrap();
        let global =
            Keymap::from_table(&registry, &[("C-c p", "explode"), ("C-f", "forward-char")])
                .unwrap();
        let mut dispatcher = Dispatcher::with(registry, global);
        let mut editor = editor("abcdef");
        assert_eq!(send(&mut dispatcher, &mut editor, "C-c p"), REDRAW);
        assert_eq!(cursor(&editor), 2);
        assert!(dispatcher.pending().is_empty());
        assert_eq!(send(&mut dispatcher, &mut editor, "C-f"), REDRAW);
        assert_eq!(cursor(&editor), 3);
    }

    #[test]
    fn run_command_runs_by_name_with_its_argument() {
        let dispatcher = Dispatcher::new();
        let mut editor = editor("abcdef");
        let (effects, result) = dispatcher.run_command(&mut editor, "forward-char", None);
        assert_eq!((effects, result), (REDRAW, Ok(())));
        assert_eq!(cursor(&editor), 1);
        let (_, result) = dispatcher.run_command(&mut editor, "forward-char", Some(3));
        assert_eq!(result, Ok(()));
        assert_eq!(cursor(&editor), 4);
        let (_, result) = dispatcher.run_command(&mut editor, "backward-char", Some(-1));
        assert_eq!(result, Ok(()));
        assert_eq!(cursor(&editor), 5);
    }

    #[test]
    fn run_command_scrolls_to_the_cursor_and_asks_to_exit() {
        let dispatcher = Dispatcher::new();
        let text: String = (0..100).map(|i| format!("{i}\n")).collect();
        let mut editor = editor(&text);
        let (_, result) = dispatcher.run_command(&mut editor, "next-line", Some(50));
        assert_eq!(result, Ok(()));
        assert_eq!(editor.window().scroll().top, 41);
        let (effects, result) =
            dispatcher.run_command(&mut editor, "save-buffers-kill-terminal", None);
        assert!(effects.exit);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn run_command_reports_errors_and_keeps_what_they_changed() {
        let dispatcher = Dispatcher::new();
        let mut editor = editor("abc");
        let (effects, result) = dispatcher.run_command(&mut editor, "no-such-command", None);
        assert_eq!(effects, Effects::default());
        assert_eq!(result, Err("no command named \"no-such-command\"".into()));
        let (effects, result) = dispatcher.run_command(&mut editor, "forward-char", Some(10));
        assert_eq!(result, Err("forward-char: End of buffer".into()));
        assert_eq!(effects, REDRAW, "it moved as far as it could");
        assert_eq!(cursor(&editor), 3);
    }

    #[test]
    fn run_command_reports_a_panic_and_an_unwanted_argument() {
        let mut registry = oso_core::command::Registry::new();
        registry
            .register(Command {
                name: "explode",
                doc: "Panics.",
                args: ArgSpec::None,
                run: explode,
            })
            .unwrap();
        let dispatcher = Dispatcher::with(registry, Keymap::new());
        let mut editor = editor("abc");
        let (effects, result) = dispatcher.run_command(&mut editor, "explode", None);
        assert_eq!(result, Err("explode panicked".into()));
        assert!(effects.redraw, "what it changed may be visible");
        let (effects, result) = dispatcher.run_command(&mut editor, "explode", Some(1));
        assert_eq!(result, Err("explode takes no argument".into()));
        assert_eq!(effects, Effects::default(), "it did not run");
    }

    #[test]
    fn a_restored_prefix_waits_for_its_next_key_and_runs_nothing() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abc");
        let keys = |s: &str| s.parse::<KeySequence>().unwrap().0;
        dispatcher.restore_pending(&keys("C-x"));
        assert_eq!(dispatcher.pending(), keys("C-x"));
        assert!(send(&mut dispatcher, &mut editor, "C-c").exit);

        // Not a prefix any more: a command, or nothing at all.
        dispatcher.restore_pending(&keys("C-f"));
        assert!(dispatcher.pending().is_empty());
        assert_eq!(cursor(&editor), 0, "the command did not run");
        dispatcher.restore_pending(&keys("C-x C-z C-z"));
        assert!(dispatcher.pending().is_empty());
    }
}
