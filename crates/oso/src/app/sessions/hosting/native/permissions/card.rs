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
//! - Point moves to it after [`QUIET`] with no key, while the person is in that session's
//!   transcript or its composer, and the composer holds no draft.
//! - Its answers (`y`, `n`, `N`, a digit, `o`, `RET`, and a plan's `a`, `A`, `e`) count only
//!   after [`QUIET`] with no key and since the card became the selected buffer, however point
//!   came; never from a macro, and never while the card's window shows less than all of it. Other keys, as `C-c C-o` or
//!   `C-x o` to leave it, are any buffer's.
//! - It is drawn again when its window's width changes, as its rows are broken at it.
//! - A plan's card approves with a mode after it, or edits the plan first: `plan` (part 3d).

mod plan;

pub(super) use plan::Plans;

use std::time::{Duration, Instant};

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::{Approval, NativeAction};
use oso_core::syntax::Highlights;
use oso_core::window::DockSlot;

use super::DENIED;
use crate::app::App;
use crate::app::agent::QUIET;
use crate::channel::protocol::permission;
use crate::commands::agent_sessions::{CARD_NOTE, CARD_OWN};
use crate::git::printable;
use crate::sessions::native::card::{self, Card, Questions, Shown, Step};
use crate::sessions::state::Target;
use crate::sessions::worker::SessionKey;

/// The width a card is drawn at when no window shows it yet, in cells.
const UNSHOWN_WIDTH: usize = 80;
/// How much longer a card waits once a prompt an agent opened over it closed.
const AFTER_AGENT: Duration = Duration::from_millis(1500);
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

/// What the card in front is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app::sessions::hosting::native) struct Current {
    card: BufferId,
    since: Instant,
}

