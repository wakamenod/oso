//! A native session's permission card (ADR 0165, step 2e-2, the author's (a) on Q1 to Q4;
//! ADR 0170's `Card` dock): what a call that waits for the person shows, as text and faces, and
//! the answers of an `AskUserQuestion` as the person gives them. `call` makes the card of a
//! call from what it says.
//!
//! Must not depend on: threads, the stream or the UI; the caller gives what the call says, the
//! card's width and the keys' meaning, and docks the text.
//!
//! - oso's words are the card's first row, its title, and its last, the keys, the top and the
//!   bottom of its frame; what Claude
//!   wrote (a command, a diff, a reason, a question, its options, a plan) is fenced between
//!   them, each row of it after the frame's left side, `│ `, in a face of its own, so that none
//!   of it can stand as a key row or pass for oso's own words (ADR 0170). The frame and the
//!   title are in [`Face::CardTitle`] and the keys' chips in [`Face::CardKey`], which oso's own
//!   spans alone take, and what is drawn behind a chip comes from that face, never from what a
//!   text holds (ADR 0180). Each of Claude's line breaks starts a row of
//!   its own, and a row longer than the card is broken into rows at its width, measured as
//!   the display draws it, the rows it goes on to fenced `┊ `: no text of Claude's runs past
//!   the card's edge or wraps unfenced, and a row is one row on screen. A mark that would
//!   start a row goes on `◌`, as it would join the fence.
//! - A call's card is at most [`MAX_ROWS`] rows; what is left out says so, and `RET` shows all
//!   of it in the card, at most [`MAX_WHOLE`] rows, with the keys on its title row. A question's
//!   and a plan's grow to what they say, up to the rows the caller has, and page past that
//!   (`body`): an option may be chosen only once it has been on screen (ADR 0180).
//! - A question's answer is an option's label, or several joined by `, ` (measured, CLI
//!   2.1.330), or the person's own words.

mod body;
pub mod call;
mod frame;
mod rows;

use self::body::{Body, Page};
pub use self::call::question;
pub use self::frame::Shown;
use self::frame::{BOTTOM, Key, MIN_WIDTH, TOP, chips, room_in};
use self::rows::{Break, Row, rows_of};
use super::render::Face;
use super::wire::card::Asked;

/// The most rows a call's card has: its title, what it fences and its keys (ADR 0170).
pub const MAX_ROWS: usize = 12;
/// The most rows of Claude's text a call's card fences, with its keys on one row.
#[cfg(test)]
const MAX_FENCED: usize = MAX_ROWS - 2;
/// The most rows `RET`'s whole view fences.
pub const MAX_WHOLE: usize = 2000;
/// The narrowest window a card is read in, in cells: a narrower one cuts each of its rows.
pub const NARROWEST: usize = MIN_WIDTH;

/// What the person is asked about a call: its card, what `RET` shows of it whole, once
/// asked, whether the card shows that now, and where a question's or a plan's page is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub card: Card,
    pub details: Option<String>,
    pub whole: bool,
    page: Box<Page>,
}

impl Question {
    /// What `card` asks, shown from its start.
    pub fn new(card: Card) -> Self {
        Self {
            card,
            details: None,
            whole: false,
            page: Box::default(),
        }
    }

    /// Whether the card grows to what it says, and pages past that: a question or a plan.
    pub fn grows(&self) -> bool {
        !matches!(self.card, Card::Call { .. }) && !self.whole
    }

    /// The card as text `width` cells wide and, if it [grows](Self::grows), at most `room` rows
    /// tall.
    pub fn shown(&mut self, width: usize, room: usize) -> Shown {
        if let (true, Some(details)) = (self.whole, &self.details) {
            return whole(&self.card, details, width);
        }
        let keys = self.card.keys();
        match &self.card {
            Card::Call { title, lines } => call_shown(title, lines, keys, width),
            Card::Questions(questions) => {
                (self.page).shown((width, room), keys, || Body::question(questions, width))
            }
            Card::Plan { plan, cut } => {
                (self.page).shown((width, room), keys, || Body::plan(plan, *cut, width))
            }
        }
    }

    /// Turns `pages` pages on, back if negative. Whether the page changed.
    pub fn turn(&mut self, pages: isize) -> bool {
        self.grows() && self.page.turn(pages)
    }

