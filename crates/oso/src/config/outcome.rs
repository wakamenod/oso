//! What a look at the configuration file comes to: the user's keys to put in, the lines for
//! `*Messages*`, the one for the echo area, and whether something is still wrong (ADR 0108,
//! ADR 0110). A pure function, so the app only carries it out.
//!
//! Must not depend on: `app` or `dispatch`; this only words what the thread found.

use oso_core::keymap::Keymap;

use super::Report;

/// What the app does with a [`Report`].
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The user's keys to put in, in place of those in force; `None` keeps them.
    pub keys: Option<Keymap>,
    /// Lines for `*Messages*` alone (`Editor::note`).
    pub notes: Vec<String>,
    /// The line for the echo area and `*Messages*`, or `None` to say it only in the log.
    pub said: Option<String>,
    /// Whether something is wrong, so that the next good look says so.
    pub troubled: bool,
}

/// What `report` comes to, for the file called `name`, after a look that found something wrong
/// if `troubled`.
pub fn outcome(report: Report, name: &str, troubled: bool) -> Outcome {
    match report {
        Report::Missing => {
            log::info!("no configuration file: the defaults apply");
            Outcome {
                keys: Some(Keymap::new()),
                notes: Vec::new(),
                said: None,
                troubled: false,
            }
        }
        Report::Refused(why) => Outcome {
            keys: None,
            notes: Vec::new(),
            said: Some(format!("Configuration not read: {why}")),
            troubled: true,
        },
        Report::Read {
            loaded,
            group_writes,
        } => {
            let mut notes = Vec::new();
            if group_writes {
                notes.push(format!(
                    "{name}: the users of its group, or of its directory's, can change it"
                ));
            }
            notes.extend(
                loaded
                    .problems
                    .iter()
                    .map(|problem| format!("{name}:{problem}")),
            );
            if loaded.more > 0 {
                notes.push(format!("{name}: {} more problems", loaded.more));
            }
            let count = loaded.problems.len() + loaded.more + usize::from(group_writes);
            let problems = if count == 1 { "problem" } else { "problems" };
            let applied = loaded.keys.is_some();
            let said = if !applied {
                Some(format!(
                    "Configuration not loaded: {count} {problems}, see *Messages*"
                ))
            } else if count > 0 {
                Some(format!("Configuration: {count} {problems}, see *Messages*"))
            } else if troubled {
                Some("Configuration loaded".to_owned())
            } else {
                log::info!("configuration loaded");
                None
            };
            Outcome {
                keys: loaded.keys,
                notes,
                said,
                troubled: !applied || count > 0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use oso_core::keymap::Keymap;

    use super::{Outcome, outcome};
    use crate::config::{Loaded, Problem, Report, Settings};

    fn read(keys: Option<Keymap>, problems: usize, more: usize, group_writes: bool) -> Report {
        let problem = |line| Problem {
            line,
            column: 1,
            message: "wrong".to_owned(),
        };
        Report::Read {
            loaded: Loaded {
                keys,
                settings: Settings::default(),
                problems: (1..=problems).map(problem).collect(),
                more,
            },
            group_writes,
        }
    }

    #[test]
    fn a_good_file_puts_its_keys_in_and_says_so_only_after_a_bad_one() {
        let quiet = outcome(read(Some(Keymap::new()), 0, 0, false), "config.toml", false);
        assert_eq!(
            quiet,
            Outcome {
                keys: Some(Keymap::new()),
                notes: Vec::new(),
                said: None,
                troubled: false,
            }
        );
        let after = outcome(read(Some(Keymap::new()), 0, 0, false), "config.toml", true);
        assert_eq!(after.said.as_deref(), Some("Configuration loaded"));
        assert!(!after.troubled);
    }

    #[test]
    fn problems_are_notes_with_the_files_name_and_one_line_counts_them() {
        let said = outcome(read(Some(Keymap::new()), 2, 3, true), "config.toml", false);
        assert_eq!(
            said.notes,
            [
                "config.toml: the users of its group, or of its directory's, can change it",
                "config.toml:1:1: wrong",
                "config.toml:2:1: wrong",
                "config.toml: 3 more problems",
            ]
        );
        assert_eq!(
            said.said.as_deref(),
            Some("Configuration: 6 problems, see *Messages*")
        );
        assert!(said.keys.is_some() && said.troubled);
        let one = outcome(read(Some(Keymap::new()), 1, 0, false), "config.toml", false);
        assert_eq!(
            one.said.as_deref(),
            Some("Configuration: 1 problem, see *Messages*")
        );
    }

    #[test]
    fn a_rejected_or_refused_file_keeps_the_keys_in_force_and_a_missing_one_drops_them() {
        let rejected = outcome(read(None, 1, 0, false), "config.toml", false);
        assert_eq!(rejected.keys, None);
        assert_eq!(
            rejected.said.as_deref(),
            Some("Configuration not loaded: 1 problem, see *Messages*")
        );
        assert!(rejected.troubled);
        let refused = outcome(Report::Refused("why".to_owned()), "config.toml", false);
        assert_eq!(refused.keys, None);
        assert_eq!(refused.said.as_deref(), Some("Configuration not read: why"));
        assert!(refused.troubled);
        let missing = outcome(Report::Missing, "config.toml", true);
        assert_eq!(missing.keys, Some(Keymap::new()));
        assert_eq!(missing.said, None);
        assert!(!missing.troubled);
    }
}
