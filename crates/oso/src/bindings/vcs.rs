//! Source control's key bindings (ADR 0127): `C-x g` everywhere, as magit's, and a git status's
//! own keymap above the global one there, from the keys its commands list, with `c` opening
//! the commit menu. A status is read-only, so its letters run commands rather than type. A
//! commit's message and the log of git's runs have keymaps of their own (step 1b).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Global key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[("C-x g", "git-status")];

/// Key sequences and the commands they run, in a git status only.
pub const STATUS: &[(&str, &str)] = crate::commands::vcs::KEYS;

/// Key sequences and the built-in menus they open, in a git status only.
pub const STATUS_MENUS: &[(&str, &str)] = crate::commands::vcs::MENUS;

/// Key sequences and the commands they run, in a commit's message.
pub const MESSAGE: &[(&str, &str)] = crate::commands::vcs::commit::MESSAGE_KEYS;

/// Key sequences and the commands they run, in the log of git's runs.
pub const LOG: &[(&str, &str)] = crate::commands::vcs::commit::LOG_KEYS;

/// Key sequences and the commands they run, in the list of what discards kept.
pub const DISCARDED: &[(&str, &str)] = crate::commands::vcs::discard::LIST_KEYS;
