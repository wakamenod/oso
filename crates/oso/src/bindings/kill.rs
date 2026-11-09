//! Default key bindings of the kill and yank commands (ADR 0030, ADR 0033, ADR 0074).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run. `C-w` is the author's, from their Emacs
/// (ADR 0030).
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-w", "kill-region-or-backward-word"),
    ("M-w", "kill-ring-save"),
    ("C-k", "kill-line"),
    ("M-d", "kill-word"),
    ("M-DEL", "backward-kill-word"),
    ("C-y", "yank"),
    // consult-yank-pop, as the author's Emacs binds it (ADR 0091).
    ("M-y", "consult-yank-pop"),
];
