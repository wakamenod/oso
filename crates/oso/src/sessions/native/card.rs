//! A native session's permission card (ADR 0165, step 2e-2, the author's (a) on Q1 to Q4;
//! ADR 0170's `Card` dock): what a call that waits for the person shows, as text and faces, and
//! the answers of an `AskUserQuestion` as the person gives them. `call` makes the card of a
//! call from what it says.
//!
//! Must not depend on: threads, the stream or the UI; the caller gives what the call says, the
//! card's width and the keys' meaning, and docks the text.
//!
//! - oso's words are the card's first row, its title, and its last, the keys; what Claude
//!   wrote (a command, a diff, a reason, a question, its options, a plan) is fenced between
//!   them, each row of it after `│ ` in a face of its own, so that none of it can stand as a key
//!   row or pass for oso's own words (ADR 0170). Each of Claude's line breaks starts a row of
//!   its own, and a row longer than the card is broken into rows at its width, measured as
//!   the display draws it, the rows it goes on to fenced `┊ `: no text of Claude's runs past
//!   the card's edge or wraps unfenced, and a row is one row on screen. A mark that would
//!   start a row goes on `◌`, as it would join the fence.
//! - The card is at most [`MAX_ROWS`] rows; what is left out says so, and `RET` shows all of
//!   it in the card ([`whole`]), at most [`MAX_WHOLE`] rows, with the keys on its title row.
//! - A question's answer is an option's label, or several joined by `, ` (measured, CLI
//!   2.1.330), or the person's own words.

pub mod call;

use std::ops::Range;

use oso_core::minibuffer::cells_of;

pub use self::call::question;
use super::render::Face;
use super::wire::card::Asked;

/// The most rows a card has: its title, what it fences and its keys (ADR 0170).
pub const MAX_ROWS: usize = 12;
/// The most rows of Claude's text a card fences.
pub const MAX_FENCED: usize = MAX_ROWS - 2;
/// The most rows `RET`'s whole view fences.
pub const MAX_WHOLE: usize = 2000;
/// The fence before each row of Claude's text.
pub const FENCE: &str = "│ ";
/// The fence before a row that goes on with the row above, broken at the card's width.
pub const FENCE_ON: &str = "┊ ";
/// The narrowest card drawn, in cells.
const MIN_WIDTH: usize = 20;

/// What the person is asked about a call: its card, what `RET` shows of it whole, once
/// asked, and whether the card shows that now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub card: Card,
    pub details: Option<String>,
    pub whole: bool,
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

/// A card as text, with faces over byte ranges of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shown {
    pub text: String,
    pub faces: Vec<(Range<usize>, Face)>,
    /// How many rows it has.
    pub rows: usize,
}

impl Shown {
    fn row(&mut self, text: &str, face: Face) {
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        let start = self.text.len();
        self.text.push_str(text);
        self.faces.push((start..self.text.len(), face));
        self.rows += 1;
    }

    /// One fenced row: the fence, `│ ` or `┊ ` for one that goes on with the row above, then
    /// `text`, which holds no line break.
    fn fenced_row(&mut self, (text, on): (&str, bool), face: Face) {
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        let start = self.text.len();
        self.text.push_str(if on { FENCE_ON } else { FENCE });
        self.faces.push((start..self.text.len(), Face::Dim));
        let start = self.text.len();
        self.text.push_str(text);
        self.faces.push((start..self.text.len(), face));
        self.rows += 1;
    }
}

/// Claude's `text` as the card's rows of at most `width` cells after the fence, each with
/// whether it goes on with the row above: a row at each line feed, a tab as a space, and a
/// longer row broken at the width, measured as the display draws it (a mark past what one
/// cluster holds is drawn boxed, wider). At most `most` rows, and whether more were left out.
fn rows_of(text: &str, width: usize, most: usize) -> (Vec<(String, bool)>, bool) {
    let room = width.max(MIN_WIDTH).saturating_sub(cells_of(FENCE)).max(1);
    let mut rows = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let (mut row, mut used, mut on) = (String::new(), 0, false);
        for c in line.chars() {
            let c = if c == '\t' { ' ' } else { c };
            let before = row.len();
            row.push(c);
            // ASCII that prints is one cell; anything else is measured with what it joins.
            let now = if c.is_ascii_graphic() || c == ' ' {
                used + 1
            } else {
                cells_of(&row)
            };
            if now > room && before > 0 {
                row.truncate(before);
                rows.push((std::mem::take(&mut row), on));
                if rows.len() >= most {
                    return (rows, true);
                }
                // A mark that starts a row would join the fence: it goes on a base of its own.
                if joins(c) {
                    row.push(BASE);
                }
                row.push(c);
                used = cells_of(&row);
                on = true;
            } else {
                used = now;
            }
        }
        if rows.len() >= most {
            return (rows, true);
        }
        rows.push((row, on));
    }
    (rows, false)
}

