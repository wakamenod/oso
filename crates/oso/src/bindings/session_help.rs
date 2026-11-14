//! What `session-help` (`C-c ?`) says in a session's terminal (ADR 0132): session mode's keys
//! and what each runs, from the keymaps that bind them, so the line says what the keys do. The
//! default line is built with the dispatcher's keymaps, and one with the user's
//! `[keys.session]` on the configuration thread, as the user's keys are.
//!
//! Must not depend on: `dispatch`, `app` or winit.

use oso_core::command::{CommandId, Registry};
use oso_core::key::{Key, KeyPress, Modifiers};
use oso_core::keymap::{Binding, Keymap, display_keys};
use oso_core::menu::Menus;

use crate::commands::agent_sessions::SESSION_HELP;
use crate::terminal::route::C_C;

/// The longest line, in bytes, before the keys after it are left out. A file's
/// `[keys.session]` may bind thousands of keys; the echo area shows one line.
pub const MAX_HELP: usize = 480;

/// What `session-help` says: each `C-c` key of session mode, the user's keys `user` over the
/// default keys `defaults`, and what it runs, the command's name without `session-`, letters
/// first, then capitals, signs and control keys (ADR 0132). `session-help` itself is left
/// out, and what does not fit in [`MAX_HELP`] bytes ends in `…`.
pub fn session_help(
    defaults: &Keymap,
    user: &Keymap,
    registry: &Registry,
    menus: &Menus,
) -> String {
    let help = registry.lookup(SESSION_HELP);
    let mut rows: Vec<(Vec<KeyPress>, String)> = Vec::new();
    rows_of(defaults, Some(user), registry, menus, help, &mut rows);
    rows_of(user, None, registry, menus, help, &mut rows);
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
        let row = format!("  {keys} {label}");
        if text.len() + row.len() > MAX_HELP {
            text.push_str("  …");
            break;
        }
        text.push_str(&row);
    }
    text
}

/// The `C-c` keys of `keymap` and their labels, after `C-c`, but those the keymap `over` it
/// sets itself, or leaves undefined.
fn rows_of(
    keymap: &Keymap,
    over: Option<&Keymap>,
    registry: &Registry,
    menus: &Menus,
    help: Option<CommandId>,
    rows: &mut Vec<(Vec<KeyPress>, String)>,
) {
    let hidden = |keys: &[KeyPress]| {
        over.is_some_and(|over| {
            (1..=keys.len()).any(|len| match over.lookup(&keys[..len]) {
                None => false,
                Some(Binding::Prefix(_)) => len == keys.len(),
                Some(_) => true,
            })
        })
    };
    let shown = |keys: &[KeyPress]| keys.len() > 1 && keys[0] == C_C && !hidden(keys);
    keymap.bindings(|keys, id| {
        let Some(command) = registry.get(id).filter(|_| Some(id) != help) else {
            return;
        };
        if shown(keys) {
            let name = command.name;
            let label = ["session-", "sessions-", "terminal-"]
                .iter()
                .find_map(|prefix| name.strip_prefix(prefix))
                .unwrap_or(name);
            rows.push((keys[1..].to_vec(), label.to_owned()));
        }
    });
    keymap.menu_bindings(|keys, id| {
        if shown(keys)
            && let Some(menu) = menus.get(id)
        {
            rows.push((keys[1..].to_vec(), format!("{} menu", menu.name())));
        }
    });
}
