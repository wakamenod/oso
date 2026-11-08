//! Iterating over a rope's lines, without their line breaks.
//!
//! Must not depend on: `decode`, `display_map`, cells or display rows.

use std::borrow::Cow;

use super::Chunks;

/// Iterates over a rope's lines. See [`Rope::lines`](super::Rope::lines).
pub struct Lines<'a> {
    pub(super) chunks: Chunks<'a>,
    /// The unread part of the current chunk.
    pub(super) rest: &'a str,
    pub(super) done: bool,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Cow<'a, str>;

    fn next(&mut self) -> Option<Cow<'a, str>> {
        if self.done {
            return None;
        }
        let mut long: Option<String> = None;
        loop {
            if let Some(index) = self.rest.find('\n') {
                let piece = &self.rest[..index];
                self.rest = &self.rest[index + 1..];
                let line = match long {
                    None => Cow::Borrowed(piece),
                    Some(mut line) => {
                        line.push_str(piece);
                        Cow::Owned(line)
                    }
                };
                return Some(strip_cr(line));
            }
            if !self.rest.is_empty() {
                long.get_or_insert_with(String::new).push_str(self.rest);
            }
            match self.chunks.next() {
                Some(text) => self.rest = text,
                None => {
                    self.done = true;
                    return Some(long.map_or(Cow::Borrowed(""), Cow::Owned));
                }
            }
        }
    }
}

fn strip_cr(line: Cow<'_, str>) -> Cow<'_, str> {
    match line {
        Cow::Borrowed(text) => Cow::Borrowed(text.strip_suffix('\r').unwrap_or(text)),
        Cow::Owned(mut text) => {
            if text.ends_with('\r') {
                text.pop();
            }
            Cow::Owned(text)
        }
    }
}
