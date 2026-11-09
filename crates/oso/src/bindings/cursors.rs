//! Default key bindings of the commands that add cursors, as multiple-cursors.el's
//! (ADR 0030, ADR 0091).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[
    ("C->", "mark-next-like-this"),
    ("C-<", "mark-previous-like-this"),
    ("C-M->", "skip-to-next-like-this"),
    ("C-M-<", "skip-to-previous-like-this"),
    ("C-c C-<", "mark-all-like-this"),
    ("C-x SPC", "edit-lines"),
];
