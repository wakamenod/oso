//! Default key bindings of the commands that change how a buffer is shown (ADR 0100).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[("C-x x t", "toggle-truncate-lines")];
