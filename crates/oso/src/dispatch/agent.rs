//! Running a command for the agent channel (ADR 0085): by name, only if its reach is the one
//! asked for and not `Person`, with its argument checked against its spec, and never with
//! what only a person gives: a key, typed text, the clipboard or a `y` to a question.
//!
//! Must not depend on: the channel's socket or protocol, or `app`; the host decides when the
//! channel may run anything at all.

use oso_core::buffer::{Buffer, BufferId, Mode};
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
        // An agent's command is no key of the person's, so it ends no flash (ADR 0105).
        let before = self.view_jumped.take();
        let ran = self.run_agent_command(editor, name, given, reach);
        self.view_jumped = if self.view_jumped == Some(true) {
            Some(true)
        } else {
            before
        };
        ran
    }

    /// [`Dispatcher::run_for_agent`], but for the cursor flash.
    fn run_agent_command(
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
        // A buffer to show, by its name: resolved once, here, and shown as it is, so that
        // nothing makes a buffer for the agent (ADR 0094, ADR 0095).
        if let (
            ArgSpec::Buffer {
                purpose: BufferPurpose::Switch,
                ..
            },
            Given::Text(name),
        ) = (command.args, given)
        {
            let shown = showable(editor, name).map_err(|why| Refused(why.into()))?;
            return Ok(self.as_input(editor, |_, editor| {
                editor.for_agent(|editor| editor.show(shown));
                let effects = Effects {
                    redraw: true,
                    ..Effects::default()
                };
                (effects, Ok(()))
            }));
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
        // The prompts it opens are the agent's, and say so (ADR 0085). What it queues to save
        // its caller starts as the agent's (ADR 0108).
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

/// The buffer named `name`, if the agent may show it: one `C-x b` lists, as the channel
/// makes no buffer (ADR 0094); not one that may hold secrets or is still loading; and one
/// whose keys are the ordinary ones, so that the person's typing never reaches the bare keys
/// of a list such as `*Emergency Copies*` (ADR 0095). The reason never quotes the name, which
/// is the agent's text.
fn showable(editor: &Editor, name: &str) -> Result<BufferId, &'static str> {
    let Some((id, buffer)) = editor
        .buffer_named(name)
        .and_then(|id| Some((id, editor.buffer(id)?)))
    else {
        return Err("no buffer by that name; the channel makes none");
    };
    match unshowable(buffer) {
        None => Ok(id),
        Some(Unshowable::Secret) => {
            Err("that buffer may hold secrets, so it is not shown; ask the person")
        }
        Some(Unshowable::Opening) => Err("that buffer is still opening; try again"),
        Some(Unshowable::Loading) => Err("that buffer is still loading; try again"),
        Some(Unshowable::OwnKeys) => Err("that buffer's keys are the person's; ask the person"),
        Some(Unshowable::Withheld) => Err("only the person reads that buffer; ask the person"),
    }
}

/// Why the agent channel may not show a buffer that exists, whether by name or for an `open` of
/// a file it already holds (ADR 0094, ADR 0095).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unshowable {
    /// It may hold secrets, or is not judged yet.
    Secret,
    /// The channel is still opening it: it is shown only once its open checked it.
    Opening,
    /// It is still loading.
    Loading,
    /// It has keys of its own, which the person's typing would then reach.
    OwnKeys,
    /// It is withheld from the agent channel, though it holds no secret, such as
    /// `*Session explain*` (ADR 0121).
    Withheld,
}

/// Why the channel may not show `buffer`, if it may not. One check for every way it shows one.
pub fn unshowable(buffer: &Buffer) -> Option<Unshowable> {
    if buffer.is_secret() {
        Some(Unshowable::Secret)
    } else if buffer.withheld_from_agents() {
        Some(Unshowable::Withheld)
    } else if buffer.agent_pending() {
        Some(Unshowable::Opening)
    } else if buffer.loading().is_some() {
        Some(Unshowable::Loading)
    } else if has_own_keys(buffer.mode()) {
        Some(Unshowable::OwnKeys)
    } else {
        None
    }
}

