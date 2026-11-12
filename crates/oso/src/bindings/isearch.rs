//! Default key bindings of isearch (ADR 0033, ADR 0082): the keys that start it, and its own
//! keymap, which takes the place of the others while it is on. A key that keymap does not
//! bind, or a sequence under its `C-c`, ends the search and then runs, in the dispatcher.
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
    // A line break in the search string, as Emacs's `C-j`, not the global key's break.
    ("C-j", "isearch-newline"),
    ("C-w", "isearch-yank-word-or-char"),
    ("C-y", "isearch-yank-kill"),
    ("M-y", "isearch-yank-pop"),
    ("M-p", "isearch-ring-retreat"),
    ("M-n", "isearch-ring-advance"),
    ("M-c", "isearch-toggle-case-fold"),
    // Cursors from the search (ADR 0091); `C-c` is its only prefix.
    ("C->", "isearch-mark-next"),
    ("C-<", "isearch-mark-previous"),
    ("C-c C-<", "isearch-mark-all"),
    // Labels on the matches in view (ADR 0116).
    ("M-j", "avy-isearch"),
    ("C-g", "keyboard-quit"),
];

/// What a key that types a character runs in a search.
pub const TEXT_COMMAND: &str = "isearch-printing-char";
