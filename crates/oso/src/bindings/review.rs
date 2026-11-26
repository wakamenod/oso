//! A review's key bindings (ADR 0122): its own keymap in a review buffer, from the keys its
//! commands list. `review-layout` is the agents menu's `v`, `C-c a v` (ADR 0184).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run, in a review buffer only.
pub const REVIEW: &[(&str, &str)] = crate::commands::review::KEYS;

/// Key sequences and the commands they run, in a review's message only (ADR 0122, step 12).
pub const MESSAGE: &[(&str, &str)] = crate::commands::review::MESSAGE_KEYS;

/// Key sequences and the commands they run, in a review's files pane only (ADR 0122, step 6b).
pub const FILES: &[(&str, &str)] = crate::commands::review::FILES_KEYS;

/// Key sequences and the commands they run, in a pull request's commit list only (ADR 0167).
pub const COMMITS: &[(&str, &str)] = crate::commands::review::COMMITS_KEYS;
