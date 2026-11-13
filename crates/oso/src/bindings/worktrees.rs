//! A `*Worktrees*` buffer's own key bindings (ADR 0121), in a keymap above the global one there:
//! `g` lists the worktrees again and `D` removes the one at point. The buffer is read-only, so
//! these letters run commands rather than type.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run, in a `*Worktrees*` buffer only.
pub const WORKTREES: &[(&str, &str)] = &[("g", "worktree-list"), ("D", "worktree-remove")];
