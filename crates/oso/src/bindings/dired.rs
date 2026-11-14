//! The directory editor's key bindings (ADR 0134): `C-x d` and `C-x C-j` everywhere, and a
//! listing's own keymap above the global one there, from the keys its commands list. A listing
//! is read-only, so its letters run commands rather than type. Nothing is bound under `M-s`,
//! the global search prefix.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Global key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[("C-x d", "dired"), ("C-x C-j", "dired-jump")];

/// Key sequences and the commands they run, in a listing only.
pub const LISTING: &[(&str, &str)] = crate::commands::dired::KEYS;
