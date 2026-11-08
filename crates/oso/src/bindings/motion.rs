//! Default key bindings of the motion commands (ADR 0033, ADR 0034).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-f", "forward-char"),
    ("<right>", "forward-char"),
    ("C-b", "backward-char"),
    ("<left>", "backward-char"),
    ("M-f", "forward-word"),
    ("M-b", "backward-word"),
    ("C-n", "next-line"),
    ("<down>", "next-line"),
    ("C-p", "previous-line"),
    ("<up>", "previous-line"),
    ("C-a", "mwim-beginning-of-code-or-line"),
    ("<home>", "move-beginning-of-line"),
    ("C-e", "mwim-end-of-code-or-line"),
    ("<end>", "move-end-of-line"),
    ("M-<", "beginning-of-buffer"),
    ("M->", "end-of-buffer"),
    ("M-g g", "goto-line"),
    ("M-g M-g", "goto-line"),
];
