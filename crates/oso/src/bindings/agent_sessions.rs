//! Default key bindings of the agent session commands (ADR 0121, Q7): `C-x a` and a key, and
//! the keys of `*Sessions*`. `C-x a w` and `C-x a o` come with their commands, in later steps.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-x a c", "claude"),
    ("C-x a r", "session-resume"),
    ("C-x a w", "session-worktree"),
    ("C-x a s", "sessions"),
    ("C-x a b", "session-switch"),
    ("C-x a n", "session-next-waiting"),
    ("C-x a d", "session-next-done"),
    ("C-x a j", "space-switch"),
    ("C-x a z", "space-zoom"),
    ("C-x a e", "session-explain"),
    ("C-x a m", "session-send"),
    ("C-x a @", "session-mention"),
    ("C-x a f", "session-follow-message"),
];

/// The keys of `*Sessions*`, above the global ones in it.
pub const LIST: &[(&str, &str)] = &[
    ("RET", "sessions-show"),
    ("n", "sessions-next"),
    ("p", "sessions-previous"),
    ("TAB", "sessions-toggle-fold"),
    ("g", "sessions-refresh"),
    ("s", "sessions-regroup"),
    ("P", "sessions-pin"),
    ("A", "sessions-archive"),
    ("r", "sessions-resume"),
    ("k", "session-stop"),
    ("e", "sessions-explain"),
];
