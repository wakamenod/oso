//! A session's own `--settings` (ADR 0121, Q15; ADR 0165): the JSON of the parts oso turns on
//! for one session, the hook that steers Claude away from its worktree tools
//! (`worktree::steer`) and Remote Control turned off. Nothing is written into Claude Code's
//! settings files.
//!
//! Must not depend on: `app`, `dispatch`, the file system or sockets; plain functions over
//! values.

use std::fmt;
use std::path::Path;

use serde_json::{Map, Value, json};

use super::worktree::steer::{HOOK, MATCHER};

/// The setting that turns Remote Control off: the CLI then refuses a request to connect the
/// session to claude.ai. No project's setting turns it back on. It is no boundary (ADR 0165).
const DISABLE_REMOTE_CONTROL: &str = "disableRemoteControl";

/// The person's setting that connects each session as it starts.
const AT_STARTUP: &str = "remoteControlAtStartup";

/// What a session's `--settings` says of Remote Control.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RemoteControl {
    /// Nothing: a terminal's, where the person's own settings decide (ADR 0165).
    #[default]
    Own,
    /// It is off: every native session's, and every one-shot run's.
    Off,
    /// It is not off, and only oso's request connects it (ADR 0181): the person's setting that
    /// connects each session as it starts is off there. Not measured with a connection.
    Asked,
}

/// What a session's `--settings` holds, built from parts so that later parts (a status line,
/// ADR 0126) join the same object: a line has one `--settings`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Parts {
    /// `[sessions] steer_worktrees`: install the hook.
    pub steer_worktrees: bool,
    pub remote_control: RemoteControl,
}

/// Why no `--settings` can be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsError {
    /// oso's own path is not UTF-8, and JSON holds only text.
    NotUtf8,
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUtf8 => f.write_str("oso's own path is not UTF-8, so a hook cannot name it"),
        }
    }
}

impl std::error::Error for SettingsError {}

/// The `--settings` JSON for `parts`, `None` when no part is on. With `steer_worktrees`, a
/// `PreToolUse` hook for `EnterWorktree` and `Bash` that runs `oso` (quoted for a POSIX shell)
/// with [`HOOK`]. With Remote Control off, `disableRemoteControl` is
/// true.
///
/// # Errors
///
/// [`SettingsError::NotUtf8`] when a part needs `oso` and its path is not UTF-8.
pub fn settings(oso: &Path, parts: &Parts) -> Result<Option<String>, SettingsError> {
    let mut settings = Map::new();
    if parts.steer_worktrees {
        let oso = oso.to_str().ok_or(SettingsError::NotUtf8)?;
        let command = format!("{} {}", shell_quote(oso), HOOK.join(" "));
        let hooks = json!({
            "PreToolUse": [{
                "matcher": MATCHER,
                "hooks": [{"type": "command", "command": command}],
            }]
        });
        settings.insert("hooks".to_owned(), hooks);
    }
    match parts.remote_control {
        RemoteControl::Own => None,
        RemoteControl::Off => settings.insert(DISABLE_REMOTE_CONTROL.to_owned(), Value::Bool(true)),
        RemoteControl::Asked => settings.insert(AT_STARTUP.to_owned(), Value::Bool(false)),
    };
    Ok((!settings.is_empty()).then(|| Value::Object(settings).to_string()))
}

/// The `--settings` that turn Remote Control off alone, for a run that is no session.
pub fn remote_control_off() -> String {
    json!({ DISABLE_REMOTE_CONTROL: true }).to_string()
}

/// `text` single-quoted for a POSIX shell: each `'` becomes `'\''`.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests;