/// Whether a buffer in `mode` takes keys of its own, which the person's typing would reach. A
/// match with no wildcard, so that a new mode says.
fn has_own_keys(mode: Mode) -> bool {
    match mode {
        // A terminal's keys go to its program (ADR 0120).
        Mode::CopyList | Mode::CopyView | Mode::Terminal => true,
        Mode::Fundamental | Mode::Rust | Mode::Markdown | Mode::Toml => false,
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

    /// Runs `switch-to-buffer` for the agent with `name`, and returns why it was refused.
    fn refused_switch(dispatcher: &mut Dispatcher, editor: &mut Editor, name: &str) -> String {
        refusal(
            dispatcher,
            editor,
            "switch-to-buffer",
            Given::Text(name),
            Reach::Look,
        )
    }

    /// A loaded buffer visiting `path`, judged by the names it resolved to.
    fn visited(path: &str, resolved: &[&str]) -> Buffer {
        use oso_core::display_map::TabWidth;
        use oso_core::load::decode_all;
        let mut buffer = Buffer::visiting(std::path::PathBuf::from(path));
        let resolved: Vec<_> = resolved.iter().map(std::path::PathBuf::from).collect();
        buffer.judge_secret(Some(&resolved));
        buffer.apply_load(decode_all(b"x\n", TabWidth::default()));
        buffer
    }

    #[test]
    fn a_name_c_x_b_does_not_list_is_no_buffer_even_when_ordinary() {
        use oso_core::buffer::Mode;
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("abc"));
        let view = editor.add_internal_buffer("*copy of notes.txt*", Mode::CopyView);
        assert!(!editor.buffer(view).unwrap().is_secret(), "judged ordinary");
        let why = refused_switch(&mut dispatcher, &mut editor, "*copy of notes.txt*");
        assert!(why.contains("no buffer by that name"), "{why}");
        assert_ne!(editor.window().buffer(), view);
    }

    #[test]
    fn only_the_modes_of_emergency_copies_have_keys_of_their_own() {
        use super::{Unshowable, unshowable};
        use oso_core::buffer::Mode;
        let mut editor = editor("abc");
        let rust = editor.add_internal_buffer("main.rs", Mode::Rust);
        let view = editor.add_internal_buffer("*copy of notes.txt*", Mode::CopyView);
        assert!(unshowable(editor.buffer(rust).unwrap()).is_none());
        assert!(matches!(
            unshowable(editor.buffer(view).unwrap()),
            Some(Unshowable::OwnKeys)
        ));
    }

    #[test]
    fn the_list_of_emergency_copies_is_the_persons() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("abc"));
        let notes = editor.window().buffer();
        editor.toggle_copies();
        editor.toggle_copies();
        assert!(editor.has_copies_list());
        assert_eq!(editor.window().buffer(), notes);
        let why = refused_switch(&mut dispatcher, &mut editor, "*Emergency Copies*");
        assert!(why.contains("the person's"), "{why}");
        assert_eq!(editor.window().buffer(), notes);
    }

    #[test]
    fn of_two_files_of_one_name_only_the_ordinary_one_shows() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("abc"));
        let work = editor.add_buffer(visited("/work/notes.md", &["/work/notes.md"]));
        // A name a link gave it: the file may hold secrets.
        let home = editor.add_buffer(visited("/home/notes.md", &["/home/.env"]));
        assert!(editor.buffer(home).unwrap().is_secret());
        let name = |editor: &Editor, id| editor.buffer(id).unwrap().name().to_owned();
        let (work_name, home_name) = (name(&editor, work), name(&editor, home));
        assert_ne!(work_name, home_name, "told apart");
        let (_, result) = dispatcher
            .run_for_agent(
                &mut editor,
                "switch-to-buffer",
                Given::Text(&work_name),
                Reach::Look,
            )
            .unwrap();
        assert_eq!(result, Ok(()));
        assert_eq!(editor.window().buffer(), work, "exactly that one");
        let why = refused_switch(&mut dispatcher, &mut editor, &home_name);
        assert!(why.contains("may hold secrets"), "{why}");
        assert_eq!(editor.window().buffer(), work);
    }

    #[test]
    fn a_buffer_still_loading_is_not_shown() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("abc"));
        let mut loading = Buffer::visiting(std::path::PathBuf::from("/tmp/big.log"));
        loading.judge_secret(Some(&[std::path::PathBuf::from("/tmp/big.log")]));
        assert!(loading.loading().is_some() && !loading.is_secret());
        let id = editor.add_buffer(loading);
        let why = refused_switch(&mut dispatcher, &mut editor, "big.log");
        assert!(why.contains("still loading"), "{why}");
        assert_ne!(editor.window().buffer(), id);
    }

    #[test]
    fn a_buffer_the_channel_is_still_opening_is_not_shown_by_name() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("abc"));
        let mut opening = visited("/tmp/plain.txt", &["/tmp/plain.txt"]);
        opening.set_agent_pending(true);
        let id = editor.add_buffer(opening);
        let why = refused_switch(&mut dispatcher, &mut editor, "plain.txt");
        assert!(why.contains("still opening"), "{why}");
        assert_ne!(editor.window().buffer(), id);
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
        // Whitespace's marks show lookalike spaces, so an agent may not turn them off, even
        // with a count that would; the current line's band it may (ADR 0097).
        assert!(
            refused("global-whitespace-mode", Given::Count(0), Reach::Look)
                .contains("needs a person")
        );
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
                state: None,
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
        // The agent's own run leaves nothing for the next saves to be taken as the agent's:
        // its caller starts what it queued.
        assert!(!dispatcher.take_ran_for_agent());
        // The person answers, outside anything the agent runs: what that runs is the agent's.
        editor.answer_question(true).unwrap();
        dispatcher.after_prompts(&mut editor);
        let again = editor.prompt().unwrap();
        assert!(again.from_agent(), "{}", again.label());
        assert!(again.label().starts_with("Agent: Again?"));
        assert!(dispatcher.take_ran_for_agent(), "the y ran for the agent");
        assert!(!dispatcher.take_ran_for_agent(), "and only once");
        // A person's own question stays the person's.
        editor.answer_question(false).unwrap();
        editor.ask("Mine?", "ask-again").unwrap();
        editor.answer_question(true).unwrap();
        dispatcher.after_prompts(&mut editor);
        assert!(!editor.prompt().unwrap().from_agent());
        assert!(!dispatcher.take_ran_for_agent());
    }

    #[test]
    fn a_persons_save_after_an_agents_open_at_a_line_is_the_persons() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("one\ntwo\n"));
        // As the channel's open does when it shows a file at a line.
        let (_, result) = dispatcher
            .run_for_agent(&mut editor, "goto-line", Given::Count(2), Reach::Look)
            .unwrap();
        assert_eq!(result, Ok(()));
        assert!(!dispatcher.take_ran_for_agent());
        let _ = dispatcher.run_command(&mut editor, "save-buffer", None);
        assert!(
            !dispatcher.take_ran_for_agent(),
            "the person's save is not checked"
        );
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