impl App {
    /// After each sync: each request found whose card is not shown gets its card, docked
    /// under its session's transcript; a shown card whose window's width changed is drawn at
    /// the new width.
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
            let card = (self.hosting.natives.map.get(&key)).and_then(|native| native.card);
            let current = self.hosting.natives.permissions.current.as_mut();
            if let Some(current) = current.filter(|current| Some(current.card) == card) {
                current.since = now;
            }
        }
        let resized: Vec<SessionKey> = (self.hosting.natives.permissions.drawn.iter())
            .filter(|&&(key, width)| self.card_width(key) != width)
            .map(|&(key, _)| key)
            .collect();
        for &key in &resized {
            self.draw_card(key);
        }
        // A frame only for a card just drawn.
        if !shown.is_empty() || !resized.is_empty() {
            self.request_redraw();
        }
    }

    /// The width session `key`'s card is drawn at: its window's, else its transcript's.
    fn card_width(&self, key: SessionKey) -> usize {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return UNSHOWN_WIDTH;
        };
        let window = (self.editor.dock_window(native.buffer, DockSlot::Card)).or_else(|| {
            (self.editor.windows())
                .find(|window| window.buffer() == native.buffer)
                .map(|window| window.id())
        });
        window
            .and_then(|window| self.editor.regions(window))
            .map_or(UNSHOWN_WIDTH, |regions| regions.text.columns)
    }

    /// Writes session `key`'s card as its request's question has it, and docks it.
    fn draw_card(&mut self, key: SessionKey) {
        let width = self.card_width(key);
        let Some(shown) = (self.hosting.natives.permissions.asks.shown(key))
            .and_then(|ask| ask.question.as_ref())
            .map(|question| match (&question.details, question.whole) {
                (Some(details), true) => card::whole(&question.card, details, width),
                _ => question.card.shown(width),
            })
        else {
            return;
        };
        let permissions = &mut self.hosting.natives.permissions;
        permissions.drawn.retain(|&(drawn, _)| drawn != key);
        permissions.drawn.push((key, width));
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
        let mut highlights = Highlights::new(shown.text.len());
        for (range, face) in &shown.faces {
            highlights.replace(range.clone(), [(range.len(), Some(face.highlight()))]);
        }
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

    /// A key of session `key`'s card: `action`. Whether it was one of the card's.
    pub(in crate::app::sessions::hosting::native) fn card_key(
        &mut self,
        key: SessionKey,
        action: NativeAction,
    ) -> bool {
        let answers = matches!(
            action,
            NativeAction::Allow
                | NativeAction::Deny
                | NativeAction::Note
                | NativeAction::Choose(_)
                | NativeAction::Other
                | NativeAction::Open
                | NativeAction::EditPlan
                | NativeAction::Approve(_)
        );
        if !answers {
            return false;
        }
        let kind = (self.hosting.natives.permissions.asks.shown(key))
            .and_then(|ask| ask.question.as_ref())
            .map(|question| {
                let multi = matches!(&question.card, Card::Questions(q) if q.current().is_some_and(|a| a.multi));
                let kind = match &question.card {
                    Card::Questions(_) => Kind::Questions,
                    Card::Plan { .. } => Kind::Plan,
                    Card::Call { .. } => Kind::Call,
                };
                (kind, multi && !question.whole, question.whole)
            });
        let Some((kind, multi, whole)) = kind else {
            self.editor.message("No call waits for you here");
            return true;
        };
        if !self.card_takes_answers(key) {
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
            NativeAction::Choose(_) if whole => {
                self.editor.message("RET goes back to the card's options");
            }
            NativeAction::Choose(number) if questions => {
                self.step_questions(key, |questions| questions.choose(number));
            }
            NativeAction::Choose(_) => self.editor.message("This card has no options"),
            // A question that takes several is answered with those chosen; else the whole call.
            NativeAction::Open if multi => self.step_questions(key, Questions::confirm),
            _ => self.toggle_whole(key),
        }
        true
    }

    /// Whether session `key`'s card takes an answer now; else the echo area says why.
    fn card_takes_answers(&mut self, key: SessionKey) -> bool {
        if self.editor.macro_replaying() {
            self.editor.message("A macro does not answer a card");
            return false;
        }
        let now = Instant::now();
        let card = (self.hosting.natives.map.get(&key)).and_then(|native| native.card);
        let current = self.hosting.natives.permissions.current;
        // Point came to it with this key: the wait starts now.
        if card.is_none_or(|card| current.is_none_or(|current| current.card != card)) {
            self.note_current_card();
            self.editor.message("Wait a moment, then answer the card");
            return false;
        }
        let since = current.is_some_and(|current| now >= current.since + QUIET);
        let quiet = self
            .agent_key_before()
            .is_none_or(|before| now >= before + QUIET);
        if !(since && quiet) {
            self.editor.message("Wait a moment, then answer the card");
            return false;
        }
        if !self.card_shows_all(key) {
            self.editor
                .message("The card is cut: make the window taller to answer it");
            return false;
        }
        true
    }

    /// Whether session `key`'s card's window shows every row of it, from its start, at the
    /// width it was drawn at, with no row wrapped.
    fn card_shows_all(&self, key: SessionKey) -> bool {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return false;
        };
        let Some(buffer) = native.card.and_then(|card| self.editor.buffer(card)) else {
            return false;
        };
        let Some(id) = self.editor.dock_window(native.buffer, DockSlot::Card) else {
            return false;
        };
        let (Some(window), Some(regions)) = (self.editor.window_by_id(id), self.editor.regions(id))
        else {
            return false;
        };
        let drawn = (self.hosting.natives.permissions.drawn.iter())
            .find_map(|&(drawn, width)| (drawn == key).then_some(width));
        if drawn.is_none_or(|width| regions.text.columns < width) || buffer.wraps() {
            return false;
        }
        let whole = (self.hosting.natives.permissions.asks.shown(key))
            .and_then(|ask| ask.question.as_ref())
            .is_some_and(|question| question.whole);
        // The whole view scrolls, its keys on its title row; the card shows from its start.
        let scroll = window.scroll();
        match whole {
            true => regions.text.rows > 0 && scroll.left == 0,
            false => {
                scroll.start == 0
                    && scroll.left == 0
                    && buffer.text().line_count() <= regions.text.rows
            }
        }
    }

    /// After each event: which card, if any, is the selected buffer, and since when.
    pub(in crate::app::sessions::hosting::native) fn note_current_card(&mut self) {
        let selected = self.editor.window().buffer();
        let is_card = self
            .editor
            .buffer(selected)
            .is_some_and(|b| b.mode() == Mode::Card);
        let agents = self
            .editor
            .prompt()
            .is_some_and(|prompt| prompt.from_agent());
        let card = (is_card && self.editor.prompt().is_none()).then_some(selected);
        let permissions = &mut self.hosting.natives.permissions;
        // A prompt an agent opened over a card may leave a key meant for it to the card: the
        // card waits longer once it closes.
        permissions.after_agent |= is_card && agents;
        match card {
            Some(card)
                if permissions
                    .current
                    .is_some_and(|current| current.card == card) => {}
            Some(card) => {
                let longer = std::mem::take(&mut permissions.after_agent);
                let since = Instant::now() + if longer { AFTER_AGENT } else { Duration::ZERO };
                permissions.current = Some(Current { card, since });
            }
            None => permissions.current = None,
        }
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
        question.whole = false;
        let Card::Questions(questions) = &mut question.card else {
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
        if let (Some(mark), Some(native)) = (mark, self.hosting.natives.map.get_mut(&key))
            && let Some(change) = match mark {
                Mark::Denied(row) => native.rows.denied(id, &row),
                Mark::Approved(edited, after) => native.rows.approved(id, edited, after),
            }
        {
            super::super::draw(&mut self.editor, native, [change]);
        }
        self.tell_tracker(key, Target::Working);
        let _ = ask.reply.send(Ok(reply));
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

    /// When point is next to move to a card, if one waits for it: [`QUIET`] after it was
    /// shown and after the person's last key, later than the last look.
    pub(in crate::app) fn card_focus_deadline(&self) -> Option<Instant> {
        let permissions = &self.hosting.natives.permissions;
        let typed = self.agent_person_typed();
        let looked = permissions.looked;
        (permissions.focus_pending.iter())
            .map(|&(_, shown)| typed.map_or(shown, |typed| typed.max(shown)) + QUIET)
            .filter(|&due| looked.is_none_or(|looked| due > looked))
            .min()
    }

    /// Before the loop waits: point goes to a card that waits for it, once [`QUIET`] passed
    /// with no key, while the person is in its session's transcript, or its composer with no
    /// draft in it.
    pub(in crate::app) fn card_focus_due(&mut self, now: Instant) {
        self.note_current_card();
        let permissions = &mut self.hosting.natives.permissions;
        if permissions.focus_pending.is_empty() {
            return;
        }
        permissions.looked = Some(now);
        if self
            .agent_person_typed()
            .is_some_and(|typed| now < typed + QUIET)
        {
            return;
        }
        let selected = self.editor.window().buffer();
        let in_session = (self.hosting.natives.map.iter()).find_map(|(key, native)| {
            let composer = native.composing.buffer == Some(selected);
            (selected == native.buffer || composer).then_some((*key, composer))
        });
        let Some((key, composer)) = in_session else {
            return;
        };
        let due = (self.hosting.natives.permissions.focus_pending.iter())
            .any(|&(pending, shown)| pending == key && now >= shown + QUIET);
        // A draft in the composer keeps point there: the card waits, and says so.
        if !due || (composer && !self.composer_empty(key)) {
            return;
        }
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        let Some(window) = self.editor.dock_window(native.buffer, DockSlot::Card) else {
            return;
        };
        if !self.card_shows_all(key) {
            return;
        }
        self.editor.select_window(window);
        self.note_current_card();
        (self.hosting.natives.permissions.focus_pending).retain(|&(pending, _)| pending != key);
        self.request_redraw();
    }

    /// Whether the selected card, if one is, takes an answer now: its quiet moment passed since
    /// point came to it and since the last key. For end-to-end flows.
    #[cfg(feature = "dev")]
    pub(in crate::app) fn card_takes_answer(&self) -> Option<bool> {
        let current = self.hosting.natives.permissions.current?;
        let now = Instant::now();
        let quiet = self
            .agent_person_typed()
            .is_none_or(|typed| now >= typed + QUIET);
        Some(now >= current.since + QUIET && quiet)
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
