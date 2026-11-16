//! A terminal's own key bindings (ADR 0120), in a keymap above the global one in a terminal's
//! buffer. These are oso's keys there; `terminal::route` sends every other key to the program
//! before a keymap is looked at.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run, in a terminal's buffer only.
pub const TERMINAL: &[(&str, &str)] = &[
    ("C-c C-c", "terminal-send-interrupt"),
    ("C-c C-k", "terminal-char-mode"),
    ("C-q", "terminal-send-next-key"),
    ("C-y", "terminal-yank"),
];

/// Key sequences and the built-in menus they open, in a terminal's buffer only: the Agents
/// menu, as `C-c a` opens it elsewhere, since the global `C-c` keys do not apply after `C-c` in a
/// terminal (ADR 0143).
pub const TERMINAL_MENUS: &[(&str, &str)] = &[("C-c a", "agents")];
