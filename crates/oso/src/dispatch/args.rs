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
        // A name the agent gives would be text for a prompt that completes, with rules of its
        // own: none is given yet.
        (ArgSpec::Theme { .. }, _) => Err(format!(
            "{name} reads a theme's name in a prompt, through send-keys"
        )),
        (ArgSpec::Buffer { .. }, _) => Err(format!(
            "{name} reads a buffer's name, which only its prompt takes, from keys"
        )),
        (ArgSpec::Mark { .. }, _) => Err(format!(
            "{name} reads a mark, which only its prompt takes, from keys"
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
    }
}

#[cfg(test)]
mod tests {
    use super::{Given, Mapped, map};
    use oso_core::command::{ArgSpec, Args, Prefix};

    const NUMBER: ArgSpec = ArgSpec::Number {
        prompt: "Go to line: ",
        history: "goto-line",
    };
    const TEXT: ArgSpec = ArgSpec::Text {
        prompt: "Insert: ",
        history: "insert",
    };

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
        let buffer = ArgSpec::Buffer {
            prompt: "Switch to buffer: ",
            history: "buffer",
            purpose: oso_core::command::BufferPurpose::Switch,
        };
        assert!(refused(buffer, Given::Text("notes")).contains("buffer's name"));
        let mark = ArgSpec::Mark {
            prompt: "Go to mark: ",
        };
        assert!(refused(mark, Given::Nothing).contains("mark"));
        let theme = ArgSpec::Theme {
            prompt: "Load theme: ",
            history: "theme",
        };
        assert!(refused(theme, Given::Nothing).contains("theme's name"));
    }
}