    /// The card as drawn last is on screen.
    pub fn on_screen(&mut self) {
        self.page.on_screen();
    }

    /// Whether option `number` of the question, from 1, has been on screen: only then may the
    /// person choose it.
    pub fn has_shown(&self, number: usize) -> bool {
        self.page.has_shown(number)
    }

    /// Whether the question offers option `number`, from 1.
    pub fn offers(&self, number: usize) -> bool {
        match &self.card {
            Card::Questions(questions) => (questions.current())
                .is_some_and(|asked| (1..=asked.options.len()).contains(&number)),
            Card::Call { .. } | Card::Plan { .. } => false,
        }
    }

    /// Turns to option `number`, from 1.
    pub fn go_to(&mut self, number: usize) {
        self.page.go_to(number);
    }

    /// The question went through `step`: the next one starts at its first page.
    pub fn stepped(&mut self, step: Step) {
        match step {
            Step::Next | Step::Done => *self.page = Page::default(),
            Step::Toggled => {
                if let Card::Questions(questions) = &self.card {
                    self.page.remark(questions);
                }
            }
            Step::Nothing => {}
        }
    }
}

/// What a card asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Card {
    /// May a call run: `title`, such as `Bash wants to run`, then Claude's `lines`, each with
    /// its face.
    Call {
        title: String,
        lines: Vec<(String, Face)>,
    },
    /// Questions to answer.
    Questions(Questions),
    /// A plan to approve, and whether the request cut it.
    Plan { plan: String, cut: bool },
}

impl Card {
    /// Its keys, the card's last row.
    fn keys(&self) -> &'static [Key] {
        match self {
            Self::Call { .. } => &[
                ("y", "allow once", "allow"),
                ("n", "deny", "deny"),
                ("N", "deny with a note", "note"),
                ("RET", "the whole call", "all"),
            ],
            Self::Questions(questions) if questions.current().is_some_and(|asked| asked.multi) => {
                &[
                    ("1-9", "choose or unchoose", "choose"),
                    ("RET", "done", "done"),
                    ("o", "in your words", "own"),
                    ("n", "decline", "decline"),
                ]
            }
            Self::Questions(_) => &[
                ("1-9", "choose", "choose"),
                ("o", "in your words", "own"),
                ("n", "decline", "decline"),
            ],
            // `a` and `A` approve too, with that mode after it (part 3d).
            Self::Plan { .. } => &[
                ("y", "approve", "approve"),
                ("a", "accept edits", "edits"),
                ("A", "auto", "auto"),
                ("e", "edit", "edit"),
                ("n", "no", "no"),
                ("N", "note", "note"),
                ("RET", "all", "all"),
            ],
        }
    }
}

/// A call's card as text `width` cells wide: its `title`, Claude's `lines` fenced, at most
/// [`MAX_ROWS`] rows with them, and its `keys`, the title and the keys the frame's top and bottom.
fn call_shown(title: &str, lines: &[(String, Face)], keys: &[Key], width: usize) -> Shown {
    let mut shown = Shown::default();
    // Cut to the card: what the title says first, as a warning, stays in sight.
    shown.edge(TOP, &[(title.to_owned(), Face::CardTitle)], width);
    let room = room_in(width);
    let keys = chips("", keys, width);
    // A second row of keys takes one of Claude's.
    let most = MAX_ROWS.saturating_sub(1 + keys.len()).max(1);
    let mut rows = Vec::new();
    let mut more = false;
    for (line, face) in lines {
        let (broken, left) = rows_of(line, room, Break::Exact, most + 1 - rows.len());
        rows.extend(broken.into_iter().map(|row| (row, *face)));
        if left || rows.len() > most {
            more = true;
            break;
        }
    }
    if more {
        rows.truncate(most - 1);
    }
    for (row, face) in &rows {
        shown.fenced_row(row, *face, width);
    }
    if more {
        let said = Row::said("… more rows: RET shows all", room);
        shown.fenced_row(&said, Face::Dim, width);
    }
    shown.bottom(&keys, width);
    shown
}

