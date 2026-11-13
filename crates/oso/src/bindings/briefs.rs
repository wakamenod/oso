//! A `*Brief*` buffer's own key bindings (ADR 0121), in a keymap above the global one there:
//! `C-c C-c` sends the brief of a worktree session, and `C-c C-k` drops it, as a commit message
//! is sent or dropped.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run, in a `*Brief*` buffer only.
pub const BRIEF: &[(&str, &str)] = &[("C-c C-c", "brief-send"), ("C-c C-k", "brief-cancel")];
