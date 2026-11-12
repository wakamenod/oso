//! Default key bindings of jumps by label (ADR 0033, ADR 0116): the keys that start one, and
//! its own keymap, which takes the place of the others while it is on. A key that keymap does
//! not bind ends the jump, with nothing moved, and does not run, in the dispatcher.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// The global keys that start a jump. `avy-goto-end-of-line` has none, as in avy, and isearch's
/// keymap binds `M-j` to `avy-isearch`.
pub const BINDINGS: &[(&str, &str)] = &[("M-j", "avy-goto-char-timer"), ("M-g f", "avy-goto-line")];

/// The keys of a jump under way. A key that types a character runs [`TEXT_COMMAND`]: a
/// character of the string, or a label's key.
pub const JUMP: &[(&str, &str)] = &[
    ("DEL", "avy-delete-char"),
    ("RET", "avy-show-labels"),
    ("C-g", "keyboard-quit"),
];

/// What a key that types a character runs in a jump.
pub const TEXT_COMMAND: &str = "avy-printing-char";
