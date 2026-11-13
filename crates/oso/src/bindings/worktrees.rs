//! The key bindings of worktree sessions' own buffers (ADR 0121), each in a keymap above the
//! global one there. In `*Brief*`, `C-c C-c` sends the brief and `C-c C-k` drops it, as a commit
//! message is sent or dropped. In `*Worktrees*`, `g` lists the worktrees again and `D` removes
//! the one at point; the buffer is read-only, so these letters run commands rather than type.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run, in a `*Brief*` buffer only.
pub const BRIEF: &[(&str, &str)] = &[("C-c C-c", "brief-send"), ("C-c C-k", "brief-cancel")];

/// Key sequences and the commands they run, in a `*Worktrees*` buffer only.
pub const WORKTREES: &[(&str, &str)] = &[
    ("g", "worktree-list-refresh"),
    ("D", "worktree-list-remove"),
];
