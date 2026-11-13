//! Default key bindings of the agent session commands (ADR 0121, Q7): `C-x a` and a key. The
//! rest of `C-x a` comes with the sessions list (ADR 0121 step 3).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[("C-x a c", "claude"), ("C-x a r", "session-resume")];
