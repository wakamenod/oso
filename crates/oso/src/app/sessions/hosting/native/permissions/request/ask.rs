//! The y-or-n question before a native session's call that was not all read runs (ADR 0182,
//! "Reading", `C-u y`): it names the tool and says what of the call was not read.
//!
//! Must not depend on: key presses; the line's keys ask it, and its `y` has the call answered.
//!
//! - It is kept for the call it asked about, and its `y` allows that call alone, if it still waits.
//! - The `y` counts only after [`QUIET`](super::gate::waited) since the question opened, or oso's
//!   window came in front again, longer after a prompt an agent opened closed, and since the key
//!   before it; never from a macro, nor as the repeat of a held key.
//! - It closes as oso's window loses the front or goes out of sight, and with its call; one that
//!   closed otherwise is dropped.

use std::time::Instant;

use oso_core::minibuffer::PromptId;

use super::gate::{Pressed, waited};
use crate::app::App;
use crate::commands::agent_sessions::PLAN_CONFIRMED;
use crate::sessions::native::asks::Asks;
use crate::sessions::native::request::Block;
use crate::sessions::native::request::body::Hidden;
use crate::sessions::native::request::reading::Reading;
use crate::sessions::worker::SessionKey;

/// The question `prompt`, asked `at`, before session `key`'s call `of` runs.
#[derive(Debug)]
pub(super) struct Asked {
    prompt: PromptId,
    at: Instant,
    key: SessionKey,
    of: String,
}

impl Asked {
    /// Whether the call it asked about is the one its session shows.
    fn waits<R>(&self, asks: &Asks<R>) -> bool {
        (asks.shown(self.key))
            .is_some_and(|ask| ask.request().is_some() && ask.call.tool_use_id == self.of)
    }
}

/// The question before `block`'s call, of session `name`, of which `reading` was read, runs.
fn question(name: &str, block: &Block, reading: &Reading) -> String {
    let (read, of) = reading.lines();
    let unread = of - read;
    let said = match reading.hidden() {
        Hidden::All => "its input is not shown at all".to_owned(),
        _ if unread == 1 => format!("1 of its {of} rows is not read"),
        _ => format!("{unread} of its {of} rows are not read"),
    };
    format!("Allow {} in {name} unread: {said}? (y or n)", block.tool())
}

impl App {
    /// Asks whether session `key`'s call runs though not all of it was read.
    pub(super) fn ask_unread(&mut self, key: SessionKey) {
        let Some(ask) = self.hosting.natives.permissions.asks.shown(key) else {
            return;
        };
        let Some(request) = ask.request() else {
            return;
        };
        let name = (self.hosting.table.get(key)).map_or("", |s| s.name.as_str());
        let (of, question) = (
            ask.call.tool_use_id.clone(),
            question(name, &request.block, &request.reading),
        );
        // One question at a time is what `y` answers.
        self.drop_plan_question();
        self.close_unread_question();
        match self.editor.ask(question, PLAN_CONFIRMED) {
            Ok(prompt) => {
                self.hosting.natives.permissions.requests.asked = Some(Asked {
                    prompt,
                    at: Instant::now(),
                    key,
                    of,
                });
            }
            Err(e) => self.editor.message(format!("Cannot ask: {e}")),
        }
        self.request_redraw();
    }

    /// `y` to the question asked last: its call allowed, if it still waits. Whether one was asked.
    pub(in crate::app::sessions::hosting::native::permissions) fn unread_confirmed(
        &mut self,
    ) -> bool {
        let Some(asked) = self.hosting.natives.permissions.requests.asked.take() else {
            return false;
        };
        let pressed = Pressed::last(self.person_typed_at(), Instant::now());
        let since = self.waits_after_agents(asked.at);
        if self.editor.macro_replaying() || !waited(since, pressed) {
            self.editor.message(
                "Not allowed: wait a moment after the question, then answer it; C-u y asks again",
            );
            return true;
        }
        if asked.waits(&self.hosting.natives.permissions.asks) {
            self.answer_card(asked.key, super::super::card::Answer::Allow);
        } else {
            self.editor
                .message("Not allowed: the call asked about no longer waits");
        }
        true
    }

