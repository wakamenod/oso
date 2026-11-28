//! Whether Claude Code's settings connect each session to claude.ai as it starts,
//! `remoteControlAtStartup` (ADR 0188), read as CLI 2.1.331 reads it: a managed settings
//! document's value before the person's own, on only for `true`, and a `false` in the project's
//! or the local settings of the session's directory turns it off. A project's `true` does nothing.
//!
//! Must not depend on: `app` or the UI; it reads a few small files on the thread that asks: the
//! configuration's, and a session's start thread.
//!
//! - Read: `settings.json` in Claude Code's configuration directory (`$CLAUDE_CONFIG_DIR`, else
//!   `~/.claude`), and `managed-settings.json` and `managed-settings.d/*.json` in the system's
//!   managed directory. Not read: a managed preferences plist, Windows' policy key, settings an
//!   organization serves, and the legacy `~/.claude.json`.
//! - Fails closed: a file that is not a regular one, larger than [`MAX_BYTES`], not UTF-8 JSON
//!   of an object, or whose value is not `true` or `false`, reads as off, and says which file. A
//!   missing file says nothing. Nothing quotes a file.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The setting.
pub const KEY: &str = "remoteControlAtStartup";

/// The largest settings file read.
pub const MAX_BYTES: u64 = 256 * 1024;

/// The most documents of `managed-settings.d` read; more reads as off.
const MAX_DOCUMENTS: usize = 64;

/// The managed settings' directory, as the CLI has it.
#[cfg(target_os = "macos")]
const MANAGED: &str = "/Library/Application Support/ClaudeCode";
#[cfg(windows)]
const MANAGED: &str = r"C:\Program Files\ClaudeCode";
#[cfg(not(any(target_os = "macos", windows)))]
const MANAGED: &str = "/etc/claude-code";

/// What the settings say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Reading {
    /// Not read, or not set, or `false`.
    #[default]
    Off,
    On,
    /// A file could not be read, so it is off: which, and why.
    Unread(String),
}

impl Reading {
    /// Whether a session connects as it starts.
    pub fn on(&self) -> bool {
        *self == Self::On
    }
}

/// The person's settings files, and the managed ones'.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Files {
    /// `settings.json` in Claude Code's configuration directory, if there is one.
    pub user: Option<PathBuf>,
    /// The managed settings' directory.
    pub managed: PathBuf,
}

impl Files {
    /// The files this user's `claude` reads.
    pub fn person() -> Self {
        let user = crate::sessions::registry::directory()
            .and_then(|sessions| sessions.parent().map(|dir| dir.join("settings.json")));
        Self {
            user,
            managed: PathBuf::from(MANAGED),
        }
    }

    /// Every file and directory read, for a look at whether they changed.
    pub fn paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![
            self.managed.join("managed-settings.json"),
            self.managed.join("managed-settings.d"),
        ];
        paths.extend(self.documents().unwrap_or_default());
        paths.extend(self.user.clone());
        paths
    }

    /// What they say: a managed document's `false` turns it off, else one's `true` on, else the
    /// person's own `true` turns it on.
    pub fn read(&self) -> Reading {
        let mut managed_on = false;
        let mut documents = vec![self.managed.join("managed-settings.json")];
        match self.documents() {
            Ok(more) => documents.extend(more),
            Err(why) => return Reading::Unread(why),
        }
        for document in &documents {
            match value_in(document) {
                Ok(Some(false)) => return Reading::Off,
                Ok(Some(true)) => managed_on = true,
                Ok(None) => {}
                Err(why) => return Reading::Unread(why),
            }
        }
        if managed_on {
            return Reading::On;
        }
        let Some(user) = &self.user else {
            return Reading::Off;
        };
        match value_in(user) {
            Ok(Some(true)) => Reading::On,
            Ok(_) => Reading::Off,
            Err(why) => Reading::Unread(why),
        }
    }

    /// The documents of `managed-settings.d`, in their names' order: none if it is missing.
    fn documents(&self) -> Result<Vec<PathBuf>, String> {
        let dir = self.managed.join("managed-settings.d");
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(format!("{} could not be listed", dir.display())),
        };
        let mut documents: Vec<PathBuf> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|_| format!("{} could not be listed", dir.display()))?;
            let name = entry.file_name();
            let json = Path::new(&name)
                .extension()
                .is_some_and(|ext| ext == "json");
            if json && !name.to_string_lossy().starts_with('.') {
                documents.push(entry.path());
            }
            if documents.len() > MAX_DOCUMENTS {
                return Err(format!("{} holds too many documents", dir.display()));
            }
        }
        documents.sort();
        Ok(documents)
    }
}

/// Whether a `false` in the project's or the local settings of `directory` turns it off: the
/// CLI's own rule, which only takes away. `Err` with why where one cannot be read, as off.
///
/// # Errors
///
/// Which file could not be read, and why.
pub fn project_off(directory: &Path) -> Result<bool, String> {
    let claude = directory.join(".claude");
    for name in ["settings.json", "settings.local.json"] {
        if value_in(&claude.join(name))? == Some(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The setting's value in `file`: `None` where the file or the key is missing.
fn value_in(file: &Path) -> Result<Option<bool>, String> {
    let shown = file.display();
    let text = match read_bounded(file) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(None),
        Err(why) => return Err(format!("{shown} could not be read: {why}")),
    };
    let Ok(Value::Object(settings)) = serde_json::from_str::<Value>(&text) else {
        return Err(format!(
            "{shown} could not be read: it is not a JSON object"
        ));
    };
    match settings.get(KEY) {
        None => Ok(None),
        Some(Value::Bool(on)) => Ok(Some(*on)),
        Some(_) => Err(format!(
            "{shown} could not be read: its {KEY} is not true or false"
        )),
    }
}

/// The text of `file` through links, a regular file of at most [`MAX_BYTES`] of UTF-8; `None`
/// where there is none. A pipe or a device is not opened, nor waited for if swapped in.
fn read_bounded(file: &Path) -> Result<Option<String>, &'static str> {
    let (opened, metadata) = match oso_platform::file_meta::open_for_reading(file) {
        Ok(Some(opened)) => opened,
        Ok(None) => return Err("it is not a regular file"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("it cannot be opened"),
    };
    if metadata.len() > MAX_BYTES {
        return Err("it is too large");
    }
    let mut bytes = Vec::new();
    (opened.take(MAX_BYTES + 1).read_to_end(&mut bytes)).map_err(|_| "reading it failed")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("it is too large");
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "it is not UTF-8")
}

#[cfg(test)]
mod tests;
