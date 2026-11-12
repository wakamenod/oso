//! What the dispatcher does for a jump under way (ADR 0116): a key its keymap does not bind
//! ends it and does not run, and a digit no label of `avy-goto-line` takes reads a line
//! number for `goto-line`, starting with it, as avy's does.
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::command::{Args, Effects, Invocation};
use oso_core::editor::Editor;
use oso_core::jump::{Kind, Stage};
use oso_core::key::KeyPress;

use super::Dispatcher;

/// The command a digit runs from `avy-goto-line`.
const GOTO_LINE: &str = "goto-line";

impl Dispatcher {
    /// A digit typed at `avy-goto-line`'s labels that no label takes: ends the jump and
    /// opens `goto-line`'s prompt with the digit in it. `None` for any other key.
    pub(super) fn jump_digit(&mut self, editor: &mut Editor, key: KeyPress) -> Option<Effects> {
        let digit = key.text().filter(char::is_ascii_digit)?;
        let jump = editor.jump()?;
        let labels = jump.kind() == Kind::LineStart && jump.stage() == Stage::Labels;
        if !labels || jump.takes(digit) {
            return None;
        }
        editor.jump_end();
        let id = self.registry.lookup(GOTO_LINE)?;
        let opened = self
            .run_or_read(editor, id, Args::NONE, Invocation::default())
            .0;
        let mut utf8 = [0; 4];
        let typed = self.insert_text(editor, digit.encode_utf8(&mut utf8));
        Some(opened.union(typed).union(Effects {
            redraw: true,
            ..Effects::default()
        }))
    }

    /// A key the jump's keymap does not bind: ends the jump with nothing moved, and the key
    /// does not run, as a mistyped label must not type. At the labels it says so.
    pub(super) fn jump_unbound(&mut self, editor: &mut Editor) -> Effects {
        editor.jump_refuse();
        self.forget_keys(editor);
        Effects {
            redraw: true,
            ..Effects::default()
        }
    }
}
