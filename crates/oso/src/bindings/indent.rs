//! Default key bindings of the indentation commands (ADR 0115): `TAB`, `RET` and `C-j`, the
//! region's `C-M-\` and `C-x TAB` with the layer it opens, and the small commands on `M-m`, `M-i`
//! and `M-^`. Markdown's `M-RET` is in `markdown`.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[
    ("TAB", "indent-for-tab-command"),
    ("RET", "newline"),
    ("C-j", "electric-newline-and-maybe-indent"),
    ("C-M-\\", "indent-region"),
    ("C-x TAB", "indent-rigidly"),
    ("M-m", "back-to-indentation"),
    ("M-i", "tab-to-tab-stop"),
    ("M-^", "delete-indentation"),
];

/// The keys of the layer `C-x TAB` opens, above all others while it is open: any other key
/// closes it and does what it always does.
pub const RIGID: &[(&str, &str)] = &[
    ("<left>", "indent-rigidly-left"),
    ("<right>", "indent-rigidly-right"),
    ("S-<left>", "indent-rigidly-left-to-tab-stop"),
    ("S-<right>", "indent-rigidly-right-to-tab-stop"),
];
