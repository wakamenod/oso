//! Default key bindings of the commands for files (ADR 0033, ADR 0087).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[("C-x C-f", "find-file"), ("C-x C-w", "write-file")];
