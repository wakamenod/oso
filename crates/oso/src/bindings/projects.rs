//! Default key bindings of the commands of projects (ADR 0033, ADR 0114).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[("C-x p f", "project-find-file")];
