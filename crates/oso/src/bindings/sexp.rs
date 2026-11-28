//! Default key bindings of the expression commands, Emacs's own (ADR 0190, step 2). `C-M-n`
//! and `C-M-p` are `forward-list` and `backward-list` here too; a language server's
//! diagnostics are on `M-g n` and `M-g p` (`bindings::lsp`).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-M-f", "forward-sexp"),
    ("C-M-<right>", "forward-sexp"),
    ("C-M-b", "backward-sexp"),
    ("C-M-<left>", "backward-sexp"),
    ("C-M-n", "forward-list"),
    ("C-M-p", "backward-list"),
    ("C-M-u", "backward-up-list"),
    ("C-M-<up>", "backward-up-list"),
    ("C-M-d", "down-list"),
    ("C-M-<down>", "down-list"),
    ("C-M-k", "kill-sexp"),
    ("C-M-DEL", "backward-kill-sexp"),
    ("C-M-@", "mark-sexp"),
    ("C-M-SPC", "mark-sexp"),
    ("C-M-t", "transpose-sexps"),
];