impl Card {
    /// Its keys, the card's last row.
    fn keys(&self) -> &'static str {
        match self {
            Self::Call { .. } => {
                "[y] allow once  [n] deny  [N] deny with a note  [RET] the whole call"
            }
            Self::Questions(questions) if questions.current().is_some_and(|asked| asked.multi) => {
                "[1-9] choose or unchoose  [RET] done  [o] in your words  [n] decline"
            }
            Self::Questions(_) => "[1-9] choose  [o] in your words  [n] decline  [RET] all of it",
            Self::Plan { .. } => {
                "[y] approve  [n] keep planning  [N] with a note  [RET] the whole plan"
            }
        }
    }

    /// The card as text `width` cells wide: its title, Claude's rows fenced, at most
    /// [`MAX_FENCED`] of them, and its keys.
    pub fn shown(&self, width: usize) -> Shown {
        let mut shown = Shown::default();
        let (title, lines) = match self {
            Self::Call { title, lines } => (title.clone(), lines.clone()),
            Self::Questions(questions) => questions.shown(),
            Self::Plan { plan, cut } => {
                let mut lines = vec![(plan.clone(), Face::Target)];
                if *cut {
                    lines.push(("(the request cut it)".to_owned(), Face::Dim));
                }
                ("Claude's plan, as its request gives it".to_owned(), lines)
            }
        };
        // Cut to the card: what the title says first, as a warning, stays in sight.
        shown.row(&title_of(&title, width), Face::Tool);
        let mut rows = Vec::new();
        let mut more = false;
        for (line, face) in &lines {
            let (broken, left) = rows_of(line, width, MAX_FENCED + 1 - rows.len());
            rows.extend(broken.into_iter().map(|row| (row, *face)));
            if left || rows.len() > MAX_FENCED {
                more = true;
                break;
            }
        }
        if more {
            rows.truncate(MAX_FENCED - 1);
        }
        for ((row, on), face) in &rows {
            shown.fenced_row((row, *on), *face);
        }
        if more {
            shown.fenced_row(("… more rows: RET shows all", false), Face::Dim);
        }
        shown.row(self.keys(), Face::Prompt);
        shown
    }
}

/// `card` shown whole, `details`, as `RET` asks, `width` cells wide: its keys on the title row,
/// as the rows below may be many, then every row of it fenced, at most [`MAX_WHOLE`], scrolled
/// as any buffer is; `RET` goes back to the card.
pub fn whole(card: &Card, details: &str, width: usize) -> Shown {
    let mut shown = Shown::default();
    let keys = card.keys();
    let keys = match keys.find("[RET]") {
        Some(at) => format!("All of it:  {}[RET] back", &keys[..at]),
        None => format!("All of it:  {keys}  [RET] back"),
    };
    shown.row(&title_of(&keys, width), Face::Prompt);
    let (rows, more) = rows_of(details, width, MAX_WHOLE);
    for (row, on) in &rows {
        shown.fenced_row((row, *on), Face::Target);
    }
    if more {
        shown.fenced_row(("… the rest is not shown here", false), Face::Dim);
    }
    shown
}

/// What a mark that starts a row goes on, as a lone mark is shown (`◌`).
const BASE: char = '\u{25cc}';

/// Whether `c` joins the character before it, as a combining mark does.
fn joins(c: char) -> bool {
    let mut joined = String::from("a");
    joined.push(c);
    !c.is_ascii() && cells_of(&joined) <= 1
}

/// `title` cut to `width` cells, with `…` where it was cut.
fn title_of(title: &str, width: usize) -> String {
    let (kept, cut) = oso_core::minibuffer::cut_to(title, width.max(MIN_WIDTH));
    format!("{kept}{cut}")
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

    /// The card's title and Claude's lines for the question answered now.
    fn shown(&self) -> (String, Vec<(String, Face)>) {
        let Some(asked) = self.current() else {
            return ("Claude asks".to_owned(), Vec::new());
        };
        let count = self.asked.len();
        let title = match count {
            1 => "Claude asks".to_owned(),
            _ => format!("Claude asks ({} of {count})", self.at + 1),
        };
        let mut lines = Vec::new();
        if !asked.header.is_empty() {
            lines.push((asked.header.clone(), Face::Dim));
        }
        lines.push((asked.question.clone(), Face::Target));
        for (index, (label, description)) in asked.options.iter().enumerate() {
            let mark = match (asked.multi, self.picked.contains(&index)) {
                (true, true) => "[x] ",
                (true, false) => "[ ] ",
                (false, _) => "",
            };
            let line = match description.is_empty() {
                true => format!("{}. {mark}{label}", index + 1),
                false => format!("{}. {mark}{label}: {description}", index + 1),
            };
            lines.push((line, Face::Target));
        }
        (title, lines)
    }
}

#[cfg(test)]
mod tests;
