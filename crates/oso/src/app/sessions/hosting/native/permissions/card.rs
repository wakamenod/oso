//! A native session's permission card in the app (ADR 0165, step 2e-2, the author's (a) on Q1
//! to Q4): `*card: NAME*`, docked between the session's transcript and its composer while a
//! call of the session waits for the person, and gone once answered. The card's text is
//! `sessions::native::card`'s; this makes it from the call, docks it, moves point to it,
//! carries out its keys and answers the session's bridge.
//!
//! Must not depend on: key presses or how a frame is drawn; the card's commands ask, and this
//! carries out.
//!
//! - The card is the only place the call is asked (Q1 (a)): elsewhere the session's `needs
//!   input` leads there, as a terminal's dialog does.
//! - Point moves to it after [`QUIET`] with no key, while the person is in that session's
//!   transcript or composer; and in it, a key answers only after [`QUIET`] with no key, the
//!   wait starting again with each key and with point's coming (`agent::questions`).
//! - What the card shows of a call is what the session's own stream says of it, the request
//!   saying the same; but an `ExitPlanMode`'s plan, which only the request carries (measured,
//!   CLI 2.1.330), is shown as the request's (Q2 (a)).

use std::time::Instant;

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::NativeAction;
use oso_core::syntax::Highlights;
use oso_core::window::DockSlot;
use serde_json::Value;

use super::DENIED;
use crate::app::App;
use crate::app::agent::QUIET;
use crate::channel::protocol::permission::{self, MAX_INPUT};
use crate::commands::agent_sessions::{CARD_NOTE, CARD_OWN};
use crate::git::printable;
use crate::sessions::native::asks::Question;
use crate::sessions::native::card::{self, Card, Questions, Step};
use crate::sessions::native::render::{self, Face};
use crate::sessions::native::rows::Tool;
use crate::sessions::native::wire::card as read;
use crate::sessions::native::wire::tools::{decode, only_keys, pretty};
use crate::sessions::state::{Reason, Target};
use crate::sessions::worker::SessionKey;

/// The most bytes of a call's input `RET` shows.
const MAX_SHOWN: usize = 4 * MAX_INPUT;

/// What the person writes in the minibuffer for a card, as its `N` or `o` asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app::sessions::hosting::native) enum Writing {
    Note(SessionKey),
    Own(SessionKey),
}

/// What a card's answer comes to.
enum Answer {
    Allow,
    /// Each question with its answer.
    Answers(Vec<(String, String)>),
    Deny(String),
}

impl App {
    /// After each sync: each request found whose card is not shown gets its card, docked
    /// under its session's transcript.
    pub(in crate::app::sessions::hosting::native) fn show_cards(&mut self) {
        let now = Instant::now();
        let shown: Vec<SessionKey> = (self.hosting.natives.permissions.asks.unshown())
            .map(|ask| {
                ask.shown = Some(now);
                log::debug!(
                    "native sessions: a permission card shown {} ms after its request",
                    now.saturating_duration_since(ask.came).as_millis()
                );
                ask.key
            })
            .collect();
        for key in shown {
            self.draw_card(key);
            self.hosting.natives.permissions.focus_pending.push(key);
        }
        if !self.hosting.natives.permissions.focus_pending.is_empty() {
            self.request_redraw();
        }
    }

    /// Writes session `key`'s card as its request's question has it, and docks it.
    fn draw_card(&mut self, key: SessionKey) {
        let Some(card) = (self.hosting.natives.permissions.asks.shown(key))
            .and_then(|ask| ask.question.as_ref())
            .map(|question| match question.whole {
                true => card::whole(&question.card, &question.details),
                false => question.card.shown(),
            })
        else {
            return;
        };
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        let transcript = native.buffer;
        let id = match native.card.filter(|&id| self.editor.buffer(id).is_some()) {
            Some(id) => id,
            None => {
                let name = self.hosting.table.get(key).map_or("", |s| s.name.as_str());
                let id = self
                    .editor
                    .add_internal_buffer(format!("*card: {name}*"), Mode::Card);
                if let Some(buffer) = self.editor.buffer_mut(id) {
                    buffer.withhold_from_agents();
                }
                if let Some(native) = self.hosting.natives.map.get_mut(&key) {
                    native.card = Some(id);
                }
                id
            }
        };
        self.editor.set_internal_text(id, &card.text);
        let mut highlights = Highlights::new(card.text.len());
        for (range, face) in &card.faces {
            let run = [(range.len(), Some(face.highlight()))];
            highlights.replace(range.clone(), run);
        }
        let _ = self.editor.set_internal_highlights(id, highlights);
        self.editor.set_dock(transcript, DockSlot::Card, Some(id));
    }