/// `card` shown whole, `details`, as `RET` asks, `width` cells wide: its keys on the title row,
/// as the rows below may be many, then every row of it fenced, at most [`MAX_WHOLE`], scrolled
/// as any buffer is, and the frame's bottom; `RET` goes back to the card.
fn whole(card: &Card, details: &str, width: usize) -> Shown {
    let mut shown = Shown::default();
    let before_ret = (card.keys().iter().copied()).take_while(|(key, ..)| *key != "RET");
    let keys: Vec<Key> = before_ret.chain([("RET", "back", "back")]).collect();
    shown.top(&chips("All of it:", &keys, width), width);
    let (rows, more) = rows_of(details, room_in(width), Break::Exact, MAX_WHOLE);
    for row in &rows {
        shown.fenced_row(row, Face::Target, width);
    }
    if more {
        let said = Row::said("… the rest is not shown here", room_in(width));
        shown.fenced_row(&said, Face::Dim, width);
    }
    shown.edge(BOTTOM, &[], width);
    shown
}

/// An `AskUserQuestion`'s questions, answered one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Questions {
    asked: Vec<Asked>,
    /// The question answered now.
    at: usize,
    /// Each question's answer, once given.
    answers: Vec<Option<String>>,
    /// The options chosen of a question that takes several, by their index.
    picked: Vec<usize>,
}

/// What a key in a question's card came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The next question is asked.
    Next,
    /// Every question is answered: send them.
    Done,
    /// An option of a question that takes several was chosen or unchosen.
    Toggled,
    /// Nothing changed: no such option, or none chosen yet.
    Nothing,
}

impl Questions {
    /// `asked`, none of it answered.
    pub fn new(asked: Vec<Asked>) -> Self {
        let answers = vec![None; asked.len()];
        Self {
            asked,
            at: 0,
            answers,
            picked: Vec::new(),
        }
    }

    /// The question answered now.
    pub fn current(&self) -> Option<&Asked> {
        self.asked.get(self.at)
    }

    /// The person chose option `number`, from 1.
    pub fn choose(&mut self, number: u8) -> Step {
        let Some(asked) = self.current() else {
            return Step::Nothing;
        };
        let Some(index) = usize::from(number).checked_sub(1) else {
            return Step::Nothing;
        };
        if index >= asked.options.len() {
            return Step::Nothing;
        }
        if asked.multi {
            match self.picked.iter().position(|&picked| picked == index) {
                Some(at) => {
                    self.picked.remove(at);
                }
                None => self.picked.push(index),
            }
            return Step::Toggled;
        }
        let label = asked.options[index].0.clone();
        self.answered(label)
    }

    /// `RET`: a question that takes several is answered with those chosen, in the options'
    /// order.
    pub fn confirm(&mut self) -> Step {
        let Some(asked) = self.current().filter(|asked| asked.multi) else {
            return Step::Nothing;
        };
        if self.picked.is_empty() {
            return Step::Nothing;
        }
        let mut picked = self.picked.clone();
        picked.sort_unstable();
        let labels: Vec<&str> = (picked.iter())
            .filter_map(|&index| asked.options.get(index).map(|(label, _)| label.as_str()))
            .collect();
        let answer = labels.join(", ");
        self.answered(answer)
    }

    /// The person answered the question in their own words.
    pub fn own(&mut self, text: &str) -> Step {
        let text = text.trim();
        if text.is_empty() || self.current().is_none() {
            return Step::Nothing;
        }
        self.answered(text.to_owned())
    }

    fn answered(&mut self, answer: String) -> Step {
        if let Some(slot) = self.answers.get_mut(self.at) {
            *slot = Some(answer);
        }
        self.picked.clear();
        self.at += 1;
        match self.at < self.asked.len() {
            true => Step::Next,
            false => Step::Done,
        }
    }

    /// Each question and its answer, once all are answered.
    pub fn answers(&self) -> Option<Vec<(String, String)>> {
        (self.asked.iter().zip(&self.answers))
            .map(|(asked, answer)| Some((asked.question.clone(), answer.clone()?)))
            .collect()
    }

    /// The card's title: which question of how many is answered now.
    fn title(&self) -> String {
        match self.asked.len() {
            0 | 1 => "Claude asks".to_owned(),
            count => format!("Claude asks {}/{count}", (self.at + 1).min(count)),
        }
    }

    /// Whether the option at `index` is chosen, of a question that takes several.
    fn picks(&self, index: usize) -> bool {
        self.picked.contains(&index)
    }
}

#[cfg(test)]
mod grow_tests;
#[cfg(test)]
mod tests;
