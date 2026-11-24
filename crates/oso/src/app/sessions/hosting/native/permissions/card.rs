//! A native session's permission card in the app (ADR 0165, step 2e-2, the author's (a) on Q1
//! to Q4): `*card: NAME*`, docked between the session's transcript and its composer while a
//! call of the session waits for the person, and gone once answered. The card's text is
//! `sessions::native::card`'s; this docks it at its window's width, moves point to it, carries
//! out its keys and answers the session's bridge.
//!
//! Must not depend on: key presses or how a frame is drawn; the card's commands ask, and this
//! carries out.
//!
//! - The card is the only place the call is asked (Q1 (a)): elsewhere the session's `needs
//!   input` leads there, as a terminal's dialog does.
//! - Point moves to it as it appears, while the person is in that session's transcript or its
//!   empty composer and in the middle of nothing, or after [`QUIET`] with no key once that
//!   holds (`focus`, ADR 0180).
//! - Its answers (`y`, `n`, `N`, a digit, `o`, `RET`, and a plan's `a`, `A`, `e`) count only
//!   after [`QUIET`] with no key and since the card became the selected buffer, however point
//!   came; never from a macro, and never while the card's window shows less than all of it
//!   (`gate`). Other keys, as `C-c C-o` or `C-x o` to leave it, are any buffer's.
//! - Some of them answer from the session's empty composer too (`composer`, ADR 0180).
//! - It is drawn again when its window's width changes, as its rows are broken at it.
//! - A plan's card approves with a mode after it, or edits the plan first: `plan` (part 3d).

mod composer;
mod focus;
mod front;
mod gate;
mod pages;
mod plan;
mod size;

pub(in crate::app) use front::RequestWaits;
pub(super) use plan::Plans;

use std::time::Instant;

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::{Approval, NativeAction};
use oso_core::syntax::Highlights;
use oso_core::window::DockSlot;

use super::DENIED;
use crate::app::App;
use crate::channel::protocol::permission;
use crate::commands::agent_sessions::{CARD_NOTE, CARD_OWN};
use crate::git::printable;
use crate::sessions::native::card::{self, Card, Questions, Shown, Step};
use crate::sessions::state::Target;
use crate::sessions::worker::SessionKey;

/// How many times a card is drawn again at most as the docks settle on its rows.
const SETTLES: usize = 4;
/// What a denial without a note says on its call's row.
const BY_THE_PERSON: &str = "by the person";

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
    /// A plan approved: the person's edit of it, if any, and the mode after it (part 3d).
    Plan(Option<String>, Approval),
    /// What the CLI is told, and what the call's row says.
    Deny(String, String),
}

/// What an answered call's row says of the person's answer.
enum Mark {
    Denied(String),
    /// A plan's: whether the person edited it, and the mode after it (part 3d).
    Approved(bool, Approval),
}

/// Where an answer to a card may come from now, and since when: the card, its session's empty
/// composer (ADR 0180) or its plan's edit (part 3d), whichever is the selected buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app::sessions::hosting::native) struct Current {
    key: SessionKey,
    place: BufferId,
    since: Instant,
    /// The card's window showed all of it when this was last looked at.
    whole: bool,
}

