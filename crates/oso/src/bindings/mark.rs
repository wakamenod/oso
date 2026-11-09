//! Default key bindings of the mark commands (ADR 0033, ADR 0071).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run. `C-,` is the author's, from their
/// Emacs (ADR 0030).
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-SPC", "set-mark-command"),
    ("C-@", "set-mark-command"),
    ("C-x C-x", "exchange-point-and-mark"),
    ("C-,", "pop-to-mark-command"),
    ("C-.", "consult-mark"),
    ("C-x h", "mark-whole-buffer"),
];
