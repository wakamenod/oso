//! Default key bindings of isearch (ADR 0033, ADR 0082): the keys that start it, and its own
//! keymap, which takes the place of the others while it is on. A key that keymap does not
//! bind ends the search and then runs, in the dispatcher.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// The global keys that start a search.
pub const BINDINGS: &[(&str, &str)] = &[("C-s", "isearch-forward"), ("C-r", "isearch-backward")];

/// The keys of a search under way. A key that types a character runs [`TEXT_COMMAND`].
pub const ISEARCH: &[(&str, &str)] = &[
    ("C-s", "isearch-repeat-forward"),
    ("C-r", "isearch-repeat-backward"),
    ("DEL", "isearch-delete-char"),
    ("RET", "isearch-exit"),
    ("C-w", "isearch-yank-word-or-char"),
    ("C-y", "isearch-yank-kill"),
    ("M-p", "isearch-ring-retreat"),
    ("M-n", "isearch-ring-advance"),
    ("M-c", "isearch-toggle-case-fold"),
    ("C-g", "keyboard-quit"),
];

/// What a key that types a character runs in a search.
pub const TEXT_COMMAND: &str = "isearch-printing-char";
