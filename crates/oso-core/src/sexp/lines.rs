//! Runs read by lexing a buffer's lines one at a time, forward or backward, for a buffer with
//! no highlights (ADR 0190).
//!
//! Must not depend on: `window` or `word`; it reads text over a display snapshot.

use std::ops::Range;

use super::lex::{self, LINE_MAX};
use super::{Kind, ScanError, Table, charge};
use crate::display_map::DisplaySnapshot;
use crate::motion::WalkBudget;

/// Lines lexed one at a time going forward.
pub(super) struct LexAhead<'a> {
    table: Table,
    pieces: Box<dyn Iterator<Item = &'a str> + 'a>,
    piece: &'a [u8],
    /// Where the next line read starts.
    at: usize,
    /// Whether the next line read starts past its line's start, as a long line's rest does.
    long: bool,
    line: Vec<u8>,
    runs: Vec<(Range<usize>, Kind)>,
    next: usize,
}

impl<'a> LexAhead<'a> {
    /// From the start of the line that holds `from`, or from `from` if that line is longer
    /// than [`LINE_MAX`] before it.
    pub(super) fn new(
        display: &'a DisplaySnapshot,
        table: Table,
        from: usize,
        budget: &mut WalkBudget,
    ) -> Result<Self, ScanError> {
        let mut start = from;
        let mut long = true;
        'back: for piece in display.text_before(from) {
            for &b in piece.as_bytes().iter().rev() {
                if b == b'\n' {
                    long = false;
                    break 'back;
                }
                if from - start == LINE_MAX {
                    break 'back;
                }
                start -= 1;
            }
        }
        if start == 0 {
            long = false;
        }
        if long {
            start = from;
        }
        charge(budget, from - start)?;
        Ok(Self {
            table,
            pieces: Box::new(display.text_in(start..display.len())),
            piece: &[],
            at: start,
            long,
            line: Vec::new(),
            runs: Vec::new(),
            next: 0,
        })
    }

    /// The next run, and what reading it cost.
    pub(super) fn next(&mut self) -> Option<((Range<usize>, Kind), usize)> {
        let mut cost = 0;
        if self.next == self.runs.len() {
            cost = self.read_line();
            if self.runs.is_empty() {
                return None;
            }
        }
        let run = self.runs[self.next].clone();
        self.next += 1;
        Some((run, cost))
    }

    /// Reads and lexes the next line; returns the bytes it read.
    fn read_line(&mut self) -> usize {
        self.line.clear();
        self.runs.clear();
        self.next = 0;
        let start = self.at;
        let mut ended = false;
        let mut long = self.long;
        while !ended {
            if self.piece.is_empty() {
                match self.pieces.next() {
                    Some(piece) => self.piece = piece.as_bytes(),
                    None => break,
                }
                continue;
            }
            let take = self
                .piece
                .iter()
                .position(|&b| b == b'\n')
                .map_or(self.piece.len(), |at| {
                    ended = true;
                    at + 1
                });
            if !long && self.line.len() + take <= LINE_MAX {
                self.line.extend_from_slice(&self.piece[..take]);
            } else if !long {
                long = true;
            }
            self.at += take;
            self.piece = &self.piece[take..];
        }
        if self.at > start {
            if long {
                self.runs.push((start..self.at, Kind::Code));
            } else {
                lex::lex(self.table, &self.line, start, &mut self.runs);
            }
        }
        self.long = false;
        self.at - start
    }
}

/// Lines lexed one at a time going backward.
pub(super) struct LexBehind<'a> {
    table: Table,
    pieces: Box<dyn Iterator<Item = &'a str> + 'a>,
    piece: &'a [u8],
    /// Where the next line read ends.
    at: usize,
    /// Whether a line break was read that ends the next line.
    newline: bool,
    line: Vec<u8>,
    runs: Vec<(Range<usize>, Kind)>,
}

impl<'a> LexBehind<'a> {
    pub(super) fn new(display: &'a DisplaySnapshot, table: Table, to: usize) -> Self {
        Self {
            table,
            pieces: Box::new(display.text_before(to)),
            piece: &[],
            at: to,
            newline: false,
            line: Vec::new(),
            runs: Vec::new(),
        }
    }

    /// The next run back, and what reading it cost.
    pub(super) fn next(&mut self) -> Option<((Range<usize>, Kind), usize)> {
        let mut cost = 0;
        while self.runs.is_empty() {
            // A line can be empty only where it ends at the reader's start.
            let read = self.read_line();
            if read == 0 {
                return None;
            }
            cost += read;
        }
        self.runs.pop().map(|run| (run, cost))
    }

    /// Reads and lexes the line that ends at `at`; returns the bytes it read.
    fn read_line(&mut self) -> usize {
        self.line.clear();
        let end = self.at;
        let mut long = false;
        if self.newline {
            self.line.push(b'\n');
            self.at -= 1;
            self.newline = false;
        }
        'line: loop {
            if self.piece.is_empty() {
                match self.pieces.next() {
                    Some(piece) => self.piece = piece.as_bytes(),
                    None => break,
                }
                continue;
            }
            while let Some((&b, rest)) = self.piece.split_last() {
                self.piece = rest;
                if b == b'\n' {
                    self.newline = true;
                    break 'line;
                }
                self.at -= 1;
                if self.line.len() < LINE_MAX {
                    self.line.push(b);
                } else {
                    long = true;
                }
            }
        }
        if end > self.at {
            if long {
                self.runs.push((self.at..end, Kind::Code));
            } else {
                self.line.reverse();
                lex::lex(self.table, &self.line, self.at, &mut self.runs);
            }
        }
        end - self.at + usize::from(self.newline)
    }
}
