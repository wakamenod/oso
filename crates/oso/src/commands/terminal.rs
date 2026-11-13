//! Commands for terminals (ADR 0120): starting or switching to one, and oso's keys inside
//! one, which send the program what the key itself would not reach it as. Keys that are not
//! oso's go to the program without any command (`terminal::route`).
//!
//! Must not depend on: winit, the GPU, `bindings` or the host's terminals; a command asks the
//! host through `Editor::request_terminal`, and the host acts on the terminal after it.
//!
//! Each is `Person`: it starts a process, or types into a program, which only a person does
//! (ADR 0120, Q15), or gives or takes back an agent's leave to read a terminal (Q16).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::TerminalRequest;
use oso_core::key::{Key, KeyPress, Modifiers};
use oso_core::kill::FromClipboard;

/// The terminal commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "terminal",
        doc: "Switch to the last terminal still running, or start one.\n\nWith a prefix argument, always start a new one. It starts `[terminal] shell` from `config.toml`, else `$SHELL`, else the account's shell, else `/bin/sh`, as a login shell unless `[terminal] login` is false, in the project's root, else the buffer's directory, else the home directory. It opens in a restricted project too: what runs in it is the person's.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: terminal,
        state: None,
    },
    Command {
        name: "terminal-send-interrupt",
        doc: "Send `C-c` to the program in the terminal, as `C-c C-c` does.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: send_interrupt,
        state: None,
    },
    Command {
        name: "terminal-char-mode",
        doc: "Switch the terminal between semi-char and char mode.\n\nIn semi-char mode, `C-x`, `M-x`, the `C-c` prefix, `C-q` and `C-y` stay oso's and every other key goes to the program. In char mode every key goes to the program but `C-c C-k`, which comes back.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: char_mode,
        state: None,
    },
    Command {
        name: "terminal-send-next-key",
        doc: "Send the next key to the program in the terminal as it is, one of oso's own among them.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: send_next_key,
        state: None,
    },
    Command {
        name: "terminal-yank",
        doc: "Paste the newest kill, or what another app put on the clipboard, into the program in the terminal.\n\nWith N, paste the Nth newest kill. With bracketed paste on, control characters but tab and new line are removed first. With it off, text of several lines asks first, as a program may run them.",
        args: ArgSpec::CountAndClipboard,
        reach: Reach::Person,
        run: terminal_yank,
        state: None,
    },
    Command {
        name: "terminal-paste-lines",
        doc: "Paste the lines a question asked about into the program in the terminal.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: paste_lines,
        state: None,
    },
    Command {
        name: "terminal-allow-agent-read",
        doc: "Let the agent channel read this terminal's rows until its buffer is killed.\n\nThe modeline says `agent reads`. While the program reads a password, its rows are withheld even so. An agent never types into a terminal.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: allow_agent_read,
        state: None,
    },
    Command {
        name: "terminal-deny-agent-read",
        doc: "Take back the agent channel's leave to read this terminal's rows.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: deny_agent_read,
        state: None,
    },
];

fn terminal(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let new = args.raw().is_some();
    ctx.editor.request_terminal(TerminalRequest::Open { new });
    ctx.request_redraw();
    Ok(())
}

/// Refuses a command outside a terminal's buffer.
fn in_terminal(ctx: &Ctx<'_>) -> Result<(), CommandError> {
    if ctx.editor.current().1.is_terminal() {
        Ok(())
    } else {
        Err(CommandError::new("Not in a terminal"))
    }
}

fn send_interrupt(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    in_terminal(ctx)?;
    let c_c = KeyPress {
        modifiers: Modifiers::CONTROL,
        key: Key::Char('c'),
    };
    ctx.editor
        .request_terminal(TerminalRequest::Keys(vec![c_c]));
    Ok(())
}

fn char_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    in_terminal(ctx)?;
    ctx.editor.request_terminal(TerminalRequest::ToggleCharMode);
    ctx.request_redraw();
    Ok(())
}

fn allow_agent_read(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_agent_reads(ctx, true)
}

fn deny_agent_read(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_agent_reads(ctx, false)
}

/// Gives or takes back the agent's leave to read the selected terminal, and says so.
fn set_agent_reads(ctx: &mut Ctx<'_>, reads: bool) -> Result<(), CommandError> {
    in_terminal(ctx)?;
    let (window, buffer) = ctx.editor.current();
    let (id, was) = (window.buffer(), buffer.agent_reads());
    if was != reads {
        ctx.editor.set_agent_reads(id, reads);
        ctx.request_redraw();
    }
    ctx.editor.message(if reads {
        "Agents may read this terminal"
    } else {
        "Agents may not read this terminal"
    });
    Ok(())
}

fn send_next_key(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    in_terminal(ctx)?;
    ctx.editor.request_terminal(TerminalRequest::QuoteNext);
    Ok(())
}

