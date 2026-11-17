//! Default key bindings of the help commands (ADR 0033, ADR 0150).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[("C-h k", "describe-key"), ("C-h l", "view-lossage")];
