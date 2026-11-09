//! Running a command for the agent channel (ADR 0085): by name, only if its reach is the one
//! asked for and not `Person`, with its argument checked against its spec, and never with
//! what only a person gives: a key, typed text, the clipboard or a `y` to a question.
//!
//! Must not depend on: the channel's socket or protocol, or `app`; the host decides when the
//! channel may run anything at all.

use oso_core::command::{ArgSpec, BufferPurpose, Invocation, MAX_REMOTE_COUNT, Reach};
use oso_core::editor::Editor;

use super::args::{self, Given};
use super::{Dispatcher, Effects, Step};

/// The most bytes a text argument may come to, times the cursors it goes in at (ADR 0085).
pub const MAX_INSERTED: usize = 1 << 20;

/// Why the channel did not run a command. Its message names only a registered command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub String);

impl Dispatcher {
    /// Every registered command, for the channel to list those it may run.
    pub fn commands(&self) -> impl Iterator<Item = &oso_core::command::Command> {
        self.registry.iter().map(|(_, command)| command)
    }

    /// The command `name`, if the channel may run it with `reach`: it is there, it is no
    /// `Person` command, and `reach` is its own. Asked before anything waits for the person,
    /// so that an agent hears at once what it may never run.
    ///
    /// # Errors
    ///
    /// [`Refused`], naming only a registered command.
    pub fn by_reach(
        &self,
        name: &str,
        reach: Reach,
    ) -> Result<(oso_core::command::CommandId, oso_core::command::Command), Refused> {
        let refused = |why: String| Err(Refused(why));
        let Some(id) = self.registry.lookup(name) else {
            return refused("no command by that name".into());
        };
        let Some(command) = self.registry.get(id).copied() else {
            return refused("no command by that name".into());
        };
        if command.reach == Reach::Person {
            return refused(format!("{name} needs a person at the keyboard"));
        }
        if command.reach != reach {
            let other = if command.reach == Reach::Change {
                "changes"
            } else {
                "only looks"
            };
            return refused(format!("{name} {other}; ask for its reach"));
        }
        Ok((id, command))
    }

    /// Runs the command `name` for the agent channel, if its reach is `reach`, with `given` as
    /// its count, number or text. No prompt opens for an argument: a command without the one
    /// its spec reads is refused. Returns what the host should do, and whether the command succeeded, as
    /// `run-command` does.
    ///
    /// # Errors
    ///
    /// [`Refused`] before anything runs: no such command, a `Person` command, a reach other
    /// than `reach`, an argument its spec does not take, or a yank waiting for the clipboard.
    pub fn run_for_agent(
        &mut self,
        editor: &mut Editor,
        name: &str,
        given: Given<'_>,
        reach: Reach,
    ) -> Result<(Effects, Result<(), String>), Refused> {
        let refused = |why: String| Err(Refused(why));
        let (id, command) = self.by_reach(name, reach)?;
        if self.waits_for_clipboard() {
            return refused("a yank is waiting for the clipboard".into());
        }
        // A count repeats the command's work; a number, such as a line, is only a value.
        let repeats = !matches!(command.args, oso_core::command::ArgSpec::Number { .. });
        if let Given::Count(n) = given
            && repeats
            && n.unsigned_abs() > MAX_REMOTE_COUNT.unsigned_abs()
        {
            return refused(format!("{name}'s argument is beyond ±{MAX_REMOTE_COUNT}"));
        }
        // Text goes in at every cursor, so its size times theirs is what it costs.
        let cursors = editor.window().selections.len();
        if let Given::Text(text) = given
            && text.len().saturating_mul(cursors) > MAX_INSERTED
        {
            return refused(format!(
                "the text times the cursors is more than {MAX_INSERTED} bytes"
            ));
        }
        if let Some(why) = shows_what_it_may(editor, command.args, given) {
            return refused(why.into());
        }
        // Whatever the person's keys left stays theirs: the agent's command never takes it.
        let mapped = args::map(name, command.args, given, None).map_err(Refused)?;
        let invocation = Invocation {
            answer: mapped.answer.as_deref(),
            ..Invocation::default()
        };
        let args = mapped.args;
        // Its kill joins no kill of the person's, before or after it, so that what reaches the
        // OS clipboard from a person's kill never holds the agent's text (ADR 0085).
        editor.forget_kills();
        // The prompts it opens are the agent's, and say so (ADR 0085).
        let ran = self.as_input(editor, |this, editor| {
            editor.for_agent(|editor| {
                let (effects, result) = this.execute(editor, id, args, invocation, Step::Apart);
                let effects = effects.union(this.after_prompts(editor));
                (effects, result.map_err(|e| e.to_string()))
            })
        });
        editor.forget_kills();
        Ok(ran)
    }
}

