//! Default key bindings of query-replace (ADR 0033, ADR 0113): the key that starts it, and the
//! keymap of its answers, which takes the place of the others while it is on. A key that keymap
//! does not bind ends the session and then runs, in the dispatcher.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// The global keys that start a query-replace.
pub const BINDINGS: &[(&str, &str)] =
    &[("M-%", "query-replace"), ("C-M-%", "query-replace-regexp")];

/// The answers of a query-replace under way, as Emacs's.
pub const REPLACE: &[(&str, &str)] = &[
    ("y", "replace-this"),
    ("SPC", "replace-this"),
    ("n", "replace-skip"),
    ("DEL", "replace-skip"),
    ("!", "replace-all"),
    (".", "replace-this-and-end"),
    (",", "replace-this-and-stay"),
    ("^", "replace-back"),
    ("u", "replace-undo"),
    ("q", "replace-end"),
    ("RET", "replace-end"),
    ("?", "replace-help"),
    ("C-g", "keyboard-quit"),
];
