//! A table laid out, and its lines between box rules as drawn (ADR 0165, "The look: tables").
//!
//! Must not depend on: the syntax threads, the editor or the theme.

use super::Align;
use super::grid::Grid;
use crate::sessions::native::shown::Style;

/// What ends a cell's row that is broken inside a word, so that no address reads as two: the mark
/// a wrapped row has (ADR 0100).
const BROKEN: &str = "↩";

/// A table as drawn: its text, and what each stretch of it is.
#[derive(Debug, Default)]
pub(in super::super) struct Drawn {
    text: String,
    /// Where each stretch ends, and its style.
    spans: Vec<(usize, Option<Style>)>,
    /// Where each line ends.
    lines: Vec<usize>,
}

impl Drawn {
    pub(super) fn put(&mut self, text: &str, style: Option<Style>) {
        self.text.push_str(text);
        self.mark(style);
    }

    fn repeat(&mut self, text: &str, times: usize, style: Option<Style>) {
        for _ in 0..times {
            self.text.push_str(text);
        }
        self.mark(style);
    }

    /// What was put since the last span is of `style`.
    fn mark(&mut self, style: Option<Style>) {
        let end = self.text.len();
        let line = self.lines.last().copied().unwrap_or(0);
        match self.spans.last_mut() {
            Some((last, same)) if *same == style && *last > line => *last = end,
            Some((last, _)) if *last == end => {}
            _ if end == 0 => {}
            _ => self.spans.push((end, style)),
        }
    }

    fn end_line(&mut self) {
        self.lines.push(self.text.len());
    }

    /// Gives `line` each line's stretches.
    pub(in super::super) fn each<'a>(&'a self, mut line: impl FnMut(&[(&'a str, Option<Style>)])) {
        let mut spans = self.spans.iter().peekable();
        let (mut at, mut out) = (0, Vec::new());
        for end in &self.lines {
            out.clear();
            while let Some((stop, style)) = spans.next_if(|(stop, _)| stop <= end) {
                out.push((&self.text[at..*stop], *style));
                at = *stop;
            }
            line(&out);
        }
    }
}

/// A table laid out: how many lines it takes, and what draws them.
#[derive(Debug)]
pub(in super::super) struct Layout {
    pub(super) grid: Grid,
    pub(super) indent: usize,
    pub(super) widths: Vec<usize>,
    pub(super) aligns: Vec<Align>,
    /// A rule is between each two of its rows.
    pub(super) ruled: bool,
    pub(in super::super) lines: usize,
}

impl Layout {
    /// The table's lines.
    pub(in super::super) fn draw(&self) -> Drawn {
        let Self {
            grid,
            indent,
            widths,
            aligns,
            ..
        } = self;
        let mut out = Drawn::default();
        let rule = |out: &mut Drawn, [first, between, last]: [&str; 3]| {
            out.repeat(" ", *indent, None);
            for (at, width) in widths.iter().enumerate() {
                out.put(if at == 0 { first } else { between }, Some(Style::Rule));
                out.repeat("─", width + 2, Some(Style::Rule));
            }
            out.put(last, Some(Style::Rule));
            out.end_line();
        };
        rule(&mut out, ["┌", "┬", "┐"]);
        for (number, row) in grid.cells.chunks(widths.len()).enumerate() {
            if number == 1 || (self.ruled && number > 1) {
                rule(&mut out, ["├", "┼", "┤"]);
            }
            let tallest = row.iter().map(|cell| cell.rows.len()).max().unwrap_or(1);
            for line in 0..tallest {
                out.repeat(" ", *indent, None);
                for ((cell, width), align) in row.iter().zip(widths).zip(aligns) {
                    let row = grid.rows[cell.rows.clone()].get(line);
                    let cells = row.map_or(0, |row| row.cells);
                    let room = width.saturating_sub(cells);
                    let before = match align {
                        Align::Left => 0,
                        Align::Center => room / 2,
                        Align::Right => room,
                    };
                    out.put("│", Some(Style::Rule));
                    out.repeat(" ", 1 + before, Some(Style::Rule));
                    if let Some(row) = row {
                        grid.stretches(&mut out, cell, row.text.clone());
                    }
                    out.repeat(" ", room - before, Some(Style::Rule));
                    let broken = row.is_some_and(|row| row.broken);
                    out.put(if broken { BROKEN } else { " " }, Some(Style::Rule));
                }
                out.put("│", Some(Style::Rule));
                out.end_line();
            }
        }
        rule(&mut out, ["└", "┴", "┘"]);
        out
    }
}
