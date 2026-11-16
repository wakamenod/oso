//! Default key bindings of the agent session commands (ADR 0143): the Agents menu on `C-c a`
//! everywhere, which `bindings/menus.rs` defines, the keys of `*Spaces*`, and session mode's
//! `C-c C-<letter>` in a session's terminal (ADR 0132, ADR 0143).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Session mode's keys, in a terminal's buffer that hosts a session, with the terminal's own
/// (ADR 0143): `C-c C-<letter>` runs the Agents menu's head of that letter, where the terminal and
/// Emacs leave `C-<letter>` free, and signs keep their `C-c` keys. The menu's other heads are
/// `C-c a` and the letter, as anywhere. `C-c C-c`, `C-c C-k`, `C-c C-j`, `C-c RET` and `C-c C-s`
/// stay the terminal's.
pub const SESSION: &[(&str, &str)] = &[
    ("C-c C-n", "session-next-waiting"),
    ("C-c C-d", "session-next-done"),
    ("C-c C-b", "session-switch"),
    ("C-c C-p", "sessions"),
    ("C-c C-r", "session-resume"),
    ("C-c C-a", "session-adopt"),
    ("C-c C-w", "session-worktree"),
    ("C-c C-e", "session-explain"),
    ("C-c C-f", "session-follow-message"),
    ("C-c C-z", "space-zoom"),
    ("C-c C-v", "review-session"),
    ("C-c @", "session-mention"),
    ("C-c +", "terminal-allow-agent-read"),
    ("C-c -", "terminal-deny-agent-read"),
    ("C-c ?", "session-help"),
    ("C-c C-u", "universal-argument"),
];

/// The keys of `*Spaces*`, above the global ones in it.
pub const LIST: &[(&str, &str)] = &[
    ("RET", "sessions-show"),
    ("n", "sessions-next"),
    ("p", "sessions-previous"),
    ("TAB", "sessions-toggle-fold"),
    ("S-TAB", "sessions-cycle-folds"),
    ("<right>", "sessions-open"),
    ("<left>", "sessions-close"),
    ("q", "sessions"),
    ("g", "sessions-refresh"),
    ("s", "sessions-regroup"),
    ("P", "sessions-pin"),
    ("A", "sessions-archive"),
    ("r", "sessions-resume"),
    ("k", "session-stop"),
    ("e", "sessions-explain"),
    ("?", "sessions-help"),
    ("c", "claude"),
    ("w", "session-worktree"),
    ("a", "space-add-repo"),
    ("M-n", "sessions-next-heading"),
    ("M-p", "sessions-previous-heading"),
    ("/", "session-switch"),
    ("<", "sessions-narrower"),
    (">", "sessions-wider"),
];
