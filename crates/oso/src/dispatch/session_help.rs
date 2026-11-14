//! What `session-help` (`C-c ?`) says in a session's terminal (ADR 0132). The command only
//! checks where it runs; the dispatcher, which holds the keymaps, answers it after it
//! succeeded. The line is built by [`bindings::session_help`]: here for the default keys, and
//! on the configuration thread for the user's `[keys.session]`.
//!
//! Must not depend on: `app` or winit.

use oso_core::command::{CommandId, Registry};
use oso_core::editor::Editor;
use oso_core::keymap::Keymap;

use crate::bindings;
use crate::commands::agent_sessions::SESSION_HELP;

/// `session-help`, and the line it says.
pub(super) struct SessionHelp {
    command: Option<CommandId>,
    /// The line of the default keys.
    default: String,
    /// The line of the user's keys over them, when the configuration has given one.
    user: Option<String>,
}

impl SessionHelp {
    /// What `session-help` says of `keymap`, a session's terminal's default keymap.
    pub(super) fn new(keymap: &Keymap, registry: &Registry) -> Self {
        let menus = bindings::built_in_menus(registry);
        Self {
            command: registry.lookup(SESSION_HELP),
            default: bindings::session_help(keymap, &Keymap::new(), registry, &menus),
            user: None,
        }
    }

    /// Says `line` from now on, the user's keys' line; `None` goes back to the default one.
    pub(super) fn set(&mut self, line: Option<String>) {
        self.user = line;
    }

    /// Says the keys if `id`, which ran without an error, is `session-help`. Returns whether it
    /// did.
    pub(super) fn answer(&self, id: CommandId, editor: &mut Editor) -> bool {
        if Some(id) != self.command {
            return false;
        }
        editor.message(self.user.as_ref().unwrap_or(&self.default).clone());
        true
    }
}
