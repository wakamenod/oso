//! Default key bindings of keyboard macros (ADR 0033, ADR 0113): Emacs's `C-x` keys and its
//! function keys.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// The keys that record and replay a keyboard macro.
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-x (", "kmacro-start-macro"),
    ("<f3>", "kmacro-start-macro"),
    ("C-x )", "kmacro-end-macro"),
    ("C-x e", "kmacro-end-and-call-macro"),
    ("<f4>", "kmacro-end-and-call-macro"),
];
