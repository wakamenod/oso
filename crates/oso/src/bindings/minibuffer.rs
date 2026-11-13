//! Default key bindings of prompts (ADR 0033, ADR 0072).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// The keys of a prompt that reads an answer, above the global keymap, so editing keys
/// work in it unchanged.
pub const MINIBUFFER: &[(&str, &str)] = &[
    ("RET", "exit-minibuffer"),
    // As in Emacs, not the global key's line break.
    ("C-j", "exit-minibuffer"),
    ("C-g", "keyboard-quit"),
    ("M-p", "previous-history-element"),
    ("M-n", "next-history-element"),
    // Right after a yank, the next older kill: no list opens over a prompt (ADR 0091).
    ("M-y", "yank-pop"),
];

/// The keys of a review comment's draft, above the global keymap (ADR 0122): `RET` breaks a
/// line, as a draft holds several, and `C-c C-c` keeps it. It has no history, so no `M-p` or
/// `M-n`.
pub const DRAFT: &[(&str, &str)] = &[
    ("RET", "newline"),
    ("C-c C-c", "review-keep-draft"),
    ("C-g", "keyboard-quit"),
    ("M-y", "yank-pop"),
];

/// The keys of a question, in place of the global keymap: no other key reaches a command.
/// `SPC`, `DEL` and `RET` do not answer, unlike in Emacs, as prose types them most.
pub const QUESTION: &[(&str, &str)] = &[
    ("y", "answer-yes"),
    ("n", "answer-no"),
    ("C-g", "keyboard-quit"),
];

/// Keys only a dev build has, for the end-to-end flows.
#[cfg(feature = "dev")]
pub const DEV: &[(&str, &str)] = &[("<f34>", "dev-read-passphrase")];
