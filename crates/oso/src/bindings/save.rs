//! Default key bindings of the save and revert commands (ADR 0033, ADR 0034).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[("C-x C-s", "save-buffer"), ("C-x x g", "revert-buffer")];
