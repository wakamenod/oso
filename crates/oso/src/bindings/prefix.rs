//! Default key bindings of the prefix argument commands (ADR 0033, ADR 0034,
//! ADR 0058).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-u", "universal-argument"),
    ("M--", "negative-argument"),
    ("M-0", "digit-argument"),
    ("M-1", "digit-argument"),
    ("M-2", "digit-argument"),
    ("M-3", "digit-argument"),
    ("M-4", "digit-argument"),
    ("M-5", "digit-argument"),
    ("M-6", "digit-argument"),
    ("M-7", "digit-argument"),
    ("M-8", "digit-argument"),
    ("M-9", "digit-argument"),
];

/// Keys that build on a prefix argument while it reads digits, in the keymap
/// active then. Any other key does what it always does, with the prefix as its
/// argument.
pub const WHILE_READING: &[(&str, &str)] = &[
    ("-", "negative-argument"),
    ("0", "digit-argument"),
    ("1", "digit-argument"),
    ("2", "digit-argument"),
    ("3", "digit-argument"),
    ("4", "digit-argument"),
    ("5", "digit-argument"),
    ("6", "digit-argument"),
    ("7", "digit-argument"),
    ("8", "digit-argument"),
    ("9", "digit-argument"),
];
