//! Default key binding of dictation (ADR 0175): Option-D on macOS, where Option is Super.
//! `dictation-cancel` has no key; `M-x` runs it.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[("s-d", "dictation-toggle")];
