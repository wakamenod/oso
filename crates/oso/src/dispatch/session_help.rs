//! What `session-help` (`C-c ?`) says in a session's terminal (ADR 0132): session mode's keys
//! and what each runs, from the keymap that binds them, so the line says what the keys do.
//! The command only checks where it runs; the dispatcher, which holds the keymaps, answers it
//! after it succeeded.
//!
//! Must not depend on: `app` or winit.

use oso_core::command::{CommandId, Registry};
use oso_core::editor::Editor;
use oso_core::key::{Key, KeyPress, Modifiers};
use oso_core::keymap::{Keymap, display_keys};

use crate::bindings;
use crate::commands::agent_sessions::SESSION_HELP;

/// `session-help`, and the line it says.
pub(super) struct SessionHelp {
    command: Option<CommandId>,
    text: String,
}

impl SessionHelp {
    /// What `session-help` says of `keymap`, a session's terminal's. Build it again with a
    /// keymap that changes.
    pub(super) fn new(keymap: &Keymap, registry: &Registry) -> Self {
        let command = registry.lookup(SESSION_HELP);
        Self {
            command,
            text: text(keymap, registry, command),
        }
    }

    /// Says the keys if `id`, which ran without an error, is `session-help`. Returns whether it
    /// did.
    pub(super) fn answer(&self, id: CommandId, editor: &mut Editor) -> bool {
        if Some(id) != self.command {
            return false;
        }
        editor.message(self.text.clone());
        true
    }
}

/// What `session-help` says: each `C-c` key of session mode's keymap and what it runs, the
/// command's name without `session-`, letters first, then capitals, signs and control keys
/// (ADR 0132). `help` itself is left out.
fn text(keymap: &Keymap, registry: &Registry, help: Option<CommandId>) -> String {
    let mut rows: Vec<(Vec<KeyPress>, String)> = Vec::new();
    let c_c = KeyPress {
        modifiers: Modifiers::CONTROL,
        key: Key::Char('c'),
    };
    keymap.bindings(|keys, id| {
        let Some(command) = registry.get(id).filter(|_| Some(id) != help) else {
            return;
        };
        if keys.first() == Some(&c_c) {
            let name = command.name;
            let label = ["session-", "sessions-", "terminal-"]
                .iter()
                .find_map(|prefix| name.strip_prefix(prefix))
                .unwrap_or(name);
            rows.push((keys[1..].to_vec(), label.to_owned()));
        }
    });
    let menus = bindings::built_in_menus(registry);
    keymap.menu_bindings(|keys, id| {
        if keys.first() == Some(&c_c)
            && let Some(menu) = menus.get(id)
        {
            rows.push((keys[1..].to_vec(), format!("{} menu", menu.name())));
        }
    });
    let order = |keys: &[KeyPress]| match keys.first().map(|key| (key.modifiers, key.key)) {
        Some((Modifiers::NONE, Key::Char(c))) if c.is_ascii_lowercase() => 0,
        Some((Modifiers::NONE, Key::Char(c))) if c.is_ascii_uppercase() => 1,
        Some((Modifiers::NONE, _)) => 2,
        _ => 3,
    };
    let mut rows: Vec<(u8, String, String)> = rows
        .into_iter()
        .map(|(keys, label)| (order(&keys), display_keys(&keys).to_string(), label))
        .collect();
    rows.sort();
    let mut text = String::from("C-c then:");
    for (_, keys, label) in rows {
        text.push_str(&format!("  {keys} {label}"));
    }
    text
}
