//! What a command's argument spec makes of an argument given with it, from the outside:
//! the dev socket's `run-command` or the agent channel (ADR 0044, ADR 0085). Neither ever
//! opens a prompt for an argument; the one given stands for it, or the command is refused.
//!
//! Must not depend on: the socket, the channel or `app`; this only maps a spec and a value.

use oso_core::command::{ArgSpec, Args, Prefix};

/// An argument given with a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Given<'a> {
    Nothing,
    Count(isize),
    Text(&'a str),
}

/// What a command runs with: its arguments, and the answer a prompt would have read.
#[derive(Debug, PartialEq, Eq)]
pub struct Mapped {
    pub args: Args,
    pub answer: Option<String>,
}

/// `given` for the command `name`, whose spec is `spec`, as its arguments. With nothing
/// given, a command that takes a count gets `prefix`, the prefix argument the keys left.
///
/// # Errors
///
/// What is wrong, naming the command: an argument of the wrong kind or one missing, or a
/// passphrase, which only a prompt reads. Each caller bounds counts where it reads them
/// ([`oso_core::command::MAX_REMOTE_COUNT`]).
pub fn map(
    name: &str,
    spec: ArgSpec,
    given: Given<'_>,
    prefix: Option<Prefix>,
) -> Result<Mapped, String> {
    let plain = |args| Ok(Mapped { args, answer: None });
    let answered = |answer: String| {
        Ok(Mapped {
            args: Args::NONE,
            answer: Some(answer),
        })
    };
    match (spec, given) {
        (ArgSpec::Passphrase { .. }, _) => Err(format!(
            "{name} reads a passphrase, which only its prompt takes, from keys"
        )),
        (ArgSpec::Command { .. }, _) => Err(format!(
            "{name} reads a command's name; run the command itself"
        )),
        (ArgSpec::Theme { .. }, _) => Err(format!(
            "{name} reads a theme's name, which only its prompt takes, from keys"
        )),
        // The host's lists, of sessions and spaces, are read only in their prompt (ADR 0121).
        (ArgSpec::Listed { .. }, _) => Err(format!(
            "{name} reads one of a list, which only its prompt takes, from keys"
        )),
        // A menu reads keys, and no channel opens one (ADR 0117).
        (ArgSpec::Menu { .. }, _) => Err(format!(
            "{name} reads a menu's name, which only its prompt takes, from keys"
        )),
        // Only its prompt reads a buffer's name. The agent channel's switch to a buffer by name
        // never comes here: `Dispatcher::run_for_agent` resolves the name to a buffer it may
        // show, and shows it (ADR 0094, ADR 0095).
        (ArgSpec::Buffer { .. }, _) => Err(format!(
            "{name} reads a buffer's name, which only its prompt takes, from keys"
        )),
        (ArgSpec::Mark { .. }, _) => Err(format!(
            "{name} reads a mark, which only its prompt takes, from keys"
        )),
        (ArgSpec::KillEntry { .. }, _) => Err(format!(
            "{name} reads a kill, which only its prompt takes, from keys"
        )),
        // A directory and a name are read only in prompts, with a prefix argument (ADR 0121).
        (ArgSpec::DirectoryAndTextOnPrefix { .. }, Given::Nothing) => plain(Args::NONE),
        (ArgSpec::DirectoryAndTextOnPrefix { .. }, _) => Err(format!(
            "{name} reads a directory and a name, which only its prompts take, from keys"
        )),
        // query-replace asks at each match, from keys (ADR 0113).
        (ArgSpec::Replace { .. }, _) => Err(format!(
            "{name} reads two strings, which only its prompts take, from keys"
        )),
        // A path read with completion opens or writes a file the person typed (ADR 0087), and
        // a project's files are listed only in a prompt (ADR 0114).
        (ArgSpec::File { .. } | ArgSpec::ProjectFile { .. }, _) => Err(format!(
            "{name} reads a path, which only its prompt takes, from keys"
        )),
        // A search's rows are shown only in its prompt, which the channel never sees (ADR 0114).
        (ArgSpec::ProjectSearch { .. }, _) => Err(format!(
            "{name} searches in its prompt, which only keys open"
        )),
        (ArgSpec::None, Given::Nothing) => plain(Args::with_prefix(prefix)),
        (ArgSpec::None, _) => Err(format!("{name} takes no argument")),
        (ArgSpec::Count | ArgSpec::Key | ArgSpec::CountAndClipboard, Given::Nothing) => {
            plain(Args::with_prefix(prefix))
        }
        (ArgSpec::Count | ArgSpec::Key | ArgSpec::CountAndClipboard, Given::Count(n)) => {
            plain(Args::numeric(n))
        }
        (ArgSpec::Count | ArgSpec::Key | ArgSpec::CountAndClipboard, Given::Text(_)) => {
            Err(format!("{name} takes a count, not text"))
        }
        (ArgSpec::Number { .. }, Given::Count(n)) => answered(n.to_string()),
        (ArgSpec::Number { .. }, _) => Err(format!("{name} needs a number")),
        (ArgSpec::Text { .. }, Given::Text(text)) => answered(text.to_owned()),
        (ArgSpec::Text { .. }, _) => Err(format!("{name} needs text")),
        // Given text stands for what `C-u` would ask; none runs it at once.
        (ArgSpec::TextOnPrefix { .. }, Given::Text(text)) => answered(text.to_owned()),
        (ArgSpec::TextOnPrefix { .. }, Given::Nothing) => plain(Args::NONE),
        (ArgSpec::TextOnPrefix { .. }, Given::Count(_)) => {
            Err(format!("{name} takes text or nothing, not a count"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Given, Mapped, map};
    use oso_core::command::{ArgSpec, Args, BufferPurpose, Prefix};

    const NUMBER: ArgSpec = ArgSpec::Number {
        prompt: "Go to line: ",
        history: "goto-line",
    };
    const TEXT: ArgSpec = ArgSpec::Text {
        prompt: "Insert: ",
        history: "insert",
    };

    #[test]
    fn text_on_a_prefix_takes_text_or_nothing_from_the_channel() {
        const ON_PREFIX: ArgSpec = ArgSpec::TextOnPrefix {
            prompt: "Base: ",
            history: "base",
        };
        let given = map("b", ON_PREFIX, Given::Text("main"), None).unwrap();
        assert_eq!(given.answer.as_deref(), Some("main"));
        let none = map("b", ON_PREFIX, Given::Nothing, None).unwrap();
        assert_eq!((none.args, none.answer), (Args::NONE, None));
        assert!(map("b", ON_PREFIX, Given::Count(3), None).is_err());
    }

    #[test]
    fn each_spec_takes_its_kind_of_argument() {
        let prefix = Some(Prefix::Number(4));
        assert_eq!(
            map("a", ArgSpec::Count, Given::Nothing, prefix)
                .unwrap()
                .args,
            Args::with_prefix(prefix)
        );
        assert_eq!(
            map("a", ArgSpec::Count, Given::Count(-3), None)
                .unwrap()
                .args,
            Args::numeric(-3)
        );
        let number = map("goto-line", NUMBER, Given::Count(42), None).unwrap();
        assert_eq!(
            number,
            Mapped {
                args: Args::NONE,
                answer: Some("42".into())
            }
        );
        let text = map("insert", TEXT, Given::Text("hi"), None).unwrap();
        assert_eq!(text.answer.as_deref(), Some("hi"));
        assert_eq!(
            map("a", ArgSpec::None, Given::Nothing, None)
                .unwrap()
                .answer,
            None
        );
    }

    #[test]
    fn refuses_the_wrong_kind_a_missing_one_and_passphrases() {
        let refused = |spec, given| map("cmd", spec, given, None).unwrap_err();
        assert_eq!(
            refused(ArgSpec::None, Given::Count(1)),
            "cmd takes no argument"
        );
        assert_eq!(
            refused(ArgSpec::None, Given::Text("x")),
            "cmd takes no argument"
        );
        assert_eq!(
            refused(ArgSpec::Count, Given::Text("x")),
            "cmd takes a count, not text"
        );
        assert_eq!(refused(NUMBER, Given::Nothing), "cmd needs a number");
        assert_eq!(refused(NUMBER, Given::Text("4")), "cmd needs a number");
        assert_eq!(refused(TEXT, Given::Nothing), "cmd needs text");
        assert_eq!(refused(TEXT, Given::Count(1)), "cmd needs text");
        let passphrase = ArgSpec::Passphrase {
            prompt: "Passphrase: ",
        };
        assert!(refused(passphrase, Given::Nothing).contains("passphrase"));
        assert!(
            !refused(passphrase, Given::Nothing).contains("send-keys"),
            "no dev method's name"
        );
        let command = ArgSpec::Command {
            prompt: "M-x ",
            history: "command",
        };
        assert!(refused(command, Given::Text("forward-char")).contains("command's name"));
        // A list the host keeps holds what an agent must not read, such as sessions' recaps.
        for list in ["sessions", "spaces"] {
            let listed = ArgSpec::Listed {
                list,
                prompt: "Session: ",
                history: "session",
            };
            assert!(map("cmd", listed, Given::Text("x"), None).is_err());
            assert!(map("cmd", listed, Given::Nothing, None).is_err());
        }
        let buffer = ArgSpec::Buffer {
            prompt: "Switch to buffer: ",
            history: "buffer",
            purpose: BufferPurpose::Switch,
        };
        assert!(refused(buffer, Given::Text("notes")).contains("buffer's name"));
        assert!(refused(buffer, Given::Nothing).contains("buffer's name"));
        assert!(refused(buffer, Given::Count(1)).contains("buffer's name"));
        let kill = ArgSpec::Buffer {
            prompt: "Kill buffer: ",
            history: "buffer",
            purpose: BufferPurpose::Kill,
        };
        assert!(refused(kill, Given::Text("notes")).contains("buffer's name"));
        let mark = ArgSpec::Mark {
            prompt: "Go to mark: ",
        };
        assert!(refused(mark, Given::Nothing).contains("mark"));
        let theme = ArgSpec::Theme {
            prompt: "Load theme: ",
            history: "theme",
        };
        assert!(refused(theme, Given::Nothing).contains("theme's name"));
        let file = ArgSpec::File {
            prompt: "Find file:",
            history: "file",
        };
        assert!(refused(file, Given::Text("/etc/hosts")).contains("path"));
        let project_file = ArgSpec::ProjectFile {
            history: "project-file",
        };
        assert!(refused(project_file, Given::Text("src/main.rs")).contains("path"));
        let project_search = ArgSpec::ProjectSearch {
            history: "project-search",
        };
        assert!(refused(project_search, Given::Text("fn main")).contains("searches"));
        assert!(refused(project_search, Given::Nothing).contains("searches"));
    }
}