    /// Session `key`'s card goes, as its call was answered or went: point, if it was there,
    /// goes to the composer.
    pub(in crate::app::sessions::hosting::native) fn close_card(&mut self, key: SessionKey) {
        let permissions = &mut self.hosting.natives.permissions;
        permissions.focus_pending.retain(|&pending| pending != key);
        if matches!(permissions.writing, Some(Writing::Note(k) | Writing::Own(k)) if k == key) {
            permissions.writing = None;
        }
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        let (transcript, card) = (native.buffer, native.card);
        let was_in = card.is_some_and(|card| self.editor.window().buffer() == card);
        self.editor.set_dock(transcript, DockSlot::Card, None);
        if let Some(card) = card {
            self.editor.set_internal_text(card, "");
        }
        if was_in {
            self.composer_go(key, true);
        }
        self.request_redraw();
    }

    /// A key of session `key`'s card: `action`. Whether it was one of the card's.
    pub(in crate::app::sessions::hosting::native) fn card_key(
        &mut self,
        key: SessionKey,
        action: NativeAction,
    ) -> bool {
        let Some(card) = (self.hosting.natives.permissions.asks.shown(key))
            .and_then(|ask| ask.question.as_ref())
            .map(|question| question.card.clone())
        else {
            self.editor.message("No call waits for you here");
            return matches!(
                action,
                NativeAction::Allow
                    | NativeAction::Deny
                    | NativeAction::Note
                    | NativeAction::Choose(_)
                    | NativeAction::Other
            );
        };
        let questions = matches!(card, Card::Questions(_));
        match action {
            NativeAction::Allow if questions => {
                self.editor
                    .message("Choose an option, or o to answer in your words");
            }
            NativeAction::Allow => self.answer_card(key, Answer::Allow),
            NativeAction::Deny => {
                let why = match card {
                    Card::Questions(_) => "The person declined to answer in oso.",
                    Card::Plan { .. } => "The person wants you to keep planning, in oso.",
                    Card::Call { .. } => DENIED,
                };
                self.answer_card(key, Answer::Deny(why.to_owned()));
            }
            NativeAction::Note => self.write_for_card(Writing::Note(key)),
            NativeAction::Other if questions => self.write_for_card(Writing::Own(key)),
            NativeAction::Other => self.editor.message("This card asks no question"),
            NativeAction::Choose(number) if questions => {
                self.step_questions(key, |questions| questions.choose(number));
            }
            NativeAction::Choose(_) => self.editor.message("This card has no options"),
            // A question that takes several is answered with those chosen; else the whole call.
            NativeAction::Open => {
                let whole = (self.hosting.natives.permissions.asks.shown(key))
                    .and_then(|ask| ask.question.as_ref())
                    .is_some_and(|question| question.whole);
                let multi = !whole
                    && matches!(&card, Card::Questions(q) if q.current().is_some_and(|a| a.multi));
                if multi {
                    self.step_questions(key, Questions::confirm);
                } else {
                    self.toggle_whole(key);
                }
            }
            _ => return false,
        }
        true
    }

    /// Opens `writing`'s prompt, whose answer comes back as [`App::card_written`].
    fn write_for_card(&mut self, writing: Writing) {
        let command = match writing {
            Writing::Note(_) => CARD_NOTE,
            Writing::Own(_) => CARD_OWN,
        };
        if self.open_host_prompt(command).is_some() {
            self.hosting.natives.permissions.writing = Some(writing);
        }
    }

    /// The person wrote `text` for the card the last `N` or `o` asked about.
    pub(in crate::app) fn card_written(&mut self, text: &str) {
        let Some(writing) = self.hosting.natives.permissions.writing.take() else {
            return;
        };
        match writing {
            Writing::Note(key) => {
                let note = printable(text, permission::MAX_MESSAGE / 2);
                let said = format!("The person denied this in oso, saying: {note}");
                self.answer_card(key, Answer::Deny(said));
            }
            Writing::Own(key) => self.step_questions(key, |questions| questions.own(text)),
        }
    }

