//! What `textDocument/hover` answers, read within the bounds (ADR 0138, Hover, step 10): its
//! `contents` as one text, at most [`HOVER_BYTES_MAX`], and whether it is Markdown.
//!
//! Must not depend on: `server`, threads or processes; this reads values, and the UI shows the
//! text as a buffer's, its controls boxed.
//!
//! - A `MarkupContent` is its value, Markdown or plain text as its `kind` says.
//! - A `MarkedString`, or a list of them, is Markdown: a string as it is, and a
//!   `{language, value}` as a fenced block naming the language, the pieces joined by `---`
//!   lines, as Eglot joins them. A fence is longer than any run of backticks in its value, so
//!   no value ends it early, and a language name keeps only the characters one is spelled
//!   with.
//! - `\r\n` reads as `\n`. Past the bound the text is cut at a character, with `…`. An answer
//!   of `null`, or one whose text is only blanks, describes nothing.
//! - `Debug` gives the text's size only: a server may quote what it read.

use serde_json::Value;

use crate::json::array;
use crate::limits::HOVER_BYTES_MAX;

/// The most pieces of a list of `MarkedString`s read.
const PIECES_MAX: usize = 256;

/// The longest language name a fenced block keeps.
const LANGUAGE_BYTES_MAX: usize = 32;

/// What a hover answered: its text, and whether it is Markdown.
#[derive(Clone, PartialEq, Eq)]
pub struct Hover {
    pub text: String,
    pub markdown: bool,
}

impl std::fmt::Debug for Hover {
    /// The size only: the text may quote what the server read.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hover")
            .field("bytes", &self.text.len())
            .field("markdown", &self.markdown)
            .finish()
    }
}

/// The text a hover's `result` holds, within [`HOVER_BYTES_MAX`]; `None` if it describes
/// nothing or does not read.
pub fn hover(result: &Value) -> Option<Hover> {
    let contents = result.get("contents")?;
    let mut text = Text::default();
    let markdown = match contents {
        Value::Object(fields) if fields.contains_key("kind") => {
            text.push(fields.get("value")?);
            fields.get("kind").and_then(Value::as_str) != Some("plaintext")
        }
        Value::Array(_) => {
            for (index, piece) in array(contents, PIECES_MAX)?.iter().enumerate() {
                if index > 0 {
                    text.push_str("\n\n---\n\n");
                }
                text.marked(piece);
            }
            true
        }
        piece => {
            text.marked(piece);
            true
        }
    };
    let text = text.done();
    (!text.trim().is_empty()).then_some(Hover { text, markdown })
}

/// A hover's text as it is put together, within the bound.
#[derive(Default)]
struct Text {
    text: String,
    cut: bool,
}

impl Text {
    /// Adds `piece`, a string, as far as the bound lets it.
    fn push(&mut self, piece: &Value) {
        if let Some(piece) = piece.as_str() {
            self.push_str(piece);
        }
    }

    /// Adds `piece` as far as the bound lets it, at a character.
    fn push_str(&mut self, piece: &str) {
        let room = HOVER_BYTES_MAX.saturating_sub(self.text.len());
        if piece.len() > room {
            self.cut = true;
            self.text
                .push_str(&piece[..piece.floor_char_boundary(room)]);
        } else {
            self.text.push_str(piece);
        }
    }

    /// Adds a `MarkedString`: a string as it is, or a fenced block of its value.
    fn marked(&mut self, piece: &Value) {
        match piece {
            Value::String(_) => self.push(piece),
            Value::Object(fields) => {
                let Some(value) = fields.get("value").and_then(Value::as_str) else {
                    return;
                };
                let language: String = (fields.get("language").and_then(Value::as_str))
                    .unwrap_or_default()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || "+-_.#".contains(*c))
                    .take(LANGUAGE_BYTES_MAX)
                    .collect();
                let fence = "`".repeat(longest_backticks(value).max(2) + 1);
                self.push_str(&format!("{fence}{language}\n"));
                self.push_str(value.strip_suffix('\n').unwrap_or(value));
                self.push_str(&format!("\n{fence}"));
            }
            _ => {}
        }
    }

    /// The text, `\r\n` as `\n`, with `…` if it was cut.
    fn done(self) -> String {
        let mut text = self.text.replace("\r\n", "\n");
        if self.cut {
            text.push('…');
        }
        text
    }
}

/// The longest run of backticks in `value`.
fn longest_backticks(value: &str) -> usize {
    let (mut longest, mut run) = (0, 0);
    for c in value.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

#[cfg(test)]
mod tests;