fn terminal_yank(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    in_terminal(ctx)?;
    let unread = match ctx.clipboard() {
        Some(FromClipboard::Text {
            text,
            concealed,
            fingerprint,
        }) => {
            ctx.editor
                .take_from_clipboard(text.clone(), *concealed, *fingerprint);
            None
        }
        Some(FromClipboard::TooLarge) => Some("Clipboard text too large; pasted the newest kill"),
        Some(FromClipboard::NoAnswer) => Some("Clipboard not read; pasted the newest kill"),
        Some(FromClipboard::Own | FromClipboard::Nothing) | None => None,
    };
    let ring = ctx.editor.kill_ring();
    let len = ring.len().max(1);
    let n = match args.raw() {
        None | Some(oso_core::command::Prefix::Universal(_)) => 0,
        Some(prefix) => prefix
            .value()
            .saturating_sub(1)
            .rem_euclid(len.cast_signed())
            .unsigned_abs(),
    };
    let text = ring
        .get(n)
        .map(oso_core::kill::KillEntry::joined)
        .ok_or_else(|| CommandError::new("Kill ring is empty"))?;
    ctx.editor.request_terminal(TerminalRequest::Paste { text });
    if let Some(unread) = unread {
        ctx.editor.echo(unread);
    }
    Ok(())
}

fn paste_lines(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    in_terminal(ctx)?;
    ctx.editor.request_terminal(TerminalRequest::PasteLines);
    Ok(())
}

#[cfg(test)]
mod tests {
    use oso_core::command::{Args, Ctx, Invocation, Prefix};
    use oso_core::editor::{Editor, TerminalRequest};
    use oso_core::key::{Key, KeyPress, Modifiers};
    use oso_core::kill::Piece;
    use oso_core::rope::Rope;

    fn run(editor: &mut Editor, name: &str, args: Args) -> Result<(), String> {
        let registry = crate::commands::registry();
        let id = registry.lookup(name).unwrap();
        let mut ctx = Ctx::invoked(editor, Invocation::default());
        registry
            .execute(id, &mut ctx, args)
            .map_err(|error| error.to_string())
    }

    fn in_terminal() -> Editor {
        let mut editor = Editor::default();
        let terminal = editor.add_terminal_buffer("*terminal*");
        editor.show(terminal);
        editor
    }

    #[test]
    fn terminal_asks_for_the_last_or_with_a_prefix_a_new_one() {
        let mut editor = Editor::default();
        run(&mut editor, "terminal", Args::NONE).unwrap();
        run(
            &mut editor,
            "terminal",
            Args::with_prefix(Some(Prefix::Universal(4))),
        )
        .unwrap();
        assert_eq!(
            editor.take_terminal_requests(),
            [
                TerminalRequest::Open { new: false },
                TerminalRequest::Open { new: true }
            ]
        );
    }

    #[test]
    fn the_keys_inside_ask_only_in_a_terminal() {
        let mut editor = Editor::default();
        for name in [
            "terminal-send-interrupt",
            "terminal-char-mode",
            "terminal-send-next-key",
            "terminal-paste-lines",
        ] {
            assert_eq!(
                run(&mut editor, name, Args::NONE),
                Err(format!("{name}: Not in a terminal"))
            );
        }
        assert!(editor.take_terminal_requests().is_empty());
        let mut editor = in_terminal();
        run(&mut editor, "terminal-send-interrupt", Args::NONE).unwrap();
        run(&mut editor, "terminal-char-mode", Args::NONE).unwrap();
        run(&mut editor, "terminal-send-next-key", Args::NONE).unwrap();
        run(&mut editor, "terminal-paste-lines", Args::NONE).unwrap();
        let c_c = KeyPress {
            modifiers: Modifiers::CONTROL,
            key: Key::Char('c'),
        };
        assert_eq!(
            editor.take_terminal_requests(),
            [
                TerminalRequest::Keys(vec![c_c]),
                TerminalRequest::ToggleCharMode,
                TerminalRequest::QuoteNext,
                TerminalRequest::PasteLines,
            ]
        );
    }

    #[test]
    fn the_yank_pastes_the_newest_kill_or_the_nth() {
        let mut editor = in_terminal();
        assert_eq!(
            run(&mut editor, "terminal-yank", Args::NONE),
            Err("terminal-yank: Kill ring is empty".into())
        );
        let piece = |text: &str| vec![Piece::from(Rope::from(text))];
        editor.carry_kills(vec![piece("second"), piece("first")]);
        run(&mut editor, "terminal-yank", Args::NONE).unwrap();
        run(
            &mut editor,
            "terminal-yank",
            Args::with_prefix(Some(Prefix::Number(2))),
        )
        .unwrap();
        assert_eq!(
            editor.take_terminal_requests(),
            [
                TerminalRequest::Paste {
                    text: "second".into()
                },
                TerminalRequest::Paste {
                    text: "first".into()
                },
            ]
        );
    }

    #[test]
    fn leave_to_read_is_given_and_taken_back_only_in_a_terminal() {
        let mut editor = Editor::default();
        assert!(run(&mut editor, "terminal-allow-agent-read", Args::NONE).is_err());
        let mut editor = in_terminal();
        let id = editor.current().0.buffer();
        run(&mut editor, "terminal-allow-agent-read", Args::NONE).unwrap();
        assert!(editor.buffer(id).unwrap().agent_reads());
        run(&mut editor, "terminal-deny-agent-read", Args::NONE).unwrap();
        assert!(!editor.buffer(id).unwrap().agent_reads());
    }

    #[test]
    fn every_terminal_command_is_the_persons() {
        let registry = crate::commands::registry();
        for command in super::COMMANDS {
            let id = registry.lookup(command.name).unwrap();
            assert_eq!(
                registry.get(id).unwrap().reach,
                oso_core::command::Reach::Person,
                "{}",
                command.name
            );
        }
    }
}
