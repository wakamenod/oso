//! Default key bindings of the commands of paragraphs, sentences and pages (ADR 0033,
//! ADR 0190).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run, Emacs's own.
pub const BINDINGS: &[(&str, &str)] = &[
    ("M-}", "forward-paragraph"),
    ("C-<down>", "forward-paragraph"),
    ("M-{", "backward-paragraph"),
    ("C-<up>", "backward-paragraph"),
    ("M-h", "mark-paragraph"),
    ("M-e", "forward-sentence"),
    ("M-a", "backward-sentence"),
    ("M-k", "kill-sentence"),
    ("C-x DEL", "backward-kill-sentence"),
    ("C-x ]", "forward-page"),
    ("C-x [", "backward-page"),
    ("C-x C-p", "mark-page"),
];
