//! A question's or a plan's card, as tall as its text needs (ADR 0180): every row of it, the
//! page of them that fits the room there is, and which of a question's options have been on
//! screen.
//!
//! Must not depend on: the app or the UI; the caller gives the width and the rows there are, and
//! says when a page is on screen.

use std::ops::Range;

use super::frame::{ACROSS, Key, Shown, TOP, chips, room_in};
use super::rows::{Break, Row, rows_of};
use super::{MAX_WHOLE, Questions};
use crate::sessions::native::render::Face;
use crate::sessions::native::wire::card::Asked;

/// The fewest of Claude's rows a page shows. A window that gives fewer cuts the card, which then
/// takes no answer.
const LEAST: usize = 3;
/// The keys of a page with more below, and above.
const NEXT: Key = ("SPC", "next", "next");
const BACK: Key = ("DEL", "back", "back");

/// The rows a card says, for a width, each in its face, and where a question's options are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Body {
    width: usize,
    title: String,
    rows: Vec<(Row, Face)>,
    options: Vec<Range<usize>>,
}

impl Body {
    /// `plan`'s, cut by its request if `cut`.
    pub(super) fn plan(plan: &str, cut: bool, width: usize) -> Self {
        let mut body = Self::new("Claude's plan, as its request gives it", width);
        body.text(plan, Face::Target, &[]);
        if cut {
            body.say("(the request cut it)");
        }
        body
    }

    /// The question of `questions` answered now: what it asks, then each option.
    pub(super) fn question(questions: &Questions, width: usize) -> Self {
        let mut body = Self::new(&questions.title(), width);
        let Some(asked) = questions.current() else {
            return body;
        };
        if !asked.header.is_empty() {
            body.text(&asked.header, Face::Dim, &[]);
        }
        body.text(&asked.question, Face::Target, &[]);
        body.rows.push((Row::default(), Face::Dim));
        for index in 0..asked.options.len() {
            body.option(asked, index, questions.picks(index));
        }
        body
    }

    fn new(title: &str, width: usize) -> Self {
        Self {
            width,
            title: title.to_owned(),
            rows: Vec::new(),
            options: Vec::new(),
        }
    }

    /// `asked`'s option `index`: its number on a chip and whether it is `picked`, oso's words,
    /// then its label, and its description under it, indented.
    fn option(&mut self, asked: &Asked, index: usize, picked: bool) {
        let (label, description) = &asked.options[index];
        let start = self.rows.len();
        let number = (format!(" {} ", index + 1), Face::CardKey);
        let mark = mark(asked.multi, picked);
        self.text(
            label,
            Face::Target,
            &[number, (mark.to_owned(), Face::CardTitle)],
        );
        if !description.is_empty() {
            let indent = " ".repeat(4);
            self.text(description, Face::Dim, &[(indent, Face::Dim)]);
        }
        self.options.push(start..self.rows.len());
    }

    /// Marks the options of `questions`' question answered now as they are chosen: no row moves.
    fn remark(&mut self, questions: &Questions) {
        let multi = questions.current().is_some_and(|asked| asked.multi);
        for (index, rows) in self.options.iter().enumerate() {
            let lead = self.rows.get_mut(rows.start).map(|(row, _)| &mut row.lead);
            if let Some((text, _)) = lead.and_then(|lead| lead.get_mut(1)) {
                mark(multi, questions.picks(index)).clone_into(text);
            }
        }
    }

    /// Claude's `text` in `face`, after `lead` on its first row and its cells blank on the rest.
    fn text(&mut self, text: &str, face: Face, lead: &[(String, Face)]) {
        let indent = super::frame::cells_in(lead);
        let room = room_in(self.width).saturating_sub(indent);
        let left = MAX_WHOLE.saturating_sub(self.rows.len()).max(1);
        let (rows, more) = rows_of(text, room, Break::Words, left);
        for (at, mut row) in rows.into_iter().enumerate() {
            if at == 0 {
                row.lead = lead.to_vec();
            } else if indent > 0 {
                row.lead = vec![(" ".repeat(indent), Face::Dim)];
            }
            self.rows.push((row, face));
        }
        if more {
            self.say("(the rest is cut)");
        }
    }

    /// A row of oso's among Claude's.
    fn say(&mut self, text: &str) {
        let row = Row::said(text, room_in(self.width));
        self.rows.push((row, Face::Dim));
    }
}

