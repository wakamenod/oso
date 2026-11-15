//! Source control's key bindings (ADR 0127): `C-x g` everywhere, as magit's, and a git status's
//! own keymap above the global one there, from the keys its commands list. A status is
//! read-only, so its letters run commands rather than type.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Global key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[("C-x g", "git-status")];

/// Key sequences and the commands they run, in a git status only.
pub const STATUS: &[(&str, &str)] = crate::commands::vcs::KEYS;
