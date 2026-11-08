//! Default key bindings of completion (ADR 0033, ADR 0076).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// `M-x`, in the global keymap.
pub const BINDINGS: &[(&str, &str)] = &[("M-x", "execute-extended-command")];

/// The keys of a prompt that completes, with those of every prompt that reads an answer
/// (`minibuffer::MINIBUFFER`), above the global keymap, as vertico's.
pub const COMPLETION: &[(&str, &str)] = &[
    ("C-n", "next-completion"),
    ("<down>", "next-completion"),
    ("C-p", "previous-completion"),
    ("<up>", "previous-completion"),
    ("M-<", "first-completion"),
    ("M->", "last-completion"),
    ("C-v", "scroll-completions-up"),
    ("M-v", "scroll-completions-down"),
    ("TAB", "insert-completion"),
];
