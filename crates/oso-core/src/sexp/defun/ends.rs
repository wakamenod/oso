//! Where a defun ends, by its head's rule (ADR 0190, Q2): after a bracket group or a `;` in the
//! languages of braces, before the next head of its level in sections, at its `\end` in a
//! LaTeX environment.
//!
//! Must not depend on: `window` or `word`; it reads text and runs over a display snapshot.

use super::lines::LinesAhead;
use super::{ByLines, Found, line_after};
use crate::motion::WalkBudget;
use crate::sexp::ScanError;
use crate::sexp::heads::Ends;
use crate::sexp::read::{self, Ahead};

impl ByLines<'_> {
    /// Where the defun of `found` ends.
    pub(super) fn end_of(
        &mut self,
        found: &Found,
        budget: &mut WalkBudget,
    ) -> Result<usize, ScanError> {
        match &found.head.ends {
            Ends::Braces => self.end_of_code(found, &['{'], &[';'], true, budget),
            Ends::Value => self.end_of_code(found, &['{', '['], &[','], true, budget),
            Ends::Group => self.end_of_code(found, &['(', '[', '{'], &[], false, budget),
            Ends::Level => self.end_of_level(found, budget),
            Ends::Env(name) => self.end_of_env(found, name, budget),
        }
    }

    /// The end of a defun of code: past the line of the bracket group from the first of
    /// `groups` outside brackets, or of the first of `ends` outside them. With `line_stops`,
    /// before a blank line or the next head, if either comes first, or before the line of a
    /// closer of what holds it.
    fn end_of_code(
        &mut self,
        found: &Found,
        groups: &[char],
        ends: &[char],
        line_stops: bool,
        budget: &mut WalkBudget,
    ) -> Result<usize, ScanError> {
        let syntax = self.syntax;
        let display = syntax.display;
        let table = syntax.table;
        let (mut depth, mut group) = (0usize, false);
        let mut line = found.line;
        let mut from = found.at;
        loop {
            // A line break outside brackets, after which the next line decides.
            let newline = {
                let mut reader = Ahead::new(&syntax, from, budget)?;
                loop {
                    let Some((at, c)) = reader.code_char()? else {
                        return Ok(display.len());
                    };
                    if c == '\n' {
                        line = at + 1;
                        if depth == 0 && line_stops {
                            break at + 1;
                        }
                    } else if read::is_opener(table, c, None) {
                        group |= depth == 0 && groups.contains(&c);
                        depth += 1;
                    } else if read::is_closer(table, c) {
                        if depth == 0 {
                            // What holds it closes: it ends before the closer's line.
                            return Ok(if line > found.line {
                                line
                            } else {
                                line_after(display, found.line)
                            });
                        }
                        depth -= 1;
                        if depth == 0 && group {
                            return Ok(Self::past_line(&mut reader)?.unwrap_or(display.len()));
                        }
                    } else if depth == 0 && ends.contains(&c) {
                        return Ok(Self::past_line(&mut reader)?.unwrap_or(display.len()));
                    }
                }
            };
            let mut lines = LinesAhead::new(display, newline);
            let Some(next) = lines.next(budget)? else {
                return Ok(newline);
            };
            if next.blank {
                return Ok(newline);
            }
            if let Some(head) = self.head_of(next, lines.text(), budget)? {
                return self.above(&head, newline, budget);
            }
            from = newline;
        }
    }

    /// Past the next line break `reader` reads, or `None` at the end.
    fn past_line(reader: &mut Ahead<'_, '_>) -> Result<Option<usize>, ScanError> {
        while let Some((at, c)) = reader.code_char()? {
            if c == '\n' {
                return Ok(Some(at + 1));
            }
        }
        Ok(None)
    }

    /// The end of a section: before the next head of its level or a higher one, and before
    /// the blank lines and the comment just above that head.
    fn end_of_level(&mut self, found: &Found, budget: &mut WalkBudget) -> Result<usize, ScanError> {
        let display = self.syntax.display;
        let mut lines = LinesAhead::new(display, found.line);
        let Some(first) = lines.next(budget)? else {
            return Ok(display.len());
        };
        let mut kept = first.end;
        // Where a run of comment lines with nothing between them and here starts, and what
        // was kept before it.
        let mut comments: Option<usize> = None;
        while let Some(line) = lines.next(budget)? {
            if line.blank {
                comments = None;
                continue;
            }
            if let Some(next) = self.head_of(line, lines.text(), budget)?
                && next.head.level <= found.head.level
            {
                return Ok(comments.unwrap_or(kept));
            }
            if self.is_above(line, lines.text(), budget)? {
                comments.get_or_insert(kept);
            } else {
                comments = None;
            }
            kept = line.end;
        }
        Ok(kept)
    }

    /// The end of a LaTeX environment: past the line of the `\end{name}` that closes it.
    fn end_of_env(
        &mut self,
        found: &Found,
        name: &[u8],
        budget: &mut WalkBudget,
    ) -> Result<usize, ScanError> {
        let begin = [b"\\begin{".as_slice(), name, b"}"].concat();
        let end = [b"\\end{".as_slice(), name, b"}"].concat();
        let display = self.syntax.display;
        let mut lines = LinesAhead::new(display, found.line);
        let mut depth = 0usize;
        while let Some(line) = lines.next(budget)? {
            let text = lines.text();
            let mut at = 0;
            while at < text.len() {
                let rest = &text[at..];
                if rest[0] == b'%'
                    && text[..at].iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 0
                {
                    break;
                }
                if rest.starts_with(&begin) {
                    depth += 1;
                    at += begin.len();
                } else if rest.starts_with(&end) {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Ok(line.end);
                    }
                    at += end.len();
                } else {
                    at += 1;
                }
            }
        }
        Ok(display.len())
    }
}
