//! Claude Code's settings that connect each session to claude.ai as it starts (ADR 0189): looked
//! at on the configuration's thread beside `config.toml`, by the same stamps, so that a native
//! start knows them as it is ordered. How they are read is `sessions::start::connect::at_startup`'s;
//! a start thread reads them again before its line.
//!
//! Must not depend on: `app` or `dispatch`; what it finds goes to the app.

use super::watch::{Seen, look};
use crate::sessions::start::connect::at_startup::{Files, Reading};

/// The files, and what the thread last saw and said of them.
pub(super) struct ClaudeWatch {
    files: Files,
    seen: Option<Vec<Seen>>,
    said: Option<Reading>,
}

impl ClaudeWatch {
    /// Watches this user's files; in a test, none, so that no test reads the person's own.
    pub(super) fn person() -> Self {
        #[cfg(not(test))]
        let files = Files::person();
        #[cfg(test)]
        let files = Files {
            user: None,
            managed: std::env::temp_dir().join("oso-test-no-managed-settings"),
        };
        Self::of(files)
    }

    pub(super) fn of(files: Files) -> Self {
        Self {
            files,
            seen: None,
            said: None,
        }
    }

    /// Looks at the files, and reads them if any changed, or if `poked` and one could not be
    /// read. What they say, the first time and when it changed.
    pub(super) fn look(&mut self, poked: bool) -> Option<Reading> {
        let now: Vec<Seen> = self.files.paths().iter().map(|path| look(path)).collect();
        let unread = matches!(self.said, Some(Reading::Unread(_)));
        if self.seen.as_ref() == Some(&now) && !(poked && unread) {
            return None;
        }
        self.seen = Some(now);
        let reading = self.files.read();
        if self.said.as_ref() == Some(&reading) {
            return None;
        }
        self.said = Some(reading.clone());
        Some(reading)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::ClaudeWatch;
    use crate::sessions::start::connect::at_startup::{Files, Reading};

    #[test]
    fn it_says_the_first_reading_and_each_change_once() {
        let dir = std::env::temp_dir().join(format!("oso-claude-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let user = dir.join("settings.json");
        let mut watch = ClaudeWatch::of(Files {
            user: Some(user.clone()),
            managed: dir.join("managed"),
        });
        assert_eq!(watch.look(false), Some(Reading::Off));
        assert_eq!(watch.look(true), None);
        fs::write(&user, r#"{"remoteControlAtStartup": true}"#).unwrap();
        assert_eq!(watch.look(false), Some(Reading::On));
        assert_eq!(watch.look(false), None);
        fs::write(&user, r#"{"remoteControlAtStartup": "yes", "padding": 1}"#).unwrap();
        assert!(matches!(watch.look(false), Some(Reading::Unread(_))));
        assert_eq!(watch.look(true), None, "said once");
        fs::remove_file(&user).unwrap();
        assert_eq!(watch.look(false), Some(Reading::Off));
        let _ = fs::remove_dir_all(&dir);
    }
}