    /// The question before a call runs, if one is open, is new to the person, as they come back to
    /// oso's window: its wait starts again.
    pub(in crate::app::sessions::hosting::native::permissions) fn unread_question_waits_again(
        &mut self,
    ) {
        if let Some(asked) = &mut self.hosting.natives.permissions.requests.asked {
            asked.at = asked.at.max(Instant::now());
        }
    }

    /// oso's window is no longer in front, or in sight: the question before a call runs closes.
    pub(in crate::app) fn close_unread_question(&mut self) {
        if let Some(asked) = self.hosting.natives.permissions.requests.asked.take() {
            self.close_question(asked.prompt);
        }
    }

    /// Session `key`'s call went: the question about it, if one is open, closes.
    pub(super) fn drop_unread_question(&mut self, key: SessionKey) {
        let asked = &self.hosting.natives.permissions.requests.asked;
        if asked.as_ref().is_some_and(|asked| asked.key == key) {
            self.close_unread_question();
        }
    }

    /// After an event: the question before a call runs, once it is no longer open, under another
    /// prompt or not, and was not answered `y` in it, is dropped, so that no later `y` allows.
    pub(in crate::app::sessions::hosting::native::permissions) fn drop_unanswered_unread_question(
        &mut self,
    ) {
        let Some(asked) = self.hosting.natives.permissions.requests.asked.as_ref() else {
            return;
        };
        let prompts = self.editor.prompts();
        if !prompts.iter().any(|prompt| prompt.id() == asked.prompt) {
            self.hosting.natives.permissions.requests.asked = None;
        }
    }

    /// Whether the question before a call runs, if it is the open prompt, takes its `y` now. For
    /// end-to-end flows.
    #[cfg(feature = "dev")]
    pub(in crate::app) fn unread_question_takes_answer(&self) -> Option<bool> {
        let asked = self.hosting.natives.permissions.requests.asked.as_ref()?;
        let open = self.editor.prompt().map(|prompt| prompt.id());
        let pressed = Pressed::next(self.person_typed_at(), Instant::now());
        (open == Some(asked.prompt)).then(|| waited(self.waits_after_agents(asked.at), pressed))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use serde_json::json;

    use super::{Asked, question};
    use crate::channel::Connection;
    use crate::channel::protocol::permission::Call;
    use crate::sessions::native::asks::{Ask, Asks, Look, RequestId};
    use crate::sessions::native::request::{Block, Request};

    #[test]
    fn a_yes_allows_only_the_call_asked_about() {
        let now = Instant::now();
        let mut editor = oso_core::editor::Editor::default();
        let asked = Asked {
            prompt: editor.ask("Allow?", "keyboard-quit").unwrap(),
            at: now,
            key: 7,
            of: "t1".to_owned(),
        };
        let mut asks = Asks::default();
        assert!(!asked.waits(&asks));
        // One call after another of the same session, and one of another.
        for (key, id, waits) in [(8, "t1", false), (7, "t1", true), (7, "t2", false)] {
            asks.push(Ask {
                id: RequestId::default(),
                from: Connection {
                    run: 1,
                    number: key,
                },
                key,
                call: Call {
                    tool_name: "Bash".to_owned(),
                    tool_use_id: id.to_owned(),
                    input: None,
                },
                came: now,
                made: Some(Box::new(Block::call("Bash", None, now))),
                asked: None,
                shown: None,
                reply: (),
            });
            asks.look(now, |_| Look::Call);
            for ask in asks.unshown() {
                ask.shown = Some(now);
            }
            assert_eq!(asked.waits(&asks), waits, "{key} {id}");
            if key == 7 {
                asks.take_shown(key).unwrap();
            }
        }
    }

    fn asked(tool: &str, input: Option<&serde_json::Value>) -> String {
        let request = Request::new(Block::call(tool, input, Instant::now()));
        question("s", &request.block, &request.reading)
    }

    #[test]
    fn the_question_names_the_tool_and_says_what_was_not_read() {
        let input = json!({"command": "ls\npwd", "timeout": 5});
        assert_eq!(
            asked("Bash", Some(&input)),
            "Allow Bash in s unread: 4 of its 4 rows are not read? (y or n)"
        );
        assert_eq!(
            asked("Bash", Some(&json!({"command": "ls"}))),
            "Allow Bash in s unread: 1 of its 1 rows is not read? (y or n)"
        );
        assert_eq!(
            asked("x?", None),
            "Allow x\\u{3f} in s unread: its input is not shown at all? (y or n)"
        );
    }
}
