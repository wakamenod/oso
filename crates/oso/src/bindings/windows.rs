//! Default key bindings of the commands that split, delete, select and resize windows, as
//! Emacs binds them (ADR 0112). `shrink-window`, the splits that select the new window and
//! `windmove-*` have no keys: a configuration binds them (ADR 0108).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-x 2", "split-window-below"),
    ("C-x 3", "split-window-right"),
    ("C-x 0", "delete-window"),
    ("C-x 1", "delete-other-windows"),
    ("C-x o", "other-window"),
    ("C-x +", "balance-windows"),
    ("C-x ^", "enlarge-window"),
    ("C-x }", "enlarge-window-horizontally"),
    ("C-x {", "shrink-window-horizontally"),
];
