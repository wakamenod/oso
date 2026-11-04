//! Default key bindings of the session commands (ADR 0033, ADR 0034).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-g", "keyboard-quit"),
    ("C-x C-c", "save-buffers-kill-terminal"),
];
