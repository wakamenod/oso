//! Fundamental mode's indentation rule (ADR 0115): a line takes the indentation of the line
//! before, or none if that line is blank, as Emacs's `indent-relative` does.
//!
//! Must not depend on: `buffer`; it reads only the line before.

use super::pass::{Head, Lexer};
use super::{Answer, Settings};

pub(super) struct Fundamental {
    /// The indentation of the line before, if it is not blank.
    before: Option<usize>,
}

impl Lexer for Fundamental {
    fn fresh(_settings: Settings) -> Self {
        Self { before: None }
    }

    /// Every line is: the rule reads only the line before.
    fn is_anchor(_line: &[u8]) -> bool {
        true
    }

    fn is_inside(&self) -> bool {
        false
    }

    fn line(&mut self, text: &str, indent: usize) {
        let blank = text
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r'));
        self.before = (!blank).then_some(indent);
    }

    fn answer(&self, _head: &Head, _new: bool) -> Option<Answer> {
        Some(Answer::Column(self.before.unwrap_or(0)))
    }
}
