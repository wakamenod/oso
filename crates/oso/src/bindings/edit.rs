//! Default key bindings of the editing commands (ADR 0030, ADR 0033, ADR 0034).
//! Keys that type a character are not in the table: they run [`TEXT_COMMAND`]
//! (ADR 0051).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// What a key that types a character runs, in the global keymap.
pub const TEXT_COMMAND: &str = "self-insert-command";

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[
    ("DEL", "delete-backward-char"),
    ("C-d", "delete-char"),
    ("<delete>", "delete-char"),
    // Option-Space: `SPC` stays half-width with a Japanese input source (ADR 0069).
    ("s-SPC", "insert-full-width-space"),
    ("C-z", "undo"),
    ("C-M-z", "undo-redo"),
];