    /// Carries `step` out on session `key`'s questions: the card shows the next, or every
    /// answer goes.
    fn step_questions(&mut self, key: SessionKey, step: impl FnOnce(&mut Questions) -> Step) {
        let asks = &mut self.hosting.natives.permissions.asks;
        let Some(Card::Questions(questions)) = (asks.shown_mut(key))
            .and_then(|ask| ask.question.as_mut())
            .map(|question| &mut question.card)
        else {
            return;
        };
        match step(questions) {
            Step::Done => {
                let answers = questions.answers().unwrap_or_default();
                self.answer_card(key, Answer::Answers(answers));
            }
            Step::Next | Step::Toggled => {
                self.draw_card(key);
                self.request_redraw();
            }
            Step::Nothing => self.editor.message("No such option"),
        }
    }

    /// Answers session `key`'s shown request as `answer` says: its bridge is told, its call's
    /// row says what came of it, and its card goes.
    fn answer_card(&mut self, key: SessionKey, answer: Answer) {
        let Some(ask) = self.hosting.natives.permissions.asks.take_shown(key) else {
            return;
        };
        let (reply, allowed, said) = match answer {
            Answer::Allow => (permission::allow(), true, "allowed"),
            Answer::Answers(answers) => (permission::allow_answering(&answers), true, "answered"),
            Answer::Deny(why) => (permission::deny(&why), false, "denied"),
        };
        self.agent_log(&format!(
            "a native session's {}: {said}",
            ask.call.tool_name
        ));
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            let note = (!allowed)
                .then(|| reply.get("message").and_then(Value::as_str))
                .flatten();
            if let Some(change) = native.rows.answered(&ask.call.tool_use_id, allowed, note) {
                super::super::draw(&mut self.editor, native, [change]);
            }
        }
        self.tell_tracker(key, Target::Working);
        let _ = ask.reply.send(Ok(reply));
        self.close_card(key);
    }

    /// `RET` on a card: it shows the call's whole input, or the whole plan, or goes back.
    fn toggle_whole(&mut self, key: SessionKey) {
        let asks = &mut self.hosting.natives.permissions.asks;
        if let Some(question) = asks.shown_mut(key).and_then(|ask| ask.question.as_mut()) {
            question.whole = !question.whole;
        }
        self.draw_card(key);
        self.request_redraw();
    }

    /// When point is next to move to a card, if one waits for it: [`QUIET`] after it was
    /// shown and after the person's last key.
    pub(in crate::app) fn card_focus_deadline(&self) -> Option<Instant> {
        let permissions = &self.hosting.natives.permissions;
        let typed = self.agent_person_typed();
        (permissions.focus_pending.iter())
            .filter_map(|&key| permissions.asks.shown(key)?.shown)
            .map(|shown| typed.map_or(shown, |typed| typed.max(shown)) + QUIET)
            .filter(|&due| due > Instant::now())
            .min()
    }

    /// Before the loop waits: point goes to a card that waits for it, once [`QUIET`] passed
    /// with no key, while the person is in its session's transcript or composer.
    pub(in crate::app) fn card_focus_due(&mut self, now: Instant) {
        if self.hosting.natives.permissions.focus_pending.is_empty() {
            return;
        }
        let quiet = self
            .agent_person_typed()
            .is_none_or(|typed| now >= typed + QUIET);
        let selected = self.editor.window().buffer();
        let pending = self.hosting.natives.permissions.focus_pending.clone();
        for key in pending {
            let Some(native) = self.hosting.natives.map.get(&key) else {
                continue;
            };
            let in_session = selected == native.buffer || native.composing.buffer == Some(selected);
            let shown = (self.hosting.natives.permissions.asks.shown(key)).and_then(|a| a.shown);
            let due = shown.is_some_and(|shown| now >= shown + QUIET);
            if !(quiet && due && in_session) {
                continue;
            }
            let Some(window) = self.editor.dock_window(native.buffer, DockSlot::Card) else {
                continue;
            };
            self.editor.select_window(window);
            self.card_focused();
            (self.hosting.natives.permissions.focus_pending).retain(|&pending| pending != key);
            self.request_redraw();
        }
    }

    /// Whether `buffer` is a native session's card, and whose.
    pub(in crate::app::sessions::hosting::native) fn card_session(
        &self,
        buffer: BufferId,
    ) -> Option<SessionKey> {
        (self.hosting.natives.map.iter())
            .find_map(|(key, native)| (native.card == Some(buffer)).then_some(*key))
    }
}

/// The state a session waits in while its call's card is shown.
pub(in crate::app::sessions::hosting::native) fn waits_for(tool: &str) -> Target {
    Target::NeedsInput(match tool {
        "AskUserQuestion" => Reason::Question,
        "ExitPlanMode" => Reason::Plan,
        _ => Reason::Permission,
    })
}

