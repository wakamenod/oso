//! The local agent rules file, `agent-rules/claude.toml` beside `config.toml` (ADR 0121, Q4):
//! read on the configuration's thread at start and whenever it changes, through the same checks
//! as `config.toml`, and compiled there over the bundled rules. Never read from a project.
//!
//! Must not depend on: `app` or `dispatch`; what it finds goes to the app, which hands the rules
//! to the sessions worker.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use super::checked::{Unread, read};
use super::watch::{Seen, WITHHELD, look, may_hold_secrets};
use crate::sessions::rules::{self, LOCAL_FILE, RuleSet, Source};
use crate::sessions::state::Target;

/// What a look at the rules file found.
#[derive(Debug)]
pub struct RulesReport {
    /// The rules to put in force, or `None` to keep those in force.
    pub rules: Option<RuleSet>,
    /// Lines for `*Messages*`.
    pub notes: Vec<String>,
}

/// What was last said of the file, to say only what is new.
#[derive(Debug, PartialEq, Eq)]
enum Said {
    Missing,
    Refused(String),
    /// A hash of the text read.
    Read(u64),
}

/// The file, and what the thread last saw and said of it.
pub(super) struct RulesWatch {
    file: PathBuf,
    seen: Option<Seen>,
    said: Option<Said>,
}

impl RulesWatch {
    /// Watches the rules file beside `config_file`.
    pub(super) fn beside(config_file: &Path) -> Option<Self> {
        Some(Self {
            file: config_file.parent()?.join(LOCAL_FILE),
            seen: None,
            said: None,
        })
    }

    /// Looks at the file, and reads it if it changed, or if `poked` and it was refused. `None`
    /// if there is nothing new to say.
    pub(super) fn look(&mut self, poked: bool) -> Option<RulesReport> {
        let now = look(&self.file);
        let refused = matches!(self.said, Some(Said::Refused(_)));
        if self.seen.as_ref() == Some(&now) && !(poked && refused) {
            return None;
        }
        let first = self.seen.is_none();
        self.seen = Some(now);
        let (said, report) = match read(&self.file) {
            // The worker starts with the bundled rules: no file at the first look says nothing.
            Err(Unread::Missing) if first => {
                self.said = Some(Said::Missing);
                return None;
            }
            Err(Unread::Missing) => {
                let notes = vec![format!("{LOCAL_FILE}: gone; the bundled rules apply")];
                (
                    Said::Missing,
                    RulesReport {
                        rules: Some(RuleSet::bundled()),
                        notes,
                    },
                )
            }
            Err(Unread::Refused(why)) => {
                let notes = vec![format!(
                    "{LOCAL_FILE} not read: {why}; the rules in force stay"
                )];
                (Said::Refused(why), RulesReport { rules: None, notes })
            }
            // Unlike `config.toml`, nothing in it is safe to apply when a group can change it:
            // every rule says what the person is told about an agent (ADR 0121, Q4).
            Ok((_, true)) => {
                let why = "other users in a group can change it, or the way to it".to_owned();
                let notes = vec![format!(
                    "{LOCAL_FILE} not read: {why}; the rules in force stay"
                )];
                (Said::Refused(why), RulesReport { rules: None, notes })
            }
            Ok((text, false)) => {
                let mut hash = DefaultHasher::new();
                text.hash(&mut hash);
                let report = compile(&text, may_hold_secrets(&self.file));
                (Said::Read(hash.finish()), report)
            }
        };
        if self.said.as_ref() == Some(&said) {
            return None;
        }
        self.said = Some(said);
        Some(report)
    }
}

/// The rules of `text`, the local file's, over the bundled ones, and what to say of them. If
/// `withheld`, as for `config.toml` whose name or where it leads says it may hold secrets, a
/// problem says its line and none of the file's text.
pub(super) fn compile(text: &str, withheld: bool) -> RulesReport {
    let local = rules::parse(text, Source::Local);
    let mut notes: Vec<String> = local
        .problems
        .iter()
        .map(|problem| match problem.split_once(':') {
            Some((line, _)) if withheld => format!("{LOCAL_FILE}:{line}: {WITHHELD}"),
            _ => format!("{LOCAL_FILE}:{problem}"),
        })
        .collect();
    if local.more > 0 {
        notes.push(format!("{LOCAL_FILE}: {} more problems", local.more));
    }
    if local.refused {
        notes.push(format!("{LOCAL_FILE} not applied; the rules in force stay"));
        return RulesReport { rules: None, notes };
    }
    let set = RuleSet::merged(rules::bundled(), local);
    if let Some(warning) = hold_over_need(&set, withheld) {
        notes.push(warning);
    }
    notes.push(format!(
        "Agent rules reloaded: {} local, {} bundled, {} bundled disabled",
        set.local, set.bundled, set.disabled
    ));
    RulesReport {
        rules: Some(set),
        notes,
    }
}

