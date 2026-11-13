//! Commands for agent sessions (ADR 0121): starting a hosted `claude`, resuming and forking a
//! stopped one, and explaining a saved screen with the rules in force, for writing a rule; and
//! seeing what needs the person: `*Sessions*`, the switch float, the next session waiting or
//! done, spaces and zoom, and a session's live explanation. `list` holds `*Sessions*`'s own
//! keys. `session.rs` beside it is the editing session's: cancelling and exiting.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host, whose start thread and sessions worker do the work.
//!
//! What starts or resumes a session needs a person at the keyboard: a session runs a process.
//! So does what puts a session's terminal or `*Sessions*` in front of the person's keys, as the
//! host selects the window after the command (the author's choice, 2026-11-13).
//! `session-switch` and `session-explain` are `Look`: the first completes over a list the agent
//! channel cannot answer, and the second shows only a buffer withheld from agents.

mod list;

pub use list::COMMANDS as LIST_COMMANDS;
pub use list::ROW_MARKS;

use std::path::PathBuf;

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::SessionRequest;

/// The list `session-switch` completes over: each session's buffer name.
pub const SESSIONS_LIST: &str = "sessions";

/// The list `space-switch` completes over: each space's name.
pub const SPACES_LIST: &str = "spaces";

/// The sessions commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "claude",
        doc: "Start a Claude Code session in a terminal buffer, `*claude: NAME*`.\n\nIt runs in the selected buffer's project root, else its file's directory, as a new terminal does, else the directory oso started in. It is named after that directory, then `-2`, `-3` for more there. With `C-u`, a directory is read first, then a name. oso gives the session its id, its name and an `oso` MCP server for this instance, and no permission option: it starts in the mode your own Claude Code settings give it. It runs as your login shell would, or `[sessions] claude` names the executable. It turns the agent channel on, as the session's tools need it.",
        args: ArgSpec::DirectoryAndTextOnPrefix {
            directory: "Session in directory:",
            directory_history: "session-directory",
            prompt: "Session name:",
            history: "session-name",
        },
        reach: Reach::Person,
        run: claude,
        state: None,
    },
    Command {
        name: "session-resume",
        doc: "Resume the stopped session whose terminal is this buffer, in the same buffer.\n\nIt is refused while Claude Code's registry has a live process for the same conversation, which it names, and it then offers `session-fork`. Five seconds after it resumes, the registry is read again, and two live processes for one conversation give a warning.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: session_resume,
        state: None,
    },
    Command {
        name: "session-fork",
        doc: "Copy the conversation of the session whose terminal is this buffer under a new id, and run the copy in a new terminal buffer.\n\nThe copy runs beside the original, which goes on as it was.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: session_fork,
        state: None,
    },
    Command {
        name: "session-explain-file",
        doc: "Explain a saved screen of a hosted session, its path read with completion, with the rules in force.\n\n`*Session explain*` shows the state the rules give, the rule that matched and whether it is bundled or local, the region's text, or why no rule matched. It reads at most 256 KiB, the last 40 non-empty lines, for writing a rule in `agent-rules/claude.toml` beside `config.toml`. Only the person runs it: the screen may hold what an agent must not read.",
        args: ArgSpec::File {
            prompt: "Explain screen file:",
            history: "file",
        },
        reach: Reach::Person,
        run: session_explain_file,
        state: None,
    },
    Command {
        name: "sessions",
        doc: "Show `*Sessions*` in the side window at the frame's left, and select it; select it if it is shown; hide it if it is selected.

The list groups the sessions by state, most urgent first, or by space. `C-x o` passes the side window over, and `C-x 1` keeps it. Once hidden, it does not show by itself again until oso restarts.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: sessions,
        state: None,
    },
    Command {
        name: "session-switch",
        doc: "Show a session, read with completion over the sessions, switching to its space.

Each session is its buffer's name, with its state, space, branch and recap after it: `&needs` keeps those that need input.",
        args: ArgSpec::Listed {
            list: SESSIONS_LIST,
            prompt: "Session:",
            history: "session",
        },
        reach: Reach::Look,
        run: session_switch,
        state: None,
    },
    Command {
        name: "session-next-waiting",
        doc: "Show the session that has needed input longest, switching to its space.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: session_next_waiting,
        state: None,
    },
    Command {
        name: "session-next-done",
        doc: "Show the session done or ready for review longest, switching to its space.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: session_next_done,
        state: None,
    },
    Command {
        name: "space-switch",
        doc: "Switch to a space, read with completion: a project and its sessions, with its own windows.

The windows of the space left are kept for when it is shown again. A space shown the first time shows its last file and its last session side by side.",
        args: ArgSpec::Listed {
            list: SPACES_LIST,
            prompt: "Space:",
            history: "space",
        },
        reach: Reach::Person,
        run: space_switch,
        state: None,
    },
    Command {
        name: "space-zoom",
        doc: "Show the selected window alone, and the space's windows again.

Back, the window it was zoomed from shows what the zoomed window showed, where it was.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: space_zoom,
        state: None,
    },
    Command {
        name: "session-explain",
        doc: "Explain the last look at the screen of the session whose terminal is this buffer.

`*Session explain*` shows the state the rules gave, the rule that matched, the region's text, the debounce and the registry's status. The agent channel cannot read it.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: session_explain,
        state: None,
    },
];

fn sessions(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    ctx.editor.request_session(SessionRequest::ShowList);
    Ok(())
}

/// The answer a `Listed` prompt read, or an error saying there is none.
fn answer(ctx: &Ctx<'_>, none: &str) -> Result<String, CommandError> {
    super::not_in_prompt(ctx)?;
    match ctx.answer() {
        Some(answer) if !answer.is_empty() => Ok(answer.to_owned()),
        _ => Err(CommandError::new(none.to_owned())),
    }
}

