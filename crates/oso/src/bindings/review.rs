//! A review's key bindings (ADR 0122): its own keymap in a review buffer, from the keys its
//! commands list. `C-x v r`, which opens the review menu, is with the built-in menus' keys.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run, in a review buffer only.
pub const REVIEW: &[(&str, &str)] = crate::commands::review::KEYS;
