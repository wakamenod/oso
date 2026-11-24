//! What answers a native session's request only as the person's key is pressed (ADR 0180, ADR 0182):
//! not from a keyboard macro, nor from input held while something else went on, nor by the repeat of
//! a key held down, as none of them is a key pressed now.
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::command::{CommandError, CommandId, ExecuteError};
use oso_core::editor::Editor;

use super::{Dispatcher, Effects};
use crate::commands::agent_sessions::{ANSWERS, WRITTEN};

const MACRO: &str = "A macro answers no request";
const HELD: &str = "Typed before oso was ready: press it again to answer";
const REPEATS: &str = "A held key answers once: let it go, and press it again";

impl Dispatcher {
    /// Why command `id` does not run now, if it answers a request and the person did not just press
    /// its key.
    pub(super) fn answer_refused(&self, editor: &Editor, id: CommandId) -> Option<ExecuteError> {
        let name = self.registry.get(id)?.name;
        let macro_runs = editor.macro_replaying();
        let why = match () {
            () if macro_runs && WRITTEN.contains(&name) => MACRO,
            () if !ANSWERS.contains(&name) => return None,
            () if macro_runs => MACRO,
            () if self.runs_held => HELD,
            () if self.key_repeats => REPEATS,
            () => return None,
        };
        Some(ExecuteError::Failed {
            command: name,
            error: CommandError::new(why),
        })
    }

    /// Whether the key handled next is the repeat of one held down, as the host was told.
    pub fn set_key_repeats(&mut self, repeats: bool) {
        self.key_repeats = repeats;
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

    use super::{ANSWERS, Dispatcher, MACRO, WRITTEN};
    use crate::commands::agent_sessions::{CARD_COMMANDS, REQUEST_COMMANDS};
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

    /// Every command of a card, of a plan and of a line answers, is what was written for an answer, or
    /// is named here: a new one is placed.
    #[test]
    fn every_command_of_a_request_is_placed() {
        const NO_ANSWER: [&str; 12] = [
            "native-card-page-down",
            "native-card-page-up",
            "native-card-leave",
            "native-card-edit-plan",
            "native-plan-cancel",
            "native-request-input",
            "native-request-read-on",
            "native-request-read-back",
            "native-request-first",
            "native-request-last",
            "native-request-leave",
            "native-session-request",
        ];
        for command in CARD_COMMANDS.iter().chain(REQUEST_COMMANDS) {
            let placed = [ANSWERS, WRITTEN, &NO_ANSWER[..]]
                .iter()
                .filter(|names| names.contains(&command.name))
                .count();
            assert_eq!(placed, 1, "{}", command.name);
        }
        for name in ANSWERS.iter().chain(WRITTEN) {
            assert!(registry().lookup(name).is_some(), "{name}");
        }
    }

    /// A card's, a plan's and a question's answers, as a line's: from a macro, held, and repeated.
    #[test]
    fn only_a_key_pressed_now_answers_of_every_kind() {
        let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
        let refused = |d: &Dispatcher, editor: &Editor, name: &str| {
            d.answer_refused(editor, registry().lookup(name).unwrap())
                .is_some()
        };
        let all = || ANSWERS.iter().chain(WRITTEN);
        assert!(all().all(|name| !refused(&d, &editor, name)));
        d.set_key_repeats(true);
        assert!(ANSWERS.iter().all(|name| refused(&d, &editor, name)));
        assert!(WRITTEN.iter().all(|name| !refused(&d, &editor, name)));
        assert!(!refused(&d, &editor, "native-request-read-on"));
        d.set_key_repeats(false);
        d.as_held(&mut editor, |d, editor| {
            assert!(ANSWERS.iter().all(|name| refused(d, editor, name)));
            assert!(WRITTEN.iter().all(|name| !refused(d, editor, name)));
            oso_core::command::Effects::default()
        });
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

    /// A yank waits for the clipboard, the person goes to the line meanwhile, and what was typed
    /// after the yank runs there.
    #[test]
    fn a_key_typed_behind_a_yank_answers_no_request() {
        use oso_core::kill::FromClipboard;
        let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
        send(&mut d, &mut editor, "C-y y SPC n");
        assert!(d.take_clipboard_request().is_some());
        line(&mut editor);
        d.clipboard_answered(&mut editor, &FromClipboard::Nothing);
        assert_eq!(actions(&mut editor), [NativeAction::ReadOn(1)]);
        send(&mut d, &mut editor, "n");
        assert_eq!(actions(&mut editor), [NativeAction::Deny]);
    }

    /// What was typed while a macro replayed runs as it ends, in the line the person went to.
    #[test]
    fn a_key_typed_while_a_macro_replays_answers_no_request() {
        let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
        send(&mut d, &mut editor, "C-x ( C-f C-x ) C-x e");
        send(&mut d, &mut editor, "y SPC");
        line(&mut editor);
        let soon = std::time::Instant::now() + std::time::Duration::from_secs(5);
        d.replay(&mut editor, soon);
        assert_eq!(actions(&mut editor), [NativeAction::ReadOn(1)]);
    }

    /// What was typed while a long replacement went on runs once it has ended.
    #[test]
    fn a_key_typed_while_work_went_on_answers_no_request() {
        let (mut d, mut editor) = super::super::motion_tests::at(&"a ".repeat(3000), &[0]);
        send(&mut d, &mut editor, "M-% a RET b RET !");
        send(&mut d, &mut editor, "y SPC");
        while editor.replace_works() {
            editor.replace_continue();
        }
        line(&mut editor);
        d.run_held_during_all(&mut editor);
        assert_eq!(actions(&mut editor), [NativeAction::ReadOn(1)]);
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