/// Why the buffer the agent names to show may not be shown, if it may not: it must be one
/// `C-x b` lists, as the channel makes no buffer, and not one that may hold secrets
/// (ADR 0094). The reason never quotes the name, which is the agent's text.
fn shows_what_it_may(editor: &Editor, spec: ArgSpec, given: Given<'_>) -> Option<&'static str> {
    let (
        ArgSpec::Buffer {
            purpose: BufferPurpose::Switch,
            ..
        },
        Given::Text(name),
    ) = (spec, given)
    else {
        return None;
    };
    match editor.buffer_named(name).and_then(|id| editor.buffer(id)) {
        None => Some("no buffer by that name; the channel makes none"),
        Some(buffer) if buffer.is_secret() => {
            Some("that buffer may hold secrets, so it is not shown; ask the person")
        }
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Given, Refused};
    use crate::dispatch::Dispatcher;
    use oso_core::buffer::Buffer;
    use oso_core::command::MAX_REMOTE_COUNT;
    use oso_core::command::Reach;
    use oso_core::editor::Editor;
    use oso_core::window::ViewSize;

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.md", text));
        editor.set_view_size(ViewSize::new(10, 40));
        editor
    }

    fn cursor(editor: &Editor) -> usize {
        editor.window().selections.primary().cursor
    }

    fn refusal(
        dispatcher: &mut Dispatcher,
        editor: &mut Editor,
        name: &str,
        given: Given<'_>,
        reach: Reach,
    ) -> String {
        match dispatcher.run_for_agent(editor, name, given, reach) {
            Err(Refused(why)) => why,
            Ok(outcome) => panic!("{name} ran: {outcome:?}"),
        }
    }

    #[test]
    fn runs_a_look_command_with_a_count() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("abcdef"));
        let (_, result) = dispatcher
            .run_for_agent(&mut editor, "forward-char", Given::Count(3), Reach::Look)
            .unwrap();
        assert_eq!(result, Ok(()));
        assert_eq!(cursor(&editor), 3);
        let (_, result) = dispatcher
            .run_for_agent(&mut editor, "goto-line", Given::Count(1), Reach::Look)
            .unwrap();
        assert_eq!(result, Ok(()));
        assert!(editor.prompt().is_none());
    }

    #[test]
    fn switches_by_name_only_to_a_listed_buffer_that_may_be_shown() {
        use std::path::PathBuf;
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("abc"));
        let todo = editor.add_buffer(Buffer::from_text("todo.md", "x"));
        let env = PathBuf::from("/tmp/.env");
        let mut secret = Buffer::visiting(env.clone());
        secret.judge_secret(Some(&[env]));
        editor.add_buffer(secret);
        // Not judged yet: it may hold secrets until it is.
        editor.add_buffer(Buffer::visiting(PathBuf::from("/tmp/later.txt")));
        let buffers = editor.buffers().count();
        let mut switch = |editor: &mut Editor, name| {
            dispatcher.run_for_agent(editor, "switch-to-buffer", Given::Text(name), Reach::Look)
        };
        let (_, result) = switch(&mut editor, "todo.md").unwrap();
        assert_eq!(result, Ok(()));
        assert_eq!(editor.window().buffer(), todo);
        let (_, result) = switch(&mut editor, "*Messages*").unwrap();
        assert_eq!(result, Ok(()), "an internal buffer C-x b lists");
        let messages = editor.window().buffer();
        for (name, why) in [
            ("missing.md", "no buffer by that name"),
            (".env", "may hold secrets"),
            ("later.txt", "may hold secrets"),
        ] {
            let Err(Refused(refused)) = switch(&mut editor, name) else {
                panic!("{name} was shown");
            };
            assert!(refused.contains(why), "{name}: {refused}");
            assert!(
                !refused.contains(name),
                "the agent's text is not quoted: {refused}"
            );
        }
        assert_eq!(editor.window().buffer(), messages, "nothing else showed");
        assert_eq!(
            editor.buffers().count(),
            buffers,
            "the channel made no buffer"
        );
    }

    #[test]
    fn refuses_what_needs_a_person_or_another_reach() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("abc"));
        let mut refused =
            |name, arg, reach| refusal(&mut dispatcher, &mut editor, name, arg, reach);
        assert_eq!(
            refused("no-such", Given::Nothing, Reach::Look),
            "no command by that name"
        );
        assert!(refused("yank", Given::Nothing, Reach::Change).contains("needs a person"));
        assert!(
            refused("save-buffers-kill-terminal", Given::Nothing, Reach::Change)
                .contains("needs a person")
        );
        assert!(refused("answer-yes", Given::Nothing, Reach::Look).contains("needs a person"));
        assert!(refused("delete-char", Given::Nothing, Reach::Look).contains("changes"));
        assert!(refused("forward-char", Given::Nothing, Reach::Change).contains("only looks"));
        assert!(refused("goto-line", Given::Nothing, Reach::Look).contains("needs a number"));
        // Its name comes with it as text; no prompt opens for it.
        assert!(refused("switch-to-buffer", Given::Nothing, Reach::Look).contains("buffer's name"));
        assert!(
            refused("mark-whole-buffer", Given::Count(1), Reach::Look)
                .contains("takes no argument")
        );
        let beyond = Given::Count(MAX_REMOTE_COUNT + 1);
        assert!(refused("forward-char", beyond, Reach::Look).contains("beyond"));
        assert!(refused("forward-char", Given::Count(isize::MIN), Reach::Look).contains("beyond"));
        assert_eq!(editor.current().1.text().to_string(), "abc");
    }

    #[test]
    fn a_refusal_names_no_unregistered_command() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor(""));
        let why = refusal(
            &mut dispatcher,
            &mut editor,
            "\u{202e}evil",
            Given::Nothing,
            Reach::Look,
        );
        assert!(!why.contains("evil"), "{why}");
    }

    #[test]
    fn an_agents_kill_joins_no_kill_of_the_persons() {
        use oso_core::key::KeySequence;
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("aaa bbb ccc ddd"));
        let keys = |text: &str| text.parse::<KeySequence>().unwrap().0;
        dispatcher.handle_keys(&mut editor, keys("M-d"));
        let first = editor.take_clipboard_write().unwrap();
        assert_eq!(first.text(), "aaa");
        let (_, result) = dispatcher
            .run_for_agent(&mut editor, "kill-word", Given::Nothing, Reach::Change)
            .unwrap();
        assert_eq!(result, Ok(()));
        drop(editor.take_clipboard_write());
        dispatcher.handle_keys(&mut editor, keys("M-d"));
        let after = editor.take_clipboard_write().unwrap();
        assert_eq!(
            after.text(),
            " ccc",
            "the person's kill is an entry of its own"
        );
    }

    #[test]
    fn a_prefix_argument_that_reads_no_more_digits_is_still_in_progress() {
        use oso_core::key::KeySequence;
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("abc"));
        let keys = "C-u 5 C-u".parse::<KeySequence>().unwrap().0;
        dispatcher.handle_keys(&mut editor, keys);
        assert!(!dispatcher.reads_keys());
        assert!(
            dispatcher.keys_in_progress(),
            "the next command takes the argument"
        );
    }

    #[test]
    fn insert_takes_its_text_with_the_command_and_nothing_else() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("ab"));
        let (_, result) = dispatcher
            .run_for_agent(&mut editor, "forward-char", Given::Count(1), Reach::Look)
            .unwrap();
        assert_eq!(result, Ok(()));
        let (_, result) = dispatcher
            .run_for_agent(&mut editor, "insert", Given::Text("XY"), Reach::Change)
            .unwrap();
        assert_eq!(result, Ok(()));
        assert_eq!(editor.current().1.text().to_string(), "aXYb");
        assert!(editor.prompt().is_none(), "no prompt opened");
        let mut refused =
            |given| refusal(&mut dispatcher, &mut editor, "insert", given, Reach::Change);
        assert!(refused(Given::Nothing).contains("needs text"));
        assert!(refused(Given::Count(3)).contains("needs text"));
        editor.undo().unwrap();
        assert_eq!(editor.current().1.text().to_string(), "ab");
    }

    /// Asks once, and again when its question is answered `y`.
    fn ask_again(
        ctx: &mut oso_core::command::Ctx<'_>,
        _: oso_core::command::Args,
    ) -> Result<(), oso_core::command::CommandError> {
        let question = if ctx.confirmed() { "Again?" } else { "First?" };
        ctx.editor
            .ask(question, "ask-again")
            .map_err(|e| oso_core::command::CommandError::new(e.to_string()))?;
        Ok(())
    }

    #[test]
    fn what_the_persons_yes_to_the_agents_question_opens_is_the_agents() {
        use oso_core::command::{ArgSpec, Command, Registry};
        let mut registry = Registry::new();
        registry
            .register(Command {
                name: "ask-again",
                doc: "Ask, and ask again.",
                args: ArgSpec::None,
                reach: Reach::Look,
                run: ask_again,
            })
            .unwrap();
        let mut dispatcher = Dispatcher::with(registry, oso_core::keymap::Keymap::default());
        let mut editor = editor("");
        let (_, result) = dispatcher
            .run_for_agent(&mut editor, "ask-again", Given::Nothing, Reach::Look)
            .unwrap();
        assert_eq!(result, Ok(()));
        assert!(
            editor
                .prompt()
                .unwrap()
                .label()
                .starts_with("Agent: First?")
        );
        // The person answers, outside anything the agent runs.
        editor.answer_question(true).unwrap();
        dispatcher.after_prompts(&mut editor);
        let again = editor.prompt().unwrap();
        assert!(again.from_agent(), "{}", again.label());
        assert!(again.label().starts_with("Agent: Again?"));
        // A person's own question stays the person's.
        editor.answer_question(false).unwrap();
        editor.ask("Mine?", "ask-again").unwrap();
        editor.answer_question(true).unwrap();
        dispatcher.after_prompts(&mut editor);
        assert!(!editor.prompt().unwrap().from_agent());
    }

    #[test]
    fn text_times_the_cursors_is_bounded() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("a\nb\nc"));
        let text = "x".repeat(super::MAX_INSERTED / 2 + 1);
        let (_, result) = dispatcher
            .run_for_agent(&mut editor, "insert", Given::Text(&text), Reach::Change)
            .unwrap();
        assert_eq!(result, Ok(()), "one cursor");
        editor.undo().unwrap();
        let two = oso_core::selection::SelectionSet::from_parts(
            vec![
                oso_core::selection::Selection::at(0),
                oso_core::selection::Selection::at(2),
            ],
            0,
        )
        .unwrap();
        editor.current_mut().0.selections = two;
        let why = refusal(
            &mut dispatcher,
            &mut editor,
            "insert",
            Given::Text(&text),
            Reach::Change,
        );
        assert!(why.contains("times the cursors"), "{why}");
    }

    #[test]
    fn a_change_runs_as_an_undo_step_of_its_own() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("abc"));
        for _ in 0..2 {
            let (_, result) = dispatcher
                .run_for_agent(&mut editor, "delete-char", Given::Nothing, Reach::Change)
                .unwrap();
            assert_eq!(result, Ok(()));
        }
        assert_eq!(editor.current().1.text().to_string(), "c");
        editor.undo().unwrap();
        assert_eq!(editor.current().1.text().to_string(), "bc");
    }
}
