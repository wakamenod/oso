//! Default key bindings of `consult-line` (ADR 0033, ADR 0154): the buffer's lines as a list, on
//! consult's `M-s l`.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[("M-s l", "consult-line")];
