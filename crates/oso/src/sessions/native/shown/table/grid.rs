//! A table's cells (ADR 0165, "The look: tables"): their text with its marks hidden, measured in the
//! display's clusters, and wrapped to their columns.
//!
//! Must not depend on: the syntax threads, the editor or the theme.

use std::ops::Range;

use oso_core::display_map::{CellKind, text_clusters};

use super::boxed::Drawn;
use super::breaks::{Class, Wrapped, breaks, rows_of};
use super::{cells, inner};
use crate::sessions::native::shown::inline::Marks;
use crate::sessions::native::shown::signs::{calm, stands};
use crate::sessions::native::shown::{Style, joins};

/// A cell's text as drawn: its place in [`Grid::text`], and its stretches' styles.
#[derive(Clone, Debug, Default)]
pub(super) struct Cell {
    pub(super) text: Range<usize>,
    pub(super) spans: Range<usize>,
    /// Its rows once wrapped, in [`Grid::rows`].
    pub(super) rows: Range<usize>,
    pub(super) cells: usize,
}

/// A table's cells, row after row, and their columns.
#[derive(Debug, Default)]
pub(super) struct Grid {
    pub(super) text: String,
    /// Each stretch's end in `text`, and its style.
    pub(super) spans: Vec<(usize, Option<Style>)>,
    pub(super) cells: Vec<Cell>,
    pub(super) rows: Vec<Wrapped>,
    /// Each column's widest cell and widest word.
    pub(super) widest: Vec<(usize, usize)>,
    /// Room to put a cluster between spaces in.
    pub(super) between: String,
}

impl Grid {
    /// Reads a row's cells, whose text is of `style` outside any mark.
    pub(super) fn read(&mut self, line: &str, columns: usize, style: Option<Style>) -> Option<()> {
        let inner = inner(line);
        // Where a link hides its target: nothing else of its row could read as one.
        let mut links = (0, true);
        for cell in cells(inner, columns) {
            if cell.contains("](") {
                links.0 += 1;
            } else {
                links.1 &= calm(cell, false);
            }
        }
        let hides = links.0 == 1 && links.1;
        let mut written = cells(inner, columns);
        for column in 0..columns {
            let source = written.next().unwrap_or_default();
            let cell = if source.contains(['\\', '\t']) {
                let mut plain = String::with_capacity(source.len());
                unescape(source, &mut plain);
                self.cell(&plain, style, hides)
            } else {
                self.cell(source, style, hides)
            };
            let (cells, word) = self.measure(&cell)?;
            let widest = &mut self.widest[column];
            *widest = (widest.0.max(cells), widest.1.max(word));
            self.cells.push(Cell { cells, ..cell });
        }
        Some(())
    }

    /// Puts `source`, a cell's text, with its marks hidden; as written where one between two
    /// characters that could join would go.
    fn cell(&mut self, source: &str, style: Option<Style>, hides: bool) -> Cell {
        let (text, spans) = (self.text.len(), self.spans.len());
        if !source.contains(['*', '`', '[']) {
            self.text.push_str(source);
            self.spans.push((self.text.len(), style));
            return Cell {
                text: text..self.text.len(),
                spans: spans..self.spans.len(),
                ..Cell::default()
            };
        }
        let mut marks = Marks::default();
        marks.read(source, 0, style, hides);
        let mut before: Option<char> = None;
        let mut joined = false;
        for (range, put, style) in marks.pieces() {
            let piece = put.unwrap_or(&source[range]);
            if piece.is_empty() {
                continue;
            }
            if before.is_some() {
                joined |= joins(before) || joins(piece.chars().next());
            }
            before = piece.chars().next_back();
            self.text.push_str(piece);
            self.spans.push((self.text.len(), style));
        }
        if joined {
            self.text.truncate(text);
            self.spans.truncate(spans);
            self.text.push_str(source);
            self.spans.push((self.text.len(), style));
        }
        Cell {
            text: text..self.text.len(),
            spans: spans..self.spans.len(),
            ..Cell::default()
        }
    }

