//! Text as input: from a key that types several characters and from an input method's
//! composition and commit, through the text command of the active keymaps (ADR 0069).
//!
//! Must not depend on: `app` or winit.

use oso_core::command::{Args, Effects, Invocation};
use oso_core::editor::Editor;

use super::shown::{masked, reads_passphrase};
use super::{Dispatcher, Step};

impl Dispatcher {
    /// Inserts `text`, which may hold several characters, as a key that types
    /// them would: through the text command of the active keymaps, as one command
    /// and one undo step. A pending prefix makes it an undefined key sequence.
    /// A key that types several characters comes through here.
    pub fn insert_text(&mut self, editor: &mut Editor, text: &str) -> Effects {
        self.take_text(editor, text, false)
    }

    /// Starts an input method's composition, which counts as a key: keys never reach
    /// oso during one (ADR 0069).
    pub fn begin_composition(&mut self, editor: &mut Editor) {
        if self.hold_composition() {
            return;
        }
        self.as_input(editor, |_, _| ());
    }

    /// Inserts an input method's commit, as [`Dispatcher::insert_text`] does, but
    /// always as an undo step of its own, even of one character (ADR 0069).
    pub fn commit_text(&mut self, editor: &mut Editor, text: &str) -> Effects {
        self.take_text(editor, text, true)
    }

    pub(super) fn run_text(&mut self, editor: &mut Editor, text: &str, step: Step) -> Effects {
        // Text changes the input, so a key held for its older result is dropped (ADR 0096).
        self.drop_held_completion();
        // Text answers no question, and says so, as another key does.
        if editor
            .prompt()
            .is_some_and(|prompt| prompt.kind().is_question())
        {
            editor.prompt_says(crate::commands::answer_hint(editor));
            return Effects {
                redraw: true,
                ..Effects::default()
            };
        }
        // A menu binds no text: its `foreign-keys` says what text does there (ADR 0117).
        if self.menu_refuses_text(editor) {
            return Effects::default();
        }
        if !self.resolver.pending().is_empty() {
            let pending = self.resolver.pending();
            log::debug!("{} and text are undefined", masked(pending, true));
            // The text stays out of it, as it does from a log.
            let keys = masked(pending, reads_passphrase(editor));
            editor.echo(format!("{keys} is undefined"));
            self.resolver.reset();
            self.forget_keys(editor);
            return Effects::default();
        }
        let reading = self.keymaps.reading(editor);
        let text_command = self
            .keymaps
            .active(&reading, self.prefix, editor)
            .text_command();
        match text_command {
            Some(id) if !text.is_empty() => {
                self.drop_branch_wait(editor);
                let args = Args::with_prefix(self.prefix.take());
                let invocation = Invocation {
                    input: text,
                    ..Invocation::default()
                };
                let effects = self.execute(editor, id, args, invocation, step).0;
                effects.union(self.after_prompts(editor))
            }
            _ => Effects::default(),
        }
    }
}
