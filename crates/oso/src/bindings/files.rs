//! Default key bindings of the commands for files (ADR 0033, ADR 0087, ADR 0148).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-x C-f", "find-file"),
    ("C-x C-r", "recentf-open"),
    ("C-x C-w", "write-file"),
];
