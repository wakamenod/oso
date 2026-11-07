//! Default key bindings of the commands for emergency copies (ADR 0033, ADR 0064).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run, everywhere.
pub const BINDINGS: &[(&str, &str)] = &[("C-h r", "recover-session")];

/// The keys of the list of copies. One key each, as a person answers from here.
pub const LIST: &[(&str, &str)] = &[
    ("v", "emergency-copy-view"),
    ("RET", "emergency-copy-view"),
    ("=", "emergency-copy-compare"),
    ("r", "emergency-copy-recover"),
    ("d", "emergency-copy-delete"),
    ("g", "emergency-copies-refresh"),
    ("q", "emergency-copies-back"),
];

/// The keys of a buffer showing one copy.
pub const COPY: &[(&str, &str)] = &[
    ("=", "emergency-copy-compare"),
    ("r", "emergency-copy-recover"),
    ("q", "emergency-copies-back"),
];