/// What is before an option's label, of a question that takes several if `multi`.
fn mark(multi: bool, picked: bool) -> &'static str {
    match (multi, picked) {
        (true, true) => " [x] ",
        (true, false) => " [ ] ",
        (false, _) => " ",
    }
}

/// Where a card's page is, and what of it has been on screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    body: Option<Body>,
    /// The first row shown, and the rows shown last.
    top: usize,
    on: Range<usize>,
    /// The rows that have been on screen, for the body they are of.
    rows: Vec<bool>,
    /// The options whose rows all have been.
    options: Vec<bool>,
}

impl Page {
    /// An option of `questions`' question was chosen or unchosen.
    pub(super) fn remark(&mut self, questions: &Questions) {
        if let Some(body) = &mut self.body {
            body.remark(questions);
        }
    }

    /// The card, `keys` its keys, as text `width` cells wide and at most `room` rows tall, from
    /// `body`, made once for a width: all of it, or the page there is, which says so on a row of
    /// oso's own above the keys.
    pub(super) fn shown(
        &mut self,
        (width, room): (usize, usize),
        keys: &[Key],
        body: impl FnOnce() -> Body,
    ) -> Shown {
        let body = match self.body.take() {
            Some(body) if body.width == width => body,
            // Broken anew, its rows are others: what was on screen were not they.
            _ => {
                let body = body();
                self.rows = vec![false; body.rows.len()];
                body
            }
        };
        let count = body.rows.len();
        self.options.resize(body.options.len(), false);
        let mut shown = Shown::default();
        let title = [(body.title.clone(), Face::CardTitle)];
        shown.edge(TOP, &title, width);
        let keys = chips("", keys, width);
        let fits = room.saturating_sub(1 + keys.len());
        let page = if count <= fits {
            count
        } else {
            fits.saturating_sub(1).max(LEAST.min(count))
        };
        self.top = self.top.min(count - page);
        self.on = self.top..self.top + page;
        for (row, face) in &body.rows[self.on.clone()] {
            shown.fenced_row(row, *face, width);
        }
        if page < count {
            let mark = if self.on.end < count { "▼" } else { "▲" };
            let said = format!(
                "{mark} rows {}-{} of {count}",
                self.on.start + 1,
                self.on.end
            );
            let turns: Vec<Key> = [(self.on.end < count, NEXT), (self.on.start > 0, BACK)]
                .into_iter()
                .filter_map(|(there, key)| there.then_some(key))
                .collect();
            // One row, whatever it must leave out: the keys before where the page is.
            let said = chips(&said, &turns, width);
            let row = match said.len() {
                1 => said,
                _ => chips("", &turns, width),
            };
            shown.edge(ACROSS, row.first().map_or(&[], Vec::as_slice), width);
        }
        shown.bottom(&keys, width);
        self.body = Some(body);
        shown
    }

    /// `pages` pages on, back if negative, with a row of the one before. Whether the page
    /// changed.
    pub(super) fn turn(&mut self, pages: isize) -> bool {
        let count = self.rows.len();
        let page = self.on.len();
        let step = page
            .saturating_sub(1)
            .max(1)
            .saturating_mul(pages.unsigned_abs());
        let top = match pages < 0 {
            true => self.top.saturating_sub(step),
            false => self.top.saturating_add(step),
        };
        let top = top.min(count.saturating_sub(page));
        std::mem::replace(&mut self.top, top) != top
    }

    /// What was drawn last is on screen.
    pub(super) fn on_screen(&mut self) {
        let Some(body) = &self.body else { return };
        for seen in self.rows.get_mut(self.on.clone()).into_iter().flatten() {
            *seen = true;
        }
        for (seen, rows) in self.options.iter_mut().zip(&body.options) {
            let all = (self.rows.get(rows.clone())).is_some_and(|rows| rows.iter().all(|&on| on));
            *seen |= all;
        }
    }

    /// Whether option `number`, from 1, has been on screen.
    pub(super) fn has_shown(&self, number: usize) -> bool {
        let at = number.checked_sub(1);
        at.and_then(|at| self.options.get(at)) == Some(&true)
    }

    /// The page goes to option `number`'s first row not on screen yet.
    pub(super) fn go_to(&mut self, number: usize) {
        let body = self.body.as_ref();
        let option = number.checked_sub(1);
        let rows = body.and_then(|body| body.options.get(option?)).cloned();
        if let Some(rows) = rows {
            let unseen = rows.clone().find(|&row| self.rows.get(row) != Some(&true));
            self.top = unseen.unwrap_or(rows.start);
        }
    }
}
