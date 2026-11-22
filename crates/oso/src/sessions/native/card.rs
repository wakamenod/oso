//! A native session's permission card (ADR 0165, step 2e-2, the author's (a) on Q1 to Q4;
//! ADR 0170's `Card` dock): what a call that waits for the person shows, as text and faces, and
//! the answers of an `AskUserQuestion` as the person gives them.
//!
//! Must not depend on: threads, the stream or the UI; the caller gives what the call says and
//! the keys' meaning, and docks the text.
//!
//! - oso's words are the card's first row, its title, and its last, the keys; what Claude
//!   wrote (a command, a diff, a reason, a question, its options, a plan) is fenced between
//!   them, each line of it after `│ ` in a face of its own, cut to [`MAX_FENCED`] rows, so that
//!   none of it can stand as a key row or pass for oso's own words (ADR 0170). A line of
//!   Claude's never holds a line break of its own: each of its lines is a fenced row.
//! - The card is at most [`MAX_ROWS`] rows; what is left out says so, and `RET` shows all of
//!   it in the card, fenced the same way ([`whole`]).
//! - A question's answer is an option's label, or several joined by `, ` (measured, CLI
//!   2.1.330), or the person's own words.

use std::ops::Range;

use super::render::Face;
use super::wire::card::Asked;

/// The most rows a card has: its title, what it fences and its keys (ADR 0170).
pub const MAX_ROWS: usize = 12;
/// The most rows of Claude's text a card fences.
pub const MAX_FENCED: usize = MAX_ROWS - 2;
/// The fence before each row of Claude's text.
pub const FENCE: &str = "│ ";

/// What a card asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Card {
    /// May a call run: `title`, such as `Bash wants to run`, then Claude's `lines`, each a
    /// row with its face.
    Call {
        title: String,
        lines: Vec<(String, Face)>,
    },
    /// Questions to answer.
    Questions(Questions),
    /// A plan to approve: its lines, and whether the request cut it.
    Plan { plan: String, cut: bool },
}

/// A card as text, with faces over byte ranges of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shown {
    pub text: String,
    pub faces: Vec<(Range<usize>, Face)>,
}

impl Shown {
    fn row(&mut self, text: &str, face: Face) {
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        let start = self.text.len();
        self.text.push_str(text);
        self.faces.push((start..self.text.len(), face));
    }

    /// One row of Claude's: the fence, then the text, its line breaks and tabs shown as one
    /// space each so that it stays one row.
    fn fenced(&mut self, text: &str, face: Face) {
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        let start = self.text.len();
        self.text.push_str(FENCE);
        self.faces.push((start..self.text.len(), Face::Dim));
        let start = self.text.len();
        let flat = text.replace(['\n', '\r', '\t', '\u{2028}', '\u{2029}', '\u{85}'], " ");
        self.text.push_str(&flat);
        self.faces.push((start..self.text.len(), face));
    }
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

    /// The card as text: its title, Claude's lines fenced, at most [`MAX_FENCED`] of them, and
    /// its keys.
    pub fn shown(&self) -> Shown {
        let mut shown = Shown::default();
        let keys = self.keys();
        let (title, lines): (String, Vec<(String, Face)>) = match self {
            Self::Call { title, lines } => (title.clone(), lines.clone()),
            Self::Questions(questions) => questions.shown(),
            Self::Plan { plan, cut } => {
                let mut lines: Vec<(String, Face)> = plan
                    .lines()
                    .map(|line| (line.to_owned(), Face::Target))
                    .collect();
                if *cut {
                    lines.push(("(the request cut it)".to_owned(), Face::Dim));
                }
                ("Claude's plan, as its request gives it".to_owned(), lines)
            }
        };
        shown.row(&title, Face::Tool);
        let over = lines.len() > MAX_FENCED;
        let kept = if over { MAX_FENCED - 1 } else { lines.len() };
        for (line, face) in &lines[..kept] {
            shown.fenced(line, *face);
        }
        if over {
            let more = format!("… {} more rows: RET shows all", lines.len() - kept);
            shown.fenced(&more, Face::Dim);
        }
        shown.row(keys, Face::Prompt);
        shown
    }
}

/// `card` shown whole, `details`, as `RET` asks: every line of it fenced, none left out,
/// scrolled as any buffer is, under the same keys; `RET` goes back to the card.
pub fn whole(card: &Card, details: &str) -> Shown {
    let mut shown = Shown::default();
    shown.row("All of it (RET goes back)", Face::Tool);
    for line in details.lines() {
        shown.fenced(line, Face::Target);
    }
    let keys = card.keys();
    let keys = match keys.find("[RET]") {
        Some(at) => format!("{}[RET] back", &keys[..at]),
        None => format!("{keys}  [RET] back"),
    };
    shown.row(&keys, Face::Prompt);
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

    /// The card's title and fenced lines for the question answered now.
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
        lines.extend(
            asked
                .question
                .lines()
                .map(|line| (line.to_owned(), Face::Target)),
        );
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