/// A warning if a hold rule ranks above a needs-input rule, one of them local: the view it
/// holds for could hide that dialog. Names only what the file's checks allowed in a name.
fn hold_over_need(set: &RuleSet, withheld: bool) -> Option<String> {
    let rules = set.rules();
    // Highest priority first: a hold rule before a needs-input rule ranks above it.
    let (hold, need) = rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| rule.target == Target::Hold)
        .find_map(|(at, hold)| {
            let need = rules[at + 1..].iter().find(|rule| {
                matches!(rule.target, Target::NeedsInput(_))
                    && (rule.source == Source::Local || hold.source == Source::Local)
            })?;
            Some((hold, need))
        })?;
    Some(if withheld {
        format!(
            "{LOCAL_FILE}: a hold rule ranks above a needs-input rule, so a view can hide a dialog"
        )
    } else {
        format!(
            "{LOCAL_FILE}: hold rule {} ranks above needs-input rule {}, so its view can hide \
             that dialog; give it a lower priority",
            hold.name, need.name
        )
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{RulesWatch, compile};

    #[test]
    fn a_good_file_reloads_and_says_so() {
        let report = compile(
            "version = 1\n[[rule]]\nname = \"prompt\"\ndisable = true\n[[rule]]\nname = \"x\"\n\
             state = \"working\"\nregion = \"title\"\ncontains = [\"busy\"]\n",
            false,
        );
        let rules = report.rules.unwrap();
        assert_eq!((rules.local, rules.disabled), (1, 1));
        assert_eq!(report.notes.len(), 1);
        assert!(report.notes[0].starts_with("Agent rules reloaded: 1 local"));
    }

    #[test]
    fn a_broken_file_keeps_the_rules_in_force_and_says_where() {
        let report = compile("version = 1\n[[rule]\n", false);
        assert!(report.rules.is_none());
        assert!(report.notes[0].starts_with("agent-rules/claude.toml:2: "));
        assert!(report.notes.last().unwrap().contains("not applied"));
    }

    #[test]
    fn the_watch_says_only_what_is_new() {
        let dir = std::env::temp_dir().join(format!("oso-agent-rules-{}", std::process::id()));
        fs::create_dir_all(dir.join("agent-rules")).unwrap();
        let mut watch = RulesWatch::beside(&dir.join("config.toml")).unwrap();
        assert!(
            watch.look(false).is_none(),
            "no file at first: the bundled rules stand"
        );
        assert!(watch.look(true).is_none());
        let file = dir.join("agent-rules/claude.toml");
        fs::write(&file, "version = 1\n").unwrap();
        let read = watch.look(false).unwrap();
        assert!(read.notes[0].contains("0 local"));
        assert!(watch.look(false).is_none(), "nothing changed");
        fs::remove_file(&file).unwrap();
        let gone = watch.look(false).unwrap();
        assert!(gone.notes[0].contains("gone"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_withheld_files_problems_say_their_line_and_none_of_its_text() {
        let report = compile(
            "version = 1\n[[rule]]\nname = \"token-hunter2\"\nstate = \"bogus\"\n\
             region = \"title\"\ncontains = [\"x\"]\n",
            true,
        );
        let problem = &report.notes[0];
        assert!(problem.starts_with("agent-rules/claude.toml:"), "{problem}");
        assert!(problem.contains(super::WITHHELD), "{problem}");
        assert!(
            !problem.contains("hunter2") && !problem.contains("bogus"),
            "{problem}"
        );
        let plain = compile(
            "version = 1\n[[rule]]\nname = \"token-hunter2\"\nstate = \"bogus\"\n\
             region = \"title\"\ncontains = [\"x\"]\n",
            false,
        );
        assert!(plain.notes[0].contains("hunter2"), "{}", plain.notes[0]);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_a_group_can_change_is_refused_and_the_rules_in_force_stay() {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("oso-agent-rules-group-{}", std::process::id()));
        fs::create_dir_all(dir.join("agent-rules")).unwrap();
        for sub in [&dir, &dir.join("agent-rules")] {
            fs::set_permissions(sub, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut watch = RulesWatch::beside(&dir.join("config.toml")).unwrap();
        assert!(watch.look(false).is_none());
        let file = dir.join("agent-rules/claude.toml");
        fs::write(&file, "version = 1\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o664)).unwrap();
        let report = watch.look(false).unwrap();
        assert!(report.rules.is_none(), "nothing applies");
        assert!(report.notes[0].contains("not read"), "{:?}", report.notes);
        assert!(report.notes[0].contains("group"), "{:?}", report.notes);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let read = watch.look(false).unwrap();
        assert!(read.rules.is_some() && read.notes[0].contains("0 local"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_hold_rule_ranked_above_a_dialog_is_warned_of() {
        let report = compile(
            "version = 1\n[[rule]]\nname = \"my-view\"\nstate = \"hold\"\npriority = 2000\n\
             region = \"bottom(1)\"\ncontains = [\"esc\"]\n",
            false,
        );
        assert!(report.rules.is_some(), "applied all the same");
        let warning = report
            .notes
            .iter()
            .find(|note| note.contains("ranks above"));
        let warning = warning.expect("a warning");
        assert!(warning.contains("my-view"), "{warning}");
        let low = compile(
            "version = 1\n[[rule]]\nname = \"my-view\"\nstate = \"hold\"\npriority = 10\n\
             region = \"bottom(1)\"\ncontains = [\"esc\"]\n",
            false,
        );
        assert!(!low.notes.iter().any(|note| note.contains("ranks above")));
    }
}