fn session_switch(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let name = answer(ctx, "No session")?;
    ctx.editor.request_session(SessionRequest::Switch(name));
    Ok(())
}

fn session_next_waiting(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    ctx.editor.request_session(SessionRequest::NextWaiting);
    Ok(())
}

fn session_next_done(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    ctx.editor.request_session(SessionRequest::NextDone);
    Ok(())
}

fn space_switch(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let name = answer(ctx, "No space")?;
    ctx.editor.request_session(SessionRequest::Space(name));
    Ok(())
}

fn space_zoom(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    ctx.editor.request_session(SessionRequest::Zoom);
    Ok(())
}

fn session_explain(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let buffer = ctx.editor.window().buffer();
    ctx.editor.request_session(SessionRequest::Explain(buffer));
    Ok(())
}

fn claude(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let read = ctx.directory_and_text();
    let given = |text: &str| (!text.trim().is_empty()).then(|| text.to_owned());
    let directory = read
        .and_then(|(directory, _)| given(directory))
        .map(PathBuf::from);
    let name = read.and_then(|(_, name)| given(name));
    ctx.editor
        .request_session(SessionRequest::Start { name, directory });
    Ok(())
}

fn session_resume(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let buffer = ctx.editor.window().buffer();
    ctx.editor.request_session(SessionRequest::Resume(buffer));
    Ok(())
}

fn session_fork(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let buffer = ctx.editor.window().buffer();
    ctx.editor.request_session(SessionRequest::Fork(buffer));
    Ok(())
}

fn session_explain_file(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let path = ctx.answer().unwrap_or_default();
    if path.is_empty() {
        return Err(CommandError::new("No file to explain"));
    }
    ctx.editor
        .request_session(SessionRequest::ExplainFile(PathBuf::from(path)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use oso_core::command::{Args, Ctx, Invocation};
    use oso_core::editor::{Editor, SessionRequest};

    use super::super::tests::editor;

    fn explain(editor: &mut Editor, answer: &str) -> Result<(), String> {
        let registry = crate::commands::registry();
        let id = registry.lookup("session-explain-file").unwrap();
        let invocation = Invocation {
            answer: Some(answer),
            ..Invocation::default()
        };
        let mut ctx = Ctx::invoked(editor, invocation);
        registry
            .execute(id, &mut ctx, Args::NONE)
            .map_err(|error| error.to_string())
    }

    #[test]
    fn the_path_read_goes_to_the_host() {
        let mut editor = editor("");
        assert_eq!(explain(&mut editor, "/tmp/screen.txt"), Ok(()));
        assert_eq!(
            editor.take_session_requests(),
            [SessionRequest::ExplainFile(PathBuf::from(
                "/tmp/screen.txt"
            ))]
        );
    }

    fn run(editor: &mut Editor, name: &str, answer: Option<&str>, second: Option<&str>) {
        let registry = crate::commands::registry();
        let id = registry.lookup(name).unwrap();
        let invocation = Invocation {
            answer,
            second,
            ..Invocation::default()
        };
        let mut ctx = Ctx::invoked(editor, invocation);
        registry.execute(id, &mut ctx, Args::NONE).unwrap();
    }

    #[test]
    fn claude_asks_for_a_session_with_what_the_prompts_read() {
        let mut editor = editor("");
        run(&mut editor, "claude", None, None);
        run(&mut editor, "claude", Some("/p/oso"), Some("api fix"));
        run(&mut editor, "claude", Some(""), Some(" "));
        assert_eq!(
            editor.take_session_requests(),
            [
                SessionRequest::Start {
                    name: None,
                    directory: None
                },
                SessionRequest::Start {
                    name: Some("api fix".into()),
                    directory: Some(PathBuf::from("/p/oso"))
                },
                SessionRequest::Start {
                    name: None,
                    directory: None
                },
            ]
        );
    }

    #[test]
    fn resume_and_fork_name_the_selected_buffer() {
        let mut editor = editor("");
        let buffer = editor.window().buffer();
        run(&mut editor, "session-resume", None, None);
        run(&mut editor, "session-fork", None, None);
        assert_eq!(
            editor.take_session_requests(),
            [SessionRequest::Resume(buffer), SessionRequest::Fork(buffer)]
        );
    }

    #[test]
    fn the_switches_need_an_answer_and_name_it() {
        let registry = crate::commands::registry();
        let mut editor = editor("");
        for (name, none) in [
            ("session-switch", "No session"),
            ("space-switch", "No space"),
        ] {
            let id = registry.lookup(name).unwrap();
            let invocation = Invocation {
                answer: Some(""),
                ..Invocation::default()
            };
            let mut ctx = Ctx::invoked(&mut editor, invocation);
            let error = registry.execute(id, &mut ctx, Args::NONE).unwrap_err();
            assert!(error.to_string().contains(none), "{error}");
        }
        assert!(editor.take_session_requests().is_empty());
        run(&mut editor, "session-switch", Some("*claude: api*"), None);
        run(&mut editor, "space-switch", Some("oso/fix-1"), None);
        run(&mut editor, "sessions", None, None);
        assert_eq!(
            editor.take_session_requests(),
            [
                SessionRequest::Switch("*claude: api*".into()),
                SessionRequest::Space("oso/fix-1".into()),
                SessionRequest::ShowList,
            ]
        );
    }

    #[test]
    fn no_path_asks_nothing() {
        let mut editor = editor("");
        assert!(explain(&mut editor, "").unwrap_err().contains("No file"));
        assert!(editor.take_session_requests().is_empty());
    }
}
