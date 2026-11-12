//! Default key bindings of the indentation commands (ADR 0115): `TAB`, `RET` and `C-j`.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[
    ("TAB", "indent-for-tab-command"),
    ("RET", "newline"),
    ("C-j", "electric-newline-and-maybe-indent"),
];
