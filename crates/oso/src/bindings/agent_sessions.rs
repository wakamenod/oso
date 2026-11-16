//! Default key bindings of the agent session commands (ADR 0121, Q7): `C-x a` and a key, the
//! keys of `*Spaces*`, and session mode's `C-c` and a key in a session's terminal (ADR 0132).
//! `C-x a O` makes an issue space and `C-x a S` opens the space menu (ADR 0136). `C-x a o` comes
//! with its command, in a later step.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-x a c", "claude"),
    ("C-x a O", "space-new"),
    ("C-x a r", "session-resume"),
    ("C-x a a", "session-adopt"),
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

/// Session mode's keys, in a terminal's buffer that hosts a session, with the terminal's own
/// (ADR 0132): `C-c` and a key runs what `C-x a` and the same key runs elsewhere, and the
/// commands on the session itself take capitals and punctuation. `C-c` and a control key stays
/// the terminal's, but `C-c C-u`, as `C-u` goes to the program.
pub const SESSION: &[(&str, &str)] = &[
    ("C-c c", "claude"),
    ("C-c O", "space-new"),
    ("C-c r", "session-resume"),
    ("C-c a", "session-adopt"),
    ("C-c w", "session-worktree"),
    ("C-c s", "sessions"),
    ("C-c b", "session-switch"),
    ("C-c n", "session-next-waiting"),
    ("C-c d", "session-next-done"),
    ("C-c j", "space-switch"),
    ("C-c z", "space-zoom"),
    ("C-c e", "session-explain"),
    ("C-c m", "session-send"),
    ("C-c @", "session-mention"),
    ("C-c f", "session-follow-message"),
    ("C-c K", "session-stop"),
    ("C-c P", "session-pin"),
    ("C-c A", "session-archive"),
    ("C-c v", "review-session"),
    ("C-c +", "terminal-allow-agent-read"),
    ("C-c -", "terminal-deny-agent-read"),
    ("C-c ?", "session-help"),
    ("C-c C-u", "universal-argument"),
];

/// Session mode's menu keys (ADR 0117, ADR 0132): the review menu, as `C-x v r` elsewhere.
pub const SESSION_MENUS: &[(&str, &str)] = &[("C-c V", "review"), ("C-c S", "space")];

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