    /// A cell's cells, and its widest word's; `None` for one that is not drawn as it measures
    /// between a table's rules.
    fn measure(&mut self, cell: &Cell) -> Option<(usize, usize)> {
        let text = &self.text[cell.text.clone()];
        if text.bytes().all(|byte| matches!(byte, b' '..=b'~')) {
            let words = text.split(' ').map(str::len);
            return Some((text.len(), words.max().unwrap_or(0)));
        }
        let (mut cells, mut word, mut widest) = (0, 0, 0);
        let mut last = None;
        for cluster in text_clusters(text, 0) {
            // What the display's limits cut depends on what is before it.
            if matches!(cluster.kind, CellKind::PastLimit(_) | CellKind::LineBreak) {
                return None;
            }
            // A control joins nothing.
            let control = matches!(cluster.kind, CellKind::Control(_));
            if !control && !apart(cluster.text, &mut self.between) {
                return None;
            }
            let next = Class::of(cluster.text);
            if last.is_some_and(|last| breaks(last, next)) {
                word = 0;
            }
            cells += cluster.cells;
            if !next.space {
                word += cluster.cells;
            }
            widest = widest.max(word);
            last = Some(next);
        }
        Some((cells, widest))
    }

    /// Wraps each cell to its column's width; the lines of the table, at most `most`, and whether
    /// its rows have rules between them.
    pub(super) fn wrap(&mut self, widths: &[usize], most: usize) -> Option<(usize, bool)> {
        let (mut lines, mut ruled) = (0, false);
        let rows = self.cells.len() / widths.len();
        for (number, row) in self.cells.chunks_mut(widths.len()).enumerate() {
            let mut tallest = 1;
            for (cell, width) in row.iter_mut().zip(widths) {
                let start = self.rows.len();
                if cell.cells <= *width {
                    self.rows.push(Wrapped {
                        text: cell.text.clone(),
                        cells: cell.cells,
                        broken: false,
                    });
                } else {
                    let text = cell.text.clone();
                    rows_of(&self.text, text, *width, &mut self.rows, most - lines)?;
                }
                cell.rows = start..self.rows.len();
                tallest = tallest.max(self.rows.len() - start);
            }
            lines += tallest;
            // Rows that wrap run together with no rule between them.
            ruled |= number > 0 && tallest > 1;
            if lines > most {
                return None;
            }
        }
        // Its top and bottom, its header's, and those between its rows.
        let rules = 2 + usize::from(rows > 1) + if ruled { rows.saturating_sub(2) } else { 0 };
        (lines + rules <= most).then_some((lines + rules, ruled))
    }

    /// Puts `text`, some of `cell`'s, in its styles.
    pub(super) fn stretches(&self, out: &mut Drawn, cell: &Cell, text: Range<usize>) {
        let spans = &self.spans[cell.spans.clone()];
        let first = spans.partition_point(|(end, _)| *end <= text.start);
        let mut at = text.start;
        for (end, style) in &spans[first..] {
            if at >= text.end {
                break;
            }
            let end = (*end).min(text.end);
            out.put(&self.text[at..end], *style);
            at = end;
        }
    }
}

/// `cell` with its pipes' backslashes gone, and its tabs as spaces.
fn unescape(cell: &str, out: &mut String) {
    let mut chars = cell.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {}
            '\\' => {
                out.push(c);
                // Escaped, it escapes nothing.
                if let Some('\\') = chars.peek() {
                    out.extend(chars.next());
                }
            }
            '\t' => out.push(' '),
            c => out.push(c),
        }
    }
}

/// Whether `cluster` is one beside a space too, as a table's rules are past one; `between` is
/// room to look in.
pub(super) fn apart(cluster: &str, between: &mut String) -> bool {
    let mut chars = cluster.chars();
    if chars.next().is_none_or(|c| stands(c) || letter(c)) && chars.next().is_none() {
        return true;
    }
    between.clear();
    between.extend([" ", cluster, " "]);
    let mut found = text_clusters(between, 0);
    let mut is = |text: &str| found.next().is_some_and(|cluster| cluster.text == text);
    is(" ") && is(cluster) && is(" ")
}

/// Whether `c` is a letter of Hebrew, Arabic, Devanagari or Thai that joins no space.
pub(super) fn letter(c: char) -> bool {
    matches!(c, '\u{5d0}'..='\u{5ea}' | '\u{620}'..='\u{64a}' | '\u{904}'..='\u{939}' | '\u{e01}'..='\u{e30}')
}
