//! Default key bindings of the agent session commands (ADR 0143): the Agents menu on `C-c a`
//! everywhere, which `bindings/menus.rs` defines, the keys of `*Spaces*`, and session mode's
//! `C-c C-<letter>` in a session's terminal (ADR 0132, ADR 0143), which a native session's
//! buffer has too, with its own (ADR 0165).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Session mode's keys, in a terminal's buffer that hosts a session, with the terminal's own
/// (ADR 0143): `C-c C-<letter>` runs the Agents menu's head of that letter, where the terminal and
/// Emacs leave `C-<letter>` free, and signs keep their `C-c` keys. The menu's other heads are
/// `C-c a` and the letter, as anywhere. `C-c C-c`, `C-c C-k`, `C-c C-j`, `C-c RET` and `C-c C-s`
/// stay the terminal's. `C-c C-u` and `C-c C-y`, which yanks the clipboard's image into the
/// session as `C-y` yanks text (ADR 0142), are session mode's own, with no head.
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
    ("C-c C-v", "review-layout"),
    ("C-c @", "session-mention"),
    ("C-c +", "terminal-allow-agent-read"),
    ("C-c -", "terminal-deny-agent-read"),
    ("C-c ?", "session-help"),
    ("C-c C-u", "universal-argument"),
    ("C-c C-y", "session-yank-image"),
];

/// A native session's buffer's own keys (ADR 0165, ADR 0130's design), with session mode's
/// [`SESSION`] keys beside them but [`NOT_NATIVE`]'s: its rows are read, not typed into.
pub const NATIVE: &[(&str, &str)] = &[
    ("TAB", "native-session-toggle"),
    ("RET", "native-session-result"),
    ("n", "native-session-next-prompt"),
    ("p", "native-session-previous-prompt"),
    ("]", "native-session-next-tool"),
    ("[", "native-session-previous-tool"),
    // The composer and the queue (ADR 0165, step 2): `RET` on a queued row takes it back.
    ("i", "native-session-compose"),
    ("C-c C-q", "native-session-queue"),
    // Interrupts the turn under way (step 2c): session mode leaves `C-c C-k` to a terminal,
    // which a native session has none of.
    ("C-c C-k", "native-session-interrupt"),
    ("e", "native-queued-edit"),
    ("d", "native-queued-drop"),
    ("M-<up>", "native-queued-up"),
    ("M-<down>", "native-queued-down"),
];

/// The keys of a native session's composer (ADR 0165, step 2), above the global ones in it.
pub const COMPOSER: &[(&str, &str)] = &[
    ("C-c C-c", "native-composer-send"),
    ("C-c C-k", "native-composer-cancel"),
    ("M-p", "native-composer-previous"),
    ("M-n", "native-composer-next"),
    ("C-c C-o", "native-composer-transcript"),
    ("C-c C-q", "native-session-queue"),
];

/// The keys of a native session's permission card (ADR 0165, step 2e-2), above the global
/// ones in it: a digit chooses a question's option.
pub const CARD: &[(&str, &str)] = &[
    ("y", "native-card-allow"),
    ("n", "native-card-deny"),
    ("N", "native-card-note"),
    ("o", "native-card-other"),
    ("1", "native-card-choose"),
    ("2", "native-card-choose"),
    ("3", "native-card-choose"),
    ("4", "native-card-choose"),
    ("5", "native-card-choose"),
    ("6", "native-card-choose"),
    ("7", "native-card-choose"),
    ("8", "native-card-choose"),
    ("9", "native-card-choose"),
    // The whole call or plan, or a question's chosen options given.
    ("RET", "native-session-result"),
    ("i", "native-session-compose"),
    ("C-c C-o", "native-composer-transcript"),
];

/// Session mode's commands a native session's buffer does not bind: an agent's reads of a
/// terminal are let in and taken back on its screen, which a native session has none of.
pub const NOT_NATIVE: [&str; 2] = ["terminal-allow-agent-read", "terminal-deny-agent-read"];

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
    ("R", "session-rename"),
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

/// The keys of `session-search`'s prompt (ADR 0145), over completion's: `C-c C-s` searches the
/// next scope, `C-RET` reads the selected row's conversation and `M-RET` forks it, in place of
/// completion's `M-RET`, as a search's input is no answer.
pub const CONVERSATION_SEARCH: &[(&str, &str)] = &[
    ("C-c C-s", "session-search-scope"),
    ("C-RET", "session-search-read"),
    ("M-RET", "session-search-fork"),
];

/// The keys of `*conversation: NAME*` (ADR 0145, step 2), a read-only buffer, so letters run
/// commands: the next and previous message, go to its session, fork it, read it again, quit.
pub const CONVERSATION: &[(&str, &str)] = &[
    ("n", "conversation-next-message"),
    ("p", "conversation-previous-message"),
    ("r", "conversation-go"),
    ("f", "conversation-fork"),
    ("g", "conversation-revert"),
    ("q", "quit-window"),
];
