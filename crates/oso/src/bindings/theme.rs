//! Default key bindings of the commands for themes (ADR 0033, ADR 0089).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[("<f5>", "toggle-theme")];
