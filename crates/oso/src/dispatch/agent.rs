//! Running a command for the agent channel (ADR 0085): by name, only if its reach is the one
//! asked for and not `Person`, with its argument checked against its spec, and never with
//! what only a person gives: a key, typed text, the clipboard or a `y` to a question.
//!
//! Must not depend on: the channel's socket or protocol, or `app`; the host decides when the
//! channel may run anything at all.

use oso_core::command::{ArgSpec, Args, Invocation, Reach};
use oso_core::editor::Editor;

use super::{Dispatcher, Effects, Step};

/// The largest count the channel passes on, in either direction: a command runs on the UI
/// thread and may repeat its work this many times (ADR 0044).
pub const MAX_AGENT_COUNT: isize = 10_000;

/// Why the channel did not run a command. Its message names only a registered command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub String);

impl Dispatcher {
    /// Every registered command, for the channel to list those it may run.
    pub fn commands(&self) -> impl Iterator<Item = &oso_core::command::Command> {
        self.registry.iter().map(|(_, command)| command)
    }

    /// Runs the command `name` for the agent channel, if its reach is `reach`, with `arg` as
    /// its count or number. No prompt opens for an argument: a `Number` command without one
    /// is refused. Returns what the host should do, and whether the command succeeded, as
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
        arg: Option<isize>,
        reach: Reach,
    ) -> Result<(Effects, Result<(), String>), Refused> {
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
        if self.waits_for_clipboard() {
            return refused("a yank is waiting for the clipboard".into());
        }
        if arg.is_some_and(|n| n.unsigned_abs() > MAX_AGENT_COUNT.unsigned_abs()) {
            return refused(format!("{name}'s argument is beyond ±{MAX_AGENT_COUNT}"));
        }
        let answer;
        let mut invocation = Invocation::default();
        let args = match (command.args, arg) {
            (ArgSpec::None, None) => Args::NONE,
            (ArgSpec::None, Some(_)) => return refused(format!("{name} takes no argument")),
            (ArgSpec::Count, arg) => arg.map_or(Args::NONE, Args::numeric),
            (ArgSpec::Number { .. }, Some(n)) => {
                answer = n.to_string();
                invocation.answer = Some(&answer);
                Args::NONE
            }
            (ArgSpec::Number { .. }, None) => return refused(format!("{name} needs a number")),
            // `Person` commands, refused above; listed so that a new spec must be decided.
            (ArgSpec::Passphrase { .. } | ArgSpec::CountAndClipboard, _) => {
                return refused(format!("{name} needs a person at the keyboard"));
            }
        };
        // Whatever the person's keys left, the agent's command starts afresh.
        self.prefix = None;
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

#[cfg(test)]
mod tests {
    use super::{MAX_AGENT_COUNT, Refused};
    use crate::dispatch::Dispatcher;
    use oso_core::buffer::Buffer;
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
        arg: Option<isize>,
        reach: Reach,
    ) -> String {
        match dispatcher.run_for_agent(editor, name, arg, reach) {
            Err(Refused(why)) => why,
            Ok(outcome) => panic!("{name} ran: {outcome:?}"),
        }
    }

    #[test]
    fn runs_a_look_command_with_a_count() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("abcdef"));
        let (_, result) = dispatcher
            .run_for_agent(&mut editor, "forward-char", Some(3), Reach::Look)
            .unwrap();
        assert_eq!(result, Ok(()));
        assert_eq!(cursor(&editor), 3);
        let (_, result) = dispatcher
            .run_for_agent(&mut editor, "goto-line", Some(1), Reach::Look)
            .unwrap();
        assert_eq!(result, Ok(()));
        assert!(editor.prompt().is_none());
    }

    #[test]
    fn refuses_what_needs_a_person_or_another_reach() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("abc"));
        let mut refused =
            |name, arg, reach| refusal(&mut dispatcher, &mut editor, name, arg, reach);
        assert_eq!(
            refused("no-such", None, Reach::Look),
            "no command by that name"
        );
        assert!(refused("yank", None, Reach::Change).contains("needs a person"));
        assert!(
            refused("save-buffers-kill-terminal", None, Reach::Change).contains("needs a person")
        );
        assert!(refused("answer-yes", None, Reach::Look).contains("needs a person"));
        assert!(refused("delete-char", None, Reach::Look).contains("changes"));
        assert!(refused("forward-char", None, Reach::Change).contains("only looks"));
        assert!(refused("goto-line", None, Reach::Look).contains("needs a number"));
        assert!(refused("mark-whole-buffer", Some(1), Reach::Look).contains("takes no argument"));
        let beyond = Some(MAX_AGENT_COUNT + 1);
        assert!(refused("forward-char", beyond, Reach::Look).contains("beyond"));
        assert!(refused("forward-char", Some(isize::MIN), Reach::Look).contains("beyond"));
        assert_eq!(editor.current().1.text().to_string(), "abc");
    }

    #[test]
    fn a_refusal_names_no_unregistered_command() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor(""));
        let why = refusal(
            &mut dispatcher,
            &mut editor,
            "\u{202e}evil",
            None,
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
            .run_for_agent(&mut editor, "kill-word", None, Reach::Change)
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
    fn a_change_runs_as_an_undo_step_of_its_own() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("abc"));
        for _ in 0..2 {
            let (_, result) = dispatcher
                .run_for_agent(&mut editor, "delete-char", None, Reach::Change)
                .unwrap();
            assert_eq!(result, Ok(()));
        }
        assert_eq!(editor.current().1.text().to_string(), "c");
        editor.undo().unwrap();
        assert_eq!(editor.current().1.text().to_string(), "bc");
    }
}
