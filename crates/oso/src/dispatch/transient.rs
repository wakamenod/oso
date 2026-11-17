//! The layers a key opens over the buffer's for a while, closed by any command not their own
//! (ADR 0115, ADR 0138): `C-x TAB`'s, the completion popup's (step 9), which typing into the
//! buffer keeps open and narrows, and a server's description at point (step 10), which then
//! closes for good, the command running as it always does; a key no keymap binds, a menu and
//! the mouse close each.
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
    editor.close_hover();
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
        // `C-g` stops a rename that waits for its server (ADR 0138, step 14, Q7).
        if ran && self.rename_waits && !self.not_input && self.keymaps.keyboard_quit == Some(id) {
            editor.request_server(oso_core::editor::ServerRequest::CancelRename);
        }
    }

    /// Whether a rename waits for its server or its preview, which `C-g` then stops (ADR 0138,
    /// step 14): the host's to say, after each change.
    pub fn set_rename_waits(&mut self, waits: bool) {
        self.rename_waits = waits;
    }

    /// Before command `id` runs: each layer it is not one of the commands of closes, and a
    /// command run apart closes `C-x TAB`'s and the popup.
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
        // A description put away goes too.
        if !self.keymaps.hover_commands.contains(&id) {
            editor.close_hover();
        }
    }
}
