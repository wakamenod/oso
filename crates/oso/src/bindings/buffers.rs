//! Default key bindings of the commands that go between buffers (ADR 0030, ADR 0033).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[("M-,", "previous-buffer"), ("M-/", "next-buffer")];
