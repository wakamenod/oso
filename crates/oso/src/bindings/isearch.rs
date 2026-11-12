//! Default key bindings of isearch (ADR 0033, ADR 0082, ADR 0116): the keys that start it,
//! and its own keymap, which takes the place of the others while it is on. A key that keymap
//! does not bind, or a sequence under its `C-c`, ends the search and then runs, in the
//! dispatcher.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// The global keys that start a search.
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-s", "isearch-forward"),
    ("C-r", "isearch-backward"),
    // A regexp's search (ADR 0116).
    ("C-M-s", "isearch-forward-regexp"),
    ("C-M-r", "isearch-backward-regexp"),
];

/// The keys of a search under way. A key that types a character runs [`TEXT_COMMAND`].
pub const ISEARCH: &[(&str, &str)] = &[
    ("C-s", "isearch-repeat-forward"),
    ("C-r", "isearch-repeat-backward"),
    // As in Emacs, a regexp's keys repeat any search.
    ("C-M-s", "isearch-repeat-forward"),
    ("C-M-r", "isearch-repeat-backward"),
    ("DEL", "isearch-delete-char"),
    ("RET", "isearch-exit"),
    ("C-w", "isearch-yank-word-or-char"),
    ("C-y", "isearch-yank-kill"),
    ("M-y", "isearch-yank-pop"),
    ("M-p", "isearch-ring-retreat"),
    ("M-n", "isearch-ring-advance"),
    ("M-c", "isearch-toggle-case-fold"),
    ("M-r", "isearch-toggle-regexp"),
    // Cursors from the search (ADR 0091); `C-c` is its only prefix.
    ("C->", "isearch-mark-next"),
    ("C-<", "isearch-mark-previous"),
    ("C-c C-<", "isearch-mark-all"),
    ("C-g", "keyboard-quit"),
];

/// What a key that types a character runs in a search.
pub const TEXT_COMMAND: &str = "isearch-printing-char";