/// Whether the request's `input` says of the call what the stream's `tool` does: the same
/// target, as both are read the same way. A request with no input, as its input was too large
/// to send, says the same only of a call whose input the stream found too large too. An
/// `ExitPlanMode`'s stream carries no plan (measured): its id and tool are the match.
pub(in crate::app::sessions::hosting::native) fn says_the_same(
    tool: &Tool,
    input: Option<&Value>,
) -> bool {
    if tool.name == "ExitPlanMode" {
        return input.is_some();
    }
    let Some(input) = input else {
        return tool.input.cut;
    };
    let sent = decode(&tool.name, Some(input));
    let kept = &tool.input;
    (sent.path == kept.path && sent.text == kept.text)
        && (sent.kind == kept.kind && sent.items == kept.items)
}

/// The card about the call `tool` of session `name`, which runs in `directory`, with the
/// request's `input`, and what `RET` shows of it.
pub(in crate::app::sessions::hosting::native) fn question(
    name: &str,
    tool: &Tool,
    directory: &std::path::Path,
    input: Option<&Value>,
) -> Question {
    let whole = input.map_or_else(
        || {
            format!(
                "Its input is over {} KiB as JSON and is not shown here.",
                MAX_INPUT >> 10
            )
        },
        |input| pretty(input, MAX_SHOWN),
    );
    let shown: Vec<String> = whole
        .lines()
        .map(|line| printable(line, MAX_SHOWN))
        .collect();
    let mut details = format!(
        "Session {name} asks to run {}:\n\n{}\n",
        tool.name,
        shown.join("\n")
    );
    let card = match (tool.name.as_str(), input) {
        ("AskUserQuestion", Some(input)) => {
            read::questions(input).map(|asked| Card::Questions(Questions::new(asked)))
        }
        ("ExitPlanMode", Some(input)) => read::plan(input).map(|(plan, cut)| {
            details = format!("Session {name}'s plan, as its request gives it:\n\n{plan}\n");
            Card::Plan { plan, cut }
        }),
        _ => None,
    };
    let card = card.unwrap_or_else(|| call(name, tool, directory, input));
    Question {
        card,
        details,
        whole: false,
    }
}

/// The card of a call that runs: its tool and target, what it runs or changes, and why.
fn call(name: &str, tool: &Tool, directory: &std::path::Path, input: Option<&Value>) -> Card {
    let target = render::target(tool, directory);
    let field = |key: &str| {
        input
            .and_then(|input| input.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let mut lines = Vec::new();
    let title = match tool.name.as_str() {
        "Bash" => {
            lines.extend(field("command").map(|command| (format!("$ {command}"), Face::Target)));
            format!("Bash wants to run, in session {name}")
        }
        "Edit" | "MultiEdit" | "Write" => {
            lines.push((target.clone(), Face::Target));
            if let Some(input) = input {
                let (diff, cut) = read::diff(&tool.name, input);
                lines.extend(diff.into_iter().map(|(mark, line)| {
                    let face = if mark == '-' {
                        Face::Removed
                    } else {
                        Face::Added
                    };
                    (format!("{mark}{line}"), face)
                }));
                if cut {
                    lines.push((
                        "(more of the change is not shown: RET shows all)".to_owned(),
                        Face::Dim,
                    ));
                }
            }
            format!("{} wants to change a file, in session {name}", tool.name)
        }
        other => {
            if !target.is_empty() {
                lines.push((target.clone(), Face::Target));
            }
            format!("{other} wants to run, in session {name}")
        }
    };
    if let Some(why) = field("description").filter(|why| !why.is_empty()) {
        lines.push((format!("Why: {why}"), Face::Target));
    }
    let all = !target.ends_with('…') && input.is_some_and(|input| shows_all(&tool.name, input));
    let more = if all {
        ""
    } else {
        " (not all of it is shown: RET shows it)"
    };
    let title = printable(&format!("{title}{more}"), 200);
    Card::Call { title, lines }
}

/// Whether what a card shows of a call of `name` is all its `input` does: for the calls that
/// read, a command with nothing but its description and limit, and an edit with nothing but
/// its file and strings.
fn shows_all(name: &str, input: &Value) -> bool {
    match name {
        "Read" | "Glob" | "Grep" | "WebSearch" | "WebFetch" => true,
        "Bash" => only_keys(input, &["command", "description", "timeout"]),
        "Edit" => only_keys(input, &["file_path", "old_string", "new_string"]),
        "Write" => only_keys(input, &["file_path", "content"]),
        _ => false,
    }
}
