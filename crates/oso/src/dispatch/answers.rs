//! What answers a native session's request only as the person's key is pressed (ADR 0180, ADR 0182):
//! not from a keyboard macro, nor from input held while something else went on, as both are judged
//! by the time of another key.
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::command::{CommandError, CommandId, ExecuteError};
use oso_core::editor::Editor;

use super::{Dispatcher, Effects};
use crate::commands::agent_sessions::ANSWERS;

const MACRO: &str = "A macro answers no request";
const HELD: &str = "Typed before oso was ready: press it again to answer";

impl Dispatcher {
    /// Why command `id` does not run now, if it answers a request and the person did not just press
    /// its key.
    pub(super) fn answer_refused(&self, editor: &Editor, id: CommandId) -> Option<ExecuteError> {
        let held = self.runs_held;
        let why = match editor.macro_replaying() {
            true => MACRO,
            false if held => HELD,
            false => return None,
        };
        let name = self.registry.get(id)?.name;
        ANSWERS.contains(&name).then(|| ExecuteError::Failed {
            command: name,
            error: CommandError::new(why),
        })
    }

    /// Runs `run`, which handles input held until now.
    pub(super) fn as_held(
        &mut self,
        editor: &mut Editor,
        run: impl FnOnce(&mut Self, &mut Editor) -> Effects,
    ) -> Effects {
        let before = std::mem::replace(&mut self.runs_held, true);
        let effects = run(self, editor);
        self.runs_held = before;
        effects
    }
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Mode;
    use oso_core::editor::{Editor, NativeAction, SessionRequest};
    use oso_core::key::KeySequence;

    use super::{ANSWERS, Dispatcher, MACRO};
    use crate::commands::registry;

    fn line(editor: &mut Editor) {
        let line = editor.add_internal_buffer("*request: s*", Mode::Request);
        editor.show(line);
    }

    fn send(d: &mut Dispatcher, editor: &mut Editor, keys: &str) {
        let keys: KeySequence = keys.parse().unwrap();
        for key in keys.0 {
            d.handle_key(editor, key);
        }
    }

    fn actions(editor: &mut Editor) -> Vec<NativeAction> {
        (editor.take_session_requests().into_iter())
            .filter_map(|request| match request {
                SessionRequest::Native { action, .. } => Some(action),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn what_answers_is_a_command() {
        for name in ANSWERS {
            assert!(registry().lookup(name).is_some(), "{name}");
        }
    }

    #[test]
    fn a_macro_answers_no_request() {
        let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
        line(&mut editor);
        send(&mut d, &mut editor, "C-x ( y C-x )");
        assert_eq!(actions(&mut editor), [NativeAction::Allow]);
        send(&mut d, &mut editor, "C-x e");
        let soon = std::time::Instant::now() + std::time::Duration::from_secs(5);
        d.replay(&mut editor, soon);
        assert_eq!(actions(&mut editor), [], "its only key is the answer");
        let said = editor.messages_buffer();
        let said = editor.buffer(said).unwrap().text().to_string();
        assert!(said.contains(MACRO), "{said}");
        send(&mut d, &mut editor, "n");
        assert_eq!(actions(&mut editor), [NativeAction::Deny]);
    }

    /// What an input method commits, and a paste, is text, which a line takes none of.
    #[test]
    fn a_text_that_is_committed_or_typed_by_no_key_answers_no_request() {
        let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
        line(&mut editor);
        d.commit_text(&mut editor, "y");
        d.insert_text(&mut editor, "n");
        assert_eq!(actions(&mut editor), []);
        assert_eq!(editor.current().1.text().to_string(), "");
    }

    #[test]
    fn input_held_until_now_answers_no_request_and_does_what_else_it_does() {
        let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
        line(&mut editor);
        d.as_held(&mut editor, |d, editor| {
            send(d, editor, "y C-u y n N C-c C-k SPC");
            oso_core::command::Effects::default()
        });
        assert_eq!(actions(&mut editor), [NativeAction::ReadOn(1)]);
        send(&mut d, &mut editor, "C-u y C-c C-k");
        assert_eq!(
            actions(&mut editor),
            [NativeAction::Unread, NativeAction::Deny]
        );
    }
}
