//! The editor's commands (ADR 0034). Each is a plain function over the
//! host-owned state in `Ctx`, registered with a docstring, an argument spec
//! and a reach (ADR 0085). Each feature has a module here with its own table
//! of commands, and [`TABLES`] lists them. Default key bindings are in
//! `bindings`.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

mod agent;
#[cfg(feature = "dev")]
mod dev_tools;
mod edit;
mod kill;
mod mark;
mod messages;
mod minibuffer;
mod motion;
mod prefix;
mod recovery;
mod save;
mod scroll;
mod session;

#[cfg(test)]
pub(crate) use messages::kept_messages;
use oso_core::command::{Command, CommandError, Ctx, Registry};
use oso_core::motion::{TOO_MUCH, WalkBudget};

/// Every feature's commands. A new feature adds a module and one line here, in
/// alphabetical order and never joined with another, so that features added at
/// the same time rarely touch the same line. This is only the order they
/// register in: a list shown to the user decides its own order.
#[rustfmt::skip]
const TABLES: &[&[Command]] = &[
    agent::COMMANDS,
    #[cfg(feature = "dev")]
    dev_tools::COMMANDS,
    edit::COMMANDS,
    kill::COMMANDS,
    mark::COMMANDS,
    messages::COMMANDS,
    minibuffer::COMMANDS,
    motion::COMMANDS,
    prefix::COMMANDS,
    recovery::COMMANDS,
    save::COMMANDS,
    scroll::COMMANDS,
    session::COMMANDS,
];

/// Refuses a command that would change which buffer a window shows, while a prompt is
/// selected: its window always shows its own (ADR 0072).
fn not_in_prompt(ctx: &Ctx<'_>) -> Result<(), CommandError> {
    match ctx.editor.prompt() {
        Some(_) => Err(CommandError::new("Not in the minibuffer")),
        None => Ok(()),
    }
}

/// The signal for a command whose budget ran out, after it did what it could.
fn budget_left(budget: WalkBudget) -> Result<(), CommandError> {
    if budget.is_spent() {
        return Err(CommandError::new(TOO_MUCH));
    }
    Ok(())
}

/// A registry with every command in [`TABLES`].
///
/// # Panics
///
/// If a command is registered twice or lacks a docstring. The tests build
/// this registry, so such a mistake cannot ship.
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    for table in TABLES {
        if let Err(e) = registry.register_all(table) {
            panic!("{e}");
        }
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::{TABLES, registry};
    use oso_core::buffer::Buffer;
    use oso_core::command::{ArgSpec, Args, Ctx, Effects, ExecuteError, Reach};
    use oso_core::editor::Editor;
    use oso_core::window::ViewSize;

    pub(super) fn editor(text: &str) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("test", text));
        editor.set_view_size(ViewSize::new(10, 40));
        editor
    }

    pub(super) fn run(
        editor: &mut Editor,
        name: &str,
        args: Args,
    ) -> (Result<(), String>, Effects) {
        let registry = registry();
        let id = registry.lookup(name).unwrap();
        let mut ctx = Ctx::new(editor);
        let result = registry.execute(id, &mut ctx, args).map_err(|e| match e {
            ExecuteError::Failed { error, .. } => error.message().to_owned(),
            other => panic!("{other}"),
        });
        (result, ctx.effects())
    }

    pub(super) fn cursor(editor: &Editor) -> usize {
        editor.window().selections.primary().cursor
    }

    #[cfg(not(feature = "dev"))]
    #[test]
    fn a_build_without_the_dev_tools_has_no_dev_commands() {
        assert_eq!(registry().lookup("dev-panic"), None);
    }

    #[test]
    fn every_command_registers() {
        let commands: usize = TABLES.iter().map(|table| table.len()).sum();
        assert_eq!(registry().iter().len(), commands);
    }

    /// The commands the agent channel refuses (ADR 0085). A change here is a change to what an
    /// agent may do in a person's editor, so it shows in review.
    #[test]
    fn the_agent_channel_refuses_what_needs_a_person() {
        let mut person: Vec<&str> = registry()
            .iter()
            .filter(|(_, command)| command.reach == Reach::Person)
            .map(|(_, command)| command.name)
            .collect();
        person.sort_unstable();
        let mut expected = vec![
            // Who may drive the editor.
            "agent-channel-allow",
            "agent-channel-start",
            "agent-channel-stop",
            // Answering a prompt.
            "answer-no",
            "answer-yes",
            "exit-minibuffer",
            "next-history-element",
            "previous-history-element",
            // Emergency copies.
            "emergency-copies-back",
            "emergency-copies-refresh",
            "emergency-copy-compare",
            "emergency-copy-delete",
            "emergency-copy-recover",
            "emergency-copy-view",
            "recover-session",
            // Exiting, and leaving what the person is doing.
            "keyboard-quit",
            "save-buffers-kill-terminal",
            // The person's yes to the agent's question about saving.
            "save-buffer-the-agent-opened",
            // Keys: the channel passes a count or text instead.
            "digit-argument",
            "negative-argument",
            "self-insert-command",
            "universal-argument",
            // The OS clipboard.
            "yank",
        ];
        #[cfg(feature = "dev")]
        expected.extend(["dev-panic", "dev-read-passphrase"]);
        expected.sort_unstable();
        assert_eq!(person, expected);
    }

    /// A `Look` command changes no buffer text, with a region and with a count either way
    /// (ADR 0085). What it may do to the kill ring and the clipboard, the channel decides.
    #[test]
    fn look_commands_change_no_text() {
        let registry = registry();
        let looks = registry
            .iter()
            .filter(|(_, command)| command.reach == Reach::Look);
        for (id, command) in looks {
            for args in [Args::NONE, Args::numeric(3), Args::numeric(-3)] {
                let mut editor = editor("one two three\nfour five\n  six\n");
                assert_eq!(run(&mut editor, "set-mark-command", Args::NONE).0, Ok(()));
                assert_eq!(run(&mut editor, "forward-word", Args::numeric(2)).0, Ok(()));
                let before = editor.current().1.content_id();
                let mut ctx = Ctx::new(&mut editor);
                // Failing is fine here: a motion at the end, or a number with no answer.
                let _ = registry.execute(id, &mut ctx, args);
                let buffer = editor.current().1;
                assert_eq!(buffer.content_id(), before, "{} {args:?}", command.name);
                assert!(!buffer.is_modified(), "{} {args:?}", command.name);
            }
        }
    }

    /// What a command's argument or name says it needs settles its reach, whatever the list
    /// above holds (ADR 0085).
    #[test]
    fn passphrases_the_clipboard_and_dev_commands_need_a_person() {
        for (_, command) in registry().iter() {
            let needs_a_person = matches!(
                command.args,
                ArgSpec::Passphrase { .. } | ArgSpec::CountAndClipboard
            ) || command.name.starts_with("dev-");
            if needs_a_person {
                assert_eq!(command.reach, Reach::Person, "{}", command.name);
            }
        }
    }
}
