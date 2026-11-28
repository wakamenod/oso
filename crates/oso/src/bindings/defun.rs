//! Default key bindings of the defun commands (ADR 0033, ADR 0190).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run, Emacs's.
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-M-a", "beginning-of-defun"),
    ("C-M-<home>", "beginning-of-defun"),
    ("C-M-e", "end-of-defun"),
    ("C-M-<end>", "end-of-defun"),
    ("C-M-h", "mark-defun"),
    ("C-M-l", "reposition-window"),
];
