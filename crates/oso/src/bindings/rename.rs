//! `*lsp-rename*`'s keys (ADR 0138, step 14): a rename's edits shown before they apply, `y` to
//! apply them and `n`, `q` or `C-g` to leave everything as it was.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run, in `*lsp-rename*` only.
pub const PREVIEW: &[(&str, &str)] = &[
    ("y", "lsp-rename-apply"),
    ("n", "lsp-rename-cancel"),
    ("q", "lsp-rename-cancel"),
    ("C-g", "lsp-rename-cancel"),
];
