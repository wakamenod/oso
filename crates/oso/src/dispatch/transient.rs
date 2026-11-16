//! The layers a key opens over the buffer's for a while, closed by any command not their own
//! (ADR 0115, ADR 0138): `C-x TAB`'s, and the completion popup's (step 9), which typing into
//! the buffer keeps open and narrows; a key no keymap binds, a menu and the mouse close both.
//! After a command, the popup follows its word, and the person's typing is noted for
//! completing by itself (step 9b).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::command::CommandId;
use oso_core::editor::Editor;

use super::{Dispatcher, Step};

/// Closes every layer a key opened for a while: for a key no keymap binds, a menu that opens,
/// or a press or a wheel turn of the mouse.
pub(super) fn close_all(editor: &mut Editor) {
    editor.set_indent_rigidly_layer(false);
    editor.close_completion_popup();
}

impl Dispatcher {
    /// After command `id` ran, `ran` well: the completion popup follows the word, and the
    /// person's own typing is noted, which may ask for completions (ADR 0138, step 9b): never
    /// the agent's, a replayed macro's or a host's.
    pub(super) fn after_command(&self, editor: &mut Editor, id: CommandId, ran: bool) {
        editor.follow_completion_popup();
        let typed = ran && !self.not_input && !editor.macro_replaying() && !editor.runs_for_agent();
        if typed && self.keymaps.self_insert == Some(id) {
            editor.note_completion_typed();
        }
    }

    /// Before command `id` runs: each layer it is not one of the commands of closes, and a
    /// command run apart closes both.
    pub(super) fn close_transient_layers(&self, editor: &mut Editor, id: CommandId, step: Step) {
        // `C-x TAB`'s keys' edits join, so its commands keep it open.
        if step == Step::Apart || !self.keymaps.rigid_commands.contains(&id) {
            editor.set_indent_rigidly_layer(false);
        }
        if editor.completion_popup().is_some()
            && (step == Step::Apart || !self.keymaps.popup_commands.contains(&id))
        {
            editor.close_completion_popup();
        }
    }
}