impl App {
    /// After each sync: each request found whose card is not shown gets its card, docked
    /// under its session's transcript; a shown card whose window's size changed is drawn at
    /// the new one.
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
        for &key in &shown {
            self.draw_card(key);
            self.hosting
                .natives
                .permissions
                .focus_pending
                .push((key, now));
            // A new call in the card in front: its wait starts again.
            self.wait_on_card_again(key);
        }
        // Where the docks gave a row back as a card was drawn, it is drawn once more.
        let mut drawn_again = 0;
        while drawn_again < SETTLES {
            let sized: Vec<SessionKey> = (self.hosting.natives.permissions.drawn.iter())
                .filter(|&&(key, size)| self.card_size(key).is_some_and(|now| now != size))
                .map(|&(key, _)| key)
                .collect();
            if sized.is_empty() {
                break;
            }
            drawn_again += 1;
            for &key in &sized {
                self.draw_card(key);
                self.wait_on_card_again(key);
            }
        }
        if drawn_again == SETTLES {
            log::debug!("native sessions: a card's size had not settled");
        }
        for &key in &shown {
            self.focus_card_at_once(key);
        }
        self.note_cards_on_screen();
        // A frame only for a card just drawn.
        if !shown.is_empty() || drawn_again > 0 {
            self.request_redraw();
        }
    }

    /// Writes session `key`'s card as its request's question has it, and docks it.
    fn draw_card(&mut self, key: SessionKey) {
        let drawn = &self.hosting.natives.permissions.drawn;
        let before = drawn
            .iter()
            .find_map(|&(of, size)| (of == key).then_some(size));
        let size = (self.card_size(key).or(before)).unwrap_or(size::UNSHOWN);
        let permissions = &mut self.hosting.natives.permissions;
        let Some(shown) = (permissions.asks.shown_mut(key))
            .and_then(|ask| ask.question.as_mut())
            .map(|question| question.shown(size.0, size.1))
        else {
            return;
        };
        permissions.drawn.retain(|&(drawn, _)| drawn != key);
        permissions.drawn.push((key, size));
        let Some(id) = self.card_buffer(key) else {
            return;
        };
        self.write_card(id, &shown);
        if let Some(native) = self.hosting.natives.map.get(&key) {
            self.editor
                .set_dock(native.buffer, DockSlot::Card, Some(id));
        }
    }

    /// Session `key`'s card's buffer, made the first time.
    fn card_buffer(&mut self, key: SessionKey) -> Option<BufferId> {
        let native = self.hosting.natives.map.get(&key)?;
        if let Some(id) = native.card.filter(|&id| self.editor.buffer(id).is_some()) {
            return Some(id);
        }
        let name = self.hosting.table.get(key).map_or("", |s| s.name.as_str());
        let id = self
            .editor
            .add_internal_buffer(format!("*card: {name}*"), Mode::Card);
        if let Some(buffer) = self.editor.buffer_mut(id) {
            buffer.withhold_from_agents();
        }
        self.hosting.natives.map.get_mut(&key)?.card = Some(id);
        Some(id)
    }

    /// Puts `shown` in card `id`, with its faces.
    fn write_card(&mut self, id: BufferId, shown: &Shown) {
        self.editor.set_internal_text(id, &shown.text);
        // One list of runs, the line feeds between rows in no face.
        let mut runs = Vec::with_capacity(2 * shown.faces.len());
        let mut at = 0;
        for (range, face) in &shown.faces {
            if range.start > at {
                runs.push((range.start - at, None));
            }
            runs.push((range.len(), Some(face.highlight())));
            at = range.end;
        }
        if at < shown.text.len() {
            runs.push((shown.text.len() - at, None));
        }
        let mut highlights = Highlights::new(shown.text.len());
        highlights.replace(0..shown.text.len(), runs);
        let _ = self.editor.set_internal_highlights(id, highlights);
    }

    /// Session `key`'s card goes, as its call was answered or went: point, if it was there,
    /// goes to the composer.
    pub(in crate::app::sessions::hosting::native) fn close_card(&mut self, key: SessionKey) {
        let permissions = &mut self.hosting.natives.permissions;
        permissions
            .focus_pending
            .retain(|&(pending, _)| pending != key);
        permissions.drawn.retain(|&(drawn, _)| drawn != key);
        if matches!(permissions.writing, Some(Writing::Note(k) | Writing::Own(k)) if k == key) {
            permissions.writing = None;
        }
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return self.plan_card_closed(key);
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
        self.plan_card_closed(key);
        self.request_redraw();
    }

    /// A key of session `key`'s card, pressed in buffer `from`: `action`. Whether it was one
    /// of the card's.
    pub(in crate::app::sessions::hosting::native) fn card_key(
        &mut self,
        key: SessionKey,
        from: BufferId,
        action: NativeAction,
    ) -> bool {
        // From the composer, only what its keys may ask (ADR 0180).
        if Some(from) != (self.hosting.natives.map.get(&key)).and_then(|native| native.card)
            && !composer::takes_action(action)
        {
            return false;
        }
        let answers = matches!(
            action,
            NativeAction::Allow
                | NativeAction::Deny
                | NativeAction::Note
                | NativeAction::Choose(_)
                | NativeAction::Other
                | NativeAction::Page(_)
                | NativeAction::Open
                | NativeAction::EditPlan
                | NativeAction::Approve(_)
        );
        if !answers {
            return false;
        }
        let Some((kind, multi)) = self.card_kind(key) else {
            self.editor.message("No call waits for you here");
            return true;
        };
        // A page is no answer.
        if let NativeAction::Page(pages) = action {
            self.turn_card(key, pages);
            return true;
        }
        if !self.card_admits_answer(key, from) {
            return true;
        }
        let questions = kind == Kind::Questions;
        match action {
            NativeAction::Allow if questions => {
                self.editor
                    .message("Choose an option, or o to answer in your words");
            }
            NativeAction::Allow if kind == Kind::Plan => {
                self.approve_plan_card(key, Approval::Manual)
            }
            NativeAction::Approve(approval) if kind == Kind::Plan => {
                self.approve_plan_card(key, approval);
            }
            NativeAction::Allow => self.answer_card(key, Answer::Allow),
            NativeAction::Approve(_) => self.editor.message("This card holds no plan: y allows it"),
            NativeAction::EditPlan if kind == Kind::Plan => self.edit_plan(key),
            NativeAction::EditPlan => self.editor.message("This card holds no plan"),
            NativeAction::Deny => {
                let why = match kind {
                    Kind::Questions => "The person declined to answer in oso.",
                    Kind::Plan => "The person wants you to keep planning, in oso.",
                    Kind::Call => DENIED,
                };
                self.answer_card(key, Answer::Deny(why.to_owned(), BY_THE_PERSON.to_owned()));
            }
            NativeAction::Note => self.write_for_card(Writing::Note(key)),
            NativeAction::Other if questions => self.write_for_card(Writing::Own(key)),
            NativeAction::Other => self.editor.message("This card asks no question"),
            NativeAction::Choose(number) if questions && self.turned_to(key, number) => {}
            NativeAction::Choose(number) if questions => {
                self.step_questions(key, |questions| questions.choose(number));
            }
            NativeAction::Choose(_) => self.editor.message("This card has no options"),
            // A question that takes several is answered with those chosen; else the whole call.
            NativeAction::Open if multi => self.step_questions(key, Questions::confirm),
            NativeAction::Open if questions => {
                self.editor
                    .message("Type an option's number, or o to answer in your words");
            }
            _ => self.toggle_whole(key),
        }
        true
    }

    /// What kind of card session `key` shows, if it shows one, and whether its question takes
    /// several options.
    fn card_kind(&self, key: SessionKey) -> Option<(Kind, bool)> {
        let ask = self.hosting.natives.permissions.asks.shown(key)?;
        Some(match &ask.question.as_ref()?.card {
            Card::Questions(questions) => (
                Kind::Questions,
                questions.current().is_some_and(|asked| asked.multi),
            ),
            Card::Plan { .. } => (Kind::Plan, false),
            Card::Call { .. } => (Kind::Call, false),
        })
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
                self.answer_card(key, Answer::Deny(said, note));
            }
            Writing::Own(key) => self.step_questions(key, |questions| questions.own(text)),
        }
    }

    /// Carries `step` out on session `key`'s questions: the card shows the next, or every
    /// answer goes.
    fn step_questions(&mut self, key: SessionKey, step: impl FnOnce(&mut Questions) -> Step) {
        let asks = &mut self.hosting.natives.permissions.asks;
        let Some(question) = asks.shown_mut(key).and_then(|ask| ask.question.as_mut()) else {
            return;
        };
        let Card::Questions(questions) = &mut question.card else {
            return;
        };
        let step = step(questions);
        let answers = match step {
            Step::Done => questions.answers(),
            _ => None,
        };
        question.stepped(step);
        match step {
            Step::Done => {
                self.answer_card(key, Answer::Answers(answers.unwrap_or_default()));
            }
            Step::Next | Step::Toggled => {
                self.draw_card(key);
                self.request_redraw();
            }
            Step::Nothing => self.editor.message("No such option"),
        }
    }

    /// Answers session `key`'s shown request as `answer` says: its bridge is told, its call's
    /// row says a denial, and its card goes.
    fn answer_card(&mut self, key: SessionKey, answer: Answer) {
        let Some(ask) = self.hosting.natives.permissions.asks.take_shown(key) else {
            return;
        };
        let (reply, mark, said) = match answer {
            Answer::Allow => (permission::allow(), None, "allowed"),
            Answer::Answers(answers) => (permission::allow_answering(&answers), None, "answered"),
            Answer::Plan(edit, approval) => {
                let (reply, mark, said) = plan::answered(edit.as_deref(), approval);
                (reply, Some(mark), said)
            }
            Answer::Deny(why, row) => (permission::deny(&why), Some(Mark::Denied(row)), "denied"),
        };
        self.agent_log(&format!(
            "a native session's {}: {said}",
            ask.call.tool_name
        ));
        let id = &ask.call.tool_use_id;
        // A plan approved with a mode is the person's choice of it (part 3b), once the bridge
        // took the answer.
        let chosen = match &mark {
            Some(Mark::Approved(_, after)) => plan::mode_after(*after),
            _ => None,
        };
        self.call_waits(key, id, false);
        if let (Some(mark), Some(native)) = (mark, self.hosting.natives.map.get_mut(&key)) {
            let change = match mark {
                Mark::Denied(row) => native.rows.denied(id, &row),
                Mark::Approved(edited, after) => native.rows.approved(id, edited, after),
            };
            super::super::draw(&mut self.editor, native, change);
        }
        self.tell_tracker(key, Target::Working);
        let sent = ask.reply.send(Ok(reply)).is_ok();
        if let (true, Some(mode), Some(native)) =
            (sent, chosen, self.hosting.natives.map.get_mut(&key))
        {
            native.mode.chose(mode);
        }
        self.close_card(key);
    }

    /// `RET` on a card: it shows the call's whole input, or the whole plan, or goes back. What
    /// it shows is made the first time.
    fn toggle_whole(&mut self, key: SessionKey) {
        let name = self
            .hosting
            .table
            .get(key)
            .map_or_else(String::new, |s| s.name.clone());
        let asks = &mut self.hosting.natives.permissions.asks;
        let Some(ask) = asks.shown_mut(key) else {
            return;
        };
        let (tool, input) = (&ask.call.tool_name, ask.call.input.as_ref());
        if let Some(question) = ask.question.as_mut() {
            if question.details.is_none() {
                let details = card::call::details(&name, &question.card, tool, input);
                let shown: Vec<String> = details
                    .lines()
                    .map(|line| printable(line, details.len()))
                    .collect();
                question.details = Some(shown.join("\n"));
            }
            question.whole = !question.whole;
        }
        self.draw_card(key);
        self.request_redraw();
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

/// What kind of card a key acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Call,
    Questions,
    Plan,
}
