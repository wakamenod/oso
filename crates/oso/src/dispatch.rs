//! From key presses to commands: the keymaps, the key resolver and the
//! command registry, wired together without a window so that key sequences
//! can be tested as plain functions of the editor state.

use oso_core::command::{Args, Ctx, Effects, ExecuteError, Registry};
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

    /// Handles one key press: waits on a prefix, or runs the command the
    /// keys resolve to. Returns what the host should do: redraw only when a
    /// command changed something visible, and exit when one asked to.
    pub fn handle_key(&mut self, editor: &mut Editor, key: KeyPress) -> Effects {
        let layers = Layers::global(&self.global);
        match self.resolver.feed(&layers, key) {
            Resolution::Command(id) => {
                let mut ctx = Ctx::new(editor);
                // Prefix arguments (`C-u`) are not read yet.
                match self.registry.execute(id, &mut ctx, Args::NONE) {
                    Ok(()) => ctx.effects(),
                    Err(e @ ExecuteError::Panicked { .. }) => {
                        // Not fatal (ADR 0014). The echo area shows it from
                        // M2. What the command changed before panicking may
                        // be visible, so redraw.
                        log::error!("{e}");
                        Effects {
                            redraw: true,
                            ..ctx.effects()
                        }
                    }
                    Err(e) => {
                        log::warn!("{e}");
                        ctx.effects()
                    }
                }
            }
            Resolution::Pending => {
                log::debug!("{}-", display_keys(self.resolver.pending()));
                Effects::default()
            }
            Resolution::Unbound(keys) => {
                log::info!("{} is undefined", display_keys(keys));
                Effects::default()
            }
            Resolution::Cancelled => {
                log::info!("Quit");
                Effects::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Dispatcher;
    use crate::commands::COMMANDS;
    use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Effects, Registry};
    use oso_core::editor::Editor;
    use oso_core::key::KeySequence;
    use oso_core::keymap::Keymap;

    /// Feeds the keys of `s` and returns the combined effects.
    fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, s: &str) -> Effects {
        let keys: KeySequence = s.parse().unwrap();
        keys.0.into_iter().fold(Effects::default(), |effects, key| {
            effects.union(dispatcher.handle_key(editor, key))
        })
    }

    const REDRAW: Effects = Effects {
        redraw: true,
        exit: false,
    };

    #[test]
    fn char_motion_through_the_keymap() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = Editor::default();
        assert_eq!(send(&mut dispatcher, &mut editor, "C-f C-f C-b"), REDRAW);
        assert_eq!(editor.cursor.position, 1);
        assert_eq!(
            send(&mut dispatcher, &mut editor, "<right> <right> <left>"),
            REDRAW
        );
        assert_eq!(editor.cursor.position, 2);
    }

    #[test]
    fn a_prefix_then_an_unbound_key_changes_nothing() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = Editor {
            cursor: oso_core::cursor::Cursor { position: 3 },
        };
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
        assert_eq!(editor.cursor.position, 3);
        assert!(dispatcher.pending().is_empty());
        // The next key starts afresh.
        assert_eq!(send(&mut dispatcher, &mut editor, "C-f"), REDRAW);
        assert_eq!(editor.cursor.position, 4);
    }

    #[test]
    fn c_g_cancels_a_prefix_and_quits_without_one() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = Editor::default();
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
        assert_eq!(editor.cursor.position, 0);
    }

    #[test]
    fn c_x_c_c_exits() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = Editor::default();
        assert_eq!(
            send(&mut dispatcher, &mut editor, "C-x C-c"),
            Effects {
                redraw: false,
                exit: true
            }
        );
    }

    fn explode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
        ctx.editor.cursor.position = 10;
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
        let mut editor = Editor::default();
        assert_eq!(send(&mut dispatcher, &mut editor, "C-c p"), REDRAW);
        assert_eq!(editor.cursor.position, 10);
        assert!(dispatcher.pending().is_empty());
        assert_eq!(send(&mut dispatcher, &mut editor, "C-f"), REDRAW);
        assert_eq!(editor.cursor.position, 11);
    }
}
