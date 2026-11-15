//! Positions a server reads and sends, to byte offsets in a [`Rope`] and back (ADR 0138), in
//! the encoding agreed: UTF-8 when the server offers it, UTF-16 otherwise. Each conversion is
//! O(log n), on any line, through the rope's byte and UTF-16 counts.
//!
//! Must not depend on: `framing`, `json` or `uri`; this sees only ropes and numbers.
//!
//! A position a server sends is clamped, so nothing it sends makes oso index out of the text:
//! a line past the end to the end, a column past its line to the line's end, before its line
//! break, and a column inside a character, half a surrogate pair included, to its start.
//!
//! Lines are oso's: `\n` and `\r\n` end one, a lone `\r` does not. The protocol also counts a
//! lone `\r`, so a buffer that holds one is not opened with a server (ADR 0138, step 2 as
//! built).

use std::ops::Range;

use oso_core::rope::{Point, Rope};

/// How a position counts a line's characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// In UTF-8 bytes, oso's own columns.
    Utf8,
    /// In UTF-16 code units, the protocol's default.
    Utf16,
}

impl Encoding {
    /// What oso offers in `general.positionEncodings`, the one it prefers first.
    pub const OFFERED: [&str; 2] = ["utf-8", "utf-16"];

    /// The encoding a server chose in `capabilities.positionEncoding`: UTF-16 if it said
    /// nothing, and `None` for one oso did not offer.
    pub fn chosen(answer: Option<&str>) -> Option<Self> {
        match answer {
            None | Some("utf-16") => Some(Self::Utf16),
            Some("utf-8") => Some(Self::Utf8),
            Some(_) => None,
        }
    }
}

/// A place in a document: a line from 0 and a column in the encoding agreed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    /// The line, from 0.
    pub line: u32,
    /// The column, from 0, in the encoding's units.
    pub character: u32,
}

impl Position {
    /// The position at `line` and `character`.
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// The byte offset in `text` of `position`, clamped (see the module docs). O(log n).
pub fn to_offset(text: &Rope, position: Position, encoding: Encoding) -> usize {
    let row = usize::try_from(position.line).unwrap_or(usize::MAX);
    if row > text.summary().lines {
        return text.len();
    }
    let column = usize::try_from(position.character).unwrap_or(usize::MAX);
    match encoding {
        Encoding::Utf8 => text.point_to_offset(Point::new(row, column)),
        Encoding::Utf16 => {
            let start = text.line_start(row);
            let units = text.offset_to_utf16(start).saturating_add(column);
            text.utf16_to_offset(units).min(text.line_end(row))
        }
    }
}

/// The byte range in `text` from `start` to `end`, each clamped; a range that ends before it
/// starts is read the other way round. O(log n).
pub fn to_range(text: &Rope, start: Position, end: Position, encoding: Encoding) -> Range<usize> {
    let (start, end) = (
        to_offset(text, start, encoding),
        to_offset(text, end, encoding),
    );
    start.min(end)..start.max(end)
}

/// The position of the byte offset `offset` in `text`, clipped to the text and to a
/// character's start. Counts past `u32::MAX` read as it. O(log n).
pub fn from_offset(text: &Rope, offset: usize, encoding: Encoding) -> Position {
    let point = text.offset_to_point(offset);
    let character = match encoding {
        Encoding::Utf8 => point.column,
        Encoding::Utf16 => {
            let start = text.line_start(point.row);
            text.offset_to_utf16(start + point.column) - text.offset_to_utf16(start)
        }
    };
    Position::new(saturate(point.row), saturate(character))
}

fn saturate(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
